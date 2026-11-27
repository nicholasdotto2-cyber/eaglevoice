//! Motore locale: processo figlio `llama-server` (llama.cpp b11719, CUDA) con Qwen3.5-4B
//! residente. Flag scelti dopo le prove: `-ngl 99 -fa on -c 4096 -np 1 -rea off
//! --reasoning-budget 0`. Avvio con l'app, riscaldamento con il prompt di sistema (così il
//! prefisso è già in cache alla prima dettatura), riavvio se cade, chiusura con l'app.

use futures_util::StreamExt;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use super::{CleanupState, Emitter, EngineStatus, EVENT_ENGINE_STATE};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Parametri di campionamento scelti dopo le prove.
const TEMPERATURE: f32 = 0.2;
const TOP_P: f32 = 0.9;
/// Contesto per slot e numero di slot: con 2 slot una dettatura lunga si
/// pulisce in due metà in parallelo (decodifica a lotto di 2 quasi al costo di 1).
/// Il contesto totale passato al server è CONTEXT * SLOTS.
const CONTEXT: u32 = 4096;
pub const SLOTS: u32 = 2;
const READY_TIMEOUT: Duration = Duration::from_secs(180);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Job object con `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: se l'app muore (anche con
/// `taskkill /F`) Windows chiude l'ultimo handle del job e con lui llama-server. Senza,
/// llama-server può restare vivo con la VRAM occupata dopo la chiusura forzata dell'app.
/// FFI scritta qui (kernel32) perché `Cargo.toml` non abilita `Win32_System_JobObjects`;
/// stessa tecnica del modulo `asr\`, versione propria.
#[cfg(windows)]
pub(crate) mod kill_on_close {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    pub type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimitInformation {
        per_process_user_time_limit: i64,
        per_job_user_time_limit: i64,
        limit_flags: u32,
        minimum_working_set_size: usize,
        maximum_working_set_size: usize,
        active_process_limit: u32,
        affinity: usize,
        priority_class: u32,
        scheduling_class: u32,
    }

    #[repr(C)]
    #[derive(Default)]
    struct IoCounters {
        read_operation_count: u64,
        write_operation_count: u64,
        other_operation_count: u64,
        read_transfer_count: u64,
        write_transfer_count: u64,
        other_transfer_count: u64,
    }

    #[repr(C)]
    #[derive(Default)]
    struct ExtendedLimitInformation {
        basic: BasicLimitInformation,
        io: IoCounters,
        process_memory_limit: usize,
        job_memory_limit: usize,
        peak_process_memory_used: usize,
        peak_job_memory_used: usize,
    }

    const JOB_OBJECT_EXTENDED_LIMIT_INFORMATION: i32 = 9;
    const JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE: u32 = 0x2000;

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateJobObjectW(attributes: *mut c_void, name: *const u16) -> Handle;
        fn SetInformationJobObject(job: Handle, class: i32, info: *const c_void, len: u32) -> i32;
        fn AssignProcessToJobObject(job: Handle, process: Handle) -> i32;
        fn CloseHandle(h: Handle) -> i32;
    }

    /// Crea un job che uccide i suoi processi quando si chiude l'ultimo handle.
    pub fn create() -> Result<Handle, String> {
        unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() {
                return Err(format!("CreateJobObjectW: {}", std::io::Error::last_os_error()));
            }
            let mut info = ExtendedLimitInformation::default();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const ExtendedLimitInformation as *const c_void,
                std::mem::size_of::<ExtendedLimitInformation>() as u32,
            );
            if ok == 0 {
                let e = std::io::Error::last_os_error();
                CloseHandle(h);
                return Err(format!("SetInformationJobObject: {e}"));
            }
            Ok(h)
        }
    }

    pub fn assign_to(job: Handle, child: &std::process::Child) -> Result<(), String> {
        let ok = unsafe { AssignProcessToJobObject(job, child.as_raw_handle() as Handle) };
        if ok == 0 {
            return Err(format!("AssignProcessToJobObject: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }

    pub fn close(job: Handle) {
        unsafe {
            CloseHandle(job);
        }
    }

    struct AppJob(Handle);
    // l'handle serve solo ad assegnare processi; resta aperto finché vive l'app
    unsafe impl Send for AppJob {}
    unsafe impl Sync for AppJob {}

    /// Job dell'app, creato una volta e mai chiuso: lo chiude Windows quando l'app esce.
    pub fn assign(child: &std::process::Child) -> Result<(), String> {
        static JOB: OnceLock<Result<AppJob, String>> = OnceLock::new();
        let job = JOB.get_or_init(|| create().map(AppJob)).as_ref().map_err(Clone::clone)?;
        assign_to(job.0, child)
    }
}

pub struct LlamaEngine {
    data_dir: PathBuf,
    emitter: Emitter,
    state: Mutex<CleanupState>,
    port: AtomicU16,
    child: Mutex<Option<Child>>,
    generation: AtomicU64,
    client: reqwest::Client,
    pub restarts: AtomicU64,
    /// Tempi dell'ultima risposta (prompt_n, cache_n, predicted_n, predicted_per_second).
    pub last_timings: Mutex<Option<serde_json::Value>>,
    /// Un riscaldamento di ripresa (`rewarm`) in corso: non se ne lancia un secondo e la
    /// pulizia non usa gli slot fissi finché dura.
    warming: AtomicBool,
}

fn stopped() -> CleanupState {
    CleanupState { state: EngineStatus::Stopped, detail: None, model: None, vram_mb: None }
}

impl LlamaEngine {
    pub fn new(data_dir: PathBuf, emitter: Emitter) -> Arc<Self> {
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .timeout(REQUEST_TIMEOUT)
            .no_proxy()
            .build()
            .unwrap_or_default();
        Arc::new(LlamaEngine {
            data_dir,
            emitter,
            state: Mutex::new(stopped()),
            port: AtomicU16::new(0),
            child: Mutex::new(None),
            generation: AtomicU64::new(0),
            client,
            restarts: AtomicU64::new(0),
            last_timings: Mutex::new(None),
            warming: AtomicBool::new(false),
        })
    }

    pub fn state(&self) -> CleanupState {
        self.state.lock().unwrap().clone()
    }

    pub fn is_ready(&self) -> bool {
        self.state.lock().unwrap().state == EngineStatus::Ready && self.port.load(Ordering::SeqCst) != 0
    }

    pub fn port(&self) -> u16 {
        self.port.load(Ordering::SeqCst)
    }

    /// Vero mentre un riscaldamento di ripresa occupa uno slot.
    pub fn is_warming(&self) -> bool {
        self.warming.load(Ordering::SeqCst)
    }

    /// Riscaldamento di ripresa dopo una pausa lunga: lo stesso dell'avvio (prompt di sistema,
    /// `max_tokens: 1`, `cache_prompt`), slot per slot, in un thread a parte. Non blocca il
    /// chiamante; restituisce falso se il server non è pronto o se un riscaldamento è già in
    /// corso (al massimo uno alla volta). Una richiesta alla volta: l'altro slot resta libero
    /// per una pulizia vera, che va senza `id_slot` finché `is_warming` è vero.
    pub fn rewarm(self: &Arc<Self>, system: String) -> bool {
        if !self.is_ready() {
            return false;
        }
        if self.warming.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_err() {
            return false;
        }
        let me = Arc::clone(self);
        let spawned = std::thread::Builder::new().name("cleanup-rewarm".into()).spawn(move || {
            struct Done<'a>(&'a AtomicBool);
            impl Drop for Done<'_> {
                fn drop(&mut self) {
                    self.0.store(false, Ordering::SeqCst);
                }
            }
            let _done = Done(&me.warming);
            let port = me.port();
            let t = Instant::now();
            for slot in 0..SLOTS {
                // il server è ripartito o si è fermato nel frattempo: il supervisore riscalda già
                if port == 0 || me.port() != port {
                    return;
                }
                let ts = Instant::now();
                match warmup(port, &system, slot) {
                    Ok(()) => log::info!("llama-server: riscaldamento di ripresa dello slot {slot} in {:?}", ts.elapsed()),
                    Err(e) => log::warn!("llama-server: riscaldamento di ripresa dello slot {slot} fallito: {e}"),
                }
            }
            log::info!("llama-server: riscaldamento di ripresa finito in {:?}", t.elapsed());
        });
        if spawned.is_err() {
            self.warming.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }

    fn set_state(&self, state: EngineStatus, detail: Option<String>, model: Option<String>, vram_mb: Option<u32>) {
        let st = CleanupState { state, detail, model, vram_mb };
        *self.state.lock().unwrap() = st.clone();
        (self.emitter)(EVENT_ENGINE_STATE, serde_json::to_value(&st).unwrap_or_default());
    }

    fn pid_file(&self) -> PathBuf {
        self.data_dir.join("llama-server.pid")
    }

    /// Avvia (o riavvia) il server in un thread supervisore.
    pub fn start(self: &Arc<Self>, bin_dir: &str, model_path: &str, warm_system: String) {
        let gen = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        self.kill_child();
        let me = Arc::clone(self);
        let bin = PathBuf::from(bin_dir);
        let model = PathBuf::from(model_path);
        std::thread::Builder::new()
            .name("cleanup-llama".into())
            .spawn(move || me.supervise(gen, bin, model, warm_system))
            .expect("thread del supervisore di llama-server");
    }

    /// Prove: usa un llama-server già avviato da un altro processo (quello dell'app) senza
    /// possederlo: niente figlio né file pid, `stop` non lo chiude.
    #[cfg(test)]
    pub fn attach(&self, port: u16) {
        self.port.store(port, Ordering::SeqCst);
        self.set_state(EngineStatus::Ready, Some(format!("agganciato alla porta {port}")), None, None);
    }

    /// Ferma il server e il supervisore.
    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.kill_child();
        let _ = std::fs::remove_file(self.pid_file());
        if self.state.lock().unwrap().state != EngineStatus::Stopped {
            self.set_state(EngineStatus::Stopped, None, None, None);
        }
    }

    fn current(&self, gen: u64) -> bool {
        self.generation.load(Ordering::SeqCst) == gen
    }

    fn kill_child(&self) {
        self.port.store(0, Ordering::SeqCst);
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }

    /// Un `llama-server` rimasto da un'esecuzione precedente chiusa male occupa VRAM: si chiude.
    fn kill_orphan(&self) {
        let Ok(pid) = std::fs::read_to_string(self.pid_file()) else { return };
        let pid = pid.trim();
        if pid.is_empty() || !pid.chars().all(|c| c.is_ascii_digit()) {
            return;
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let out = Command::new("tasklist")
                .args(["/FI", &format!("PID eq {pid}"), "/FO", "CSV", "/NH"])
                .creation_flags(CREATE_NO_WINDOW)
                .output();
            if let Ok(o) = out {
                if String::from_utf8_lossy(&o.stdout).to_lowercase().contains("llama-server") {
                    log::info!("llama-server orfano (pid {pid}): lo chiudo");
                    let _ = Command::new("taskkill").args(["/PID", pid, "/F"]).creation_flags(CREATE_NO_WINDOW).output();
                }
            }
        }
        let _ = std::fs::remove_file(self.pid_file());
    }

    fn supervise(&self, gen: u64, bin_dir: PathBuf, model: PathBuf, warm_system: String) {
        self.kill_orphan();
        let model_name = model.file_stem().map(|s| s.to_string_lossy().into_owned());
        let mut failures: u32 = 0;
        while self.current(gen) {
            self.set_state(EngineStatus::Starting, None, model_name.clone(), None);
            match self.spawn_and_wait(gen, &bin_dir, &model) {
                Ok(port) => {
                    let t = Instant::now();
                    for slot in 0..SLOTS {
                        if let Err(e) = warmup(port, &warm_system, slot) {
                            log::warn!("llama-server: riscaldamento dello slot {slot} fallito: {e}");
                        }
                    }
                    log::info!("llama-server pronto sulla porta {port}, riscaldato in {:?}", t.elapsed());
                    self.port.store(port, Ordering::SeqCst);
                    self.set_state(EngineStatus::Ready, None, model_name.clone(), gpu_used_mb());
                    failures = 0;
                    // sorveglianza: se il processo esce, si riparte
                    loop {
                        std::thread::sleep(Duration::from_millis(500));
                        if !self.current(gen) {
                            return;
                        }
                        let exited = {
                            let mut g = self.child.lock().unwrap();
                            match g.as_mut().map(|c| c.try_wait()) {
                                Some(Ok(Some(status))) => Some(status.to_string()),
                                Some(Ok(None)) => None,
                                Some(Err(e)) => Some(e.to_string()),
                                None => Some("processo assente".into()),
                            }
                        };
                        if let Some(why) = exited {
                            log::warn!("llama-server è caduto ({why}): riavvio");
                            self.port.store(0, Ordering::SeqCst);
                            self.child.lock().unwrap().take();
                            self.restarts.fetch_add(1, Ordering::Relaxed);
                            self.set_state(EngineStatus::Error, Some(format!("crashed:{why}")), model_name.clone(), None);
                            break;
                        }
                    }
                }
                Err(e) => {
                    if !self.current(gen) {
                        return;
                    }
                    log::warn!("llama-server non parte: {e}");
                    self.kill_child();
                    failures += 1;
                    self.set_state(EngineStatus::Error, Some(e), model_name.clone(), None);
                }
            }
            // attesa crescente fra un tentativo e l'altro: 1, 2, 4 ... fino a 60 s
            let wait = Duration::from_secs(1u64 << failures.min(6)).min(Duration::from_secs(60));
            let until = Instant::now() + wait;
            while Instant::now() < until {
                if !self.current(gen) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }

    fn spawn_and_wait(&self, gen: u64, bin_dir: &Path, model: &Path) -> Result<u16, String> {
        let exe = bin_dir.join("llama-server.exe");
        if !exe.exists() {
            return Err(format!("missing:{}", exe.display()));
        }
        if !model.exists() {
            return Err(format!("missing:{}", model.display()));
        }
        let port = free_port().ok_or("nessuna porta libera")?;
        let log_dir = self.data_dir.join("logs");
        let _ = std::fs::create_dir_all(&log_dir);
        let log = std::fs::File::create(log_dir.join("llama-server.log")).map_err(|e| e.to_string())?;
        let log2 = log.try_clone().map_err(|e| e.to_string())?;
        let mut cmd = Command::new(&exe);
        cmd.args(["-m", &model.to_string_lossy(), "--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-ngl", "99", "-fa", "on", "-c", &(CONTEXT * SLOTS).to_string(), "-np", &SLOTS.to_string()])
            .args(["-rea", "off", "--reasoning-budget", "0"])
            .args(spec_args())
            .current_dir(bin_dir)
            .stdin(Stdio::null())
            .stdout(Stdio::from(log))
            .stderr(Stdio::from(log2));
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().map_err(|e| format!("spawn:{e}"))?;
        #[cfg(windows)]
        if let Err(e) = kill_on_close::assign(&child) {
            log::warn!("llama-server fuori dal job object (resterebbe orfano se l'app muore): {e}");
        }
        let _ = std::fs::write(self.pid_file(), child.id().to_string());
        *self.child.lock().unwrap() = Some(child);
        let t0 = Instant::now();
        loop {
            if !self.current(gen) {
                return Err("annullato".into());
            }
            if let Some(Ok(Some(st))) = self.child.lock().unwrap().as_mut().map(|c| c.try_wait()) {
                return Err(format!("exited:{st}"));
            }
            if http_status(port, "/health") == Some(200) {
                return Ok(port);
            }
            if t0.elapsed() > READY_TIMEOUT {
                return Err("timeout:avvio".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    /// Chiamata in streaming sull'endpoint chat. `on_partial` riceve il testo accumulato.
    pub async fn stream_chat(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
        on_partial: &mut (dyn FnMut(&str) + Send),
    ) -> anyhow::Result<String> {
        self.stream_chat_slot(system, user, max_tokens, None, on_partial).await
    }

    /// Come `stream_chat`, su uno slot preciso (così ogni slot riusa il proprio prefisso in cache).
    pub async fn stream_chat_slot(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
        slot: Option<u32>,
        on_partial: &mut (dyn FnMut(&str) + Send),
    ) -> anyhow::Result<String> {
        let port = self.port();
        if port == 0 {
            anyhow::bail!("llama-server non pronto");
        }
        let mut body = request_body(system, user, max_tokens);
        if let Some(sl) = slot {
            body["id_slot"] = serde_json::json!(sl);
        }
        let resp = self
            .client
            .post(format!("http://127.0.0.1:{port}/v1/chat/completions"))
            .json(&body)
            .send()
            .await?;
        if !resp.status().is_success() {
            let st = resp.status();
            let txt = resp.text().await.unwrap_or_default();
            anyhow::bail!("llama-server {st}: {}", txt.chars().take(300).collect::<String>());
        }
        let mut stream = resp.bytes_stream();
        let mut buf: Vec<u8> = Vec::new();
        let mut text = String::new();
        'outer: while let Some(chunk) = stream.next().await {
            buf.extend_from_slice(&chunk?);
            while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                let line: Vec<u8> = buf.drain(..=nl).collect();
                let line = String::from_utf8_lossy(&line);
                let line = line.trim();
                if line.contains("\"finish_reason\":\"length\"") {
                    anyhow::bail!("uscita troncata al tetto di {max_tokens} token");
                }
                if line.contains("\"timings\"") {
                    if let Some(t) = line
                        .strip_prefix("data:")
                        .and_then(|d| serde_json::from_str::<serde_json::Value>(d.trim()).ok())
                        .and_then(|v| v.get("timings").cloned())
                    {
                        *self.last_timings.lock().unwrap() = Some(t);
                    }
                }
                match parse_sse_line(line) {
                    SseLine::Delta(d) => {
                        text.push_str(&d);
                        on_partial(&text);
                    }
                    SseLine::Done => break 'outer,
                    SseLine::Error(e) => anyhow::bail!("llama-server: {e}"),
                    SseLine::Other => {}
                }
            }
        }
        Ok(text)
    }
}

impl LlamaEngine {
    /// Chiamata breve senza streaming con l'uscita vincolata a uno schema JSON (`response_format`
    /// di llama-server, decodifica guidata da grammatica). Oltre `timeout` si rinuncia: per il
    /// classificatore delle correzioni (tetto di 1,5 s) meglio nessun apprendimento che attendere.
    pub async fn chat_json(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
        schema: serde_json::Value,
        timeout: Duration,
    ) -> anyhow::Result<String> {
        let port = self.port();
        if port == 0 {
            anyhow::bail!("llama-server non pronto");
        }
        let mut body = request_body(system, user, max_tokens);
        body["stream"] = serde_json::json!(false);
        body["temperature"] = serde_json::json!(0.0);
        body["response_format"] = serde_json::json!({ "type": "json_object", "schema": schema });
        let call = async {
            let resp = self.client.post(format!("http://127.0.0.1:{port}/v1/chat/completions")).json(&body).send().await?;
            let st = resp.status();
            let v: serde_json::Value = resp.json().await?;
            if !st.is_success() {
                anyhow::bail!("llama-server {st}: {}", v.to_string().chars().take(300).collect::<String>());
            }
            Ok(v["choices"][0]["message"]["content"].as_str().unwrap_or_default().to_string())
        };
        match tokio::time::timeout(timeout, call).await {
            Ok(r) => r,
            Err(_) => anyhow::bail!("llama-server oltre {} ms", timeout.as_millis()),
        }
    }

    /// Token di un testo secondo il tokenizer del modello (`/tokenize`), per misurare i limiti.
    pub async fn count_tokens(&self, text: &str) -> anyhow::Result<usize> {
        let port = self.port();
        if port == 0 {
            anyhow::bail!("llama-server non pronto");
        }
        let v: serde_json::Value = self
            .client
            .post(format!("http://127.0.0.1:{port}/tokenize"))
            .json(&serde_json::json!({ "content": text }))
            .send()
            .await?
            .json()
            .await?;
        Ok(v["tokens"].as_array().map_or(0, |a| a.len()))
    }
}

pub fn request_body(system: &str, user: &str, max_tokens: u32) -> serde_json::Value {
    serde_json::json!({
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": user }
        ],
        "stream": true,
        "cache_prompt": true,
        "temperature": TEMPERATURE,
        "top_p": TOP_P,
        "max_tokens": max_tokens,
        "chat_template_kwargs": { "enable_thinking": false }
    })
}

#[derive(Debug, PartialEq)]
pub enum SseLine {
    Delta(String),
    Done,
    Error(String),
    Other,
}

/// Una riga dello stream OpenAI-compatibile di llama-server. Il `reasoning_content` (thinking)
/// si ignora.
pub fn parse_sse_line(line: &str) -> SseLine {
    let Some(data) = line.strip_prefix("data:") else { return SseLine::Other };
    let data = data.trim();
    if data == "[DONE]" {
        return SseLine::Done;
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data) else { return SseLine::Other };
    if let Some(e) = v.get("error") {
        return SseLine::Error(e.to_string());
    }
    let delta = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("delta"))
        .and_then(|d| d.get("content"))
        .and_then(|c| c.as_str());
    match delta {
        Some(d) if !d.is_empty() => SseLine::Delta(d.to_string()),
        _ => SseLine::Other,
    }
}

/// Argomenti in più per llama-server. Vuoto: la decodifica speculativa a n-grammi
/// (`--spec-type ngram-simple`, `ngram-mod`) è stata provata e non accorcia la pulizia.
/// `CLEANUP_LLAMA_EXTRA_ARGS` li
/// sostituisce per le prove ("none" = nessun argomento).
pub const SPEC_ARGS: &[&str] = &[];

pub fn spec_args() -> Vec<String> {
    match std::env::var("CLEANUP_LLAMA_EXTRA_ARGS") {
        Ok(v) if v.trim() == "none" => Vec::new(),
        Ok(v) if !v.trim().is_empty() => v.split_whitespace().map(|s| s.to_string()).collect(),
        _ => SPEC_ARGS.iter().map(|s| s.to_string()).collect(),
    }
}

fn free_port() -> Option<u16> {
    let l = TcpListener::bind("127.0.0.1:0").ok()?;
    l.local_addr().ok().map(|a| a.port())
}

fn http_raw(port: u16, request: &str, timeout: Duration) -> Option<String> {
    let mut s = TcpStream::connect_timeout(&([127, 0, 0, 1], port).into(), Duration::from_millis(500)).ok()?;
    s.set_read_timeout(Some(timeout)).ok()?;
    s.write_all(request.as_bytes()).ok()?;
    let mut out = Vec::new();
    s.read_to_end(&mut out).ok()?;
    Some(String::from_utf8_lossy(&out).into_owned())
}

fn http_status(port: u16, path: &str) -> Option<u16> {
    let r = http_raw(port, &format!("GET {path} HTTP/1.0\r\nHost: 127.0.0.1\r\n\r\n"), Duration::from_secs(2))?;
    r.split_whitespace().nth(1)?.parse().ok()
}

/// Riscaldamento: una richiesta con il prompt di sistema vero e un testo vuoto, un solo token.
/// Il prefisso resta nella cache del server (`cache_prompt`).
fn warmup(port: u16, system: &str, slot: u32) -> Result<(), String> {
    let body = serde_json::json!({
        "id_slot": slot,
        "messages": [
            { "role": "system", "content": system },
            { "role": "user", "content": "Trascrizione: \nTesto pulito:" }
        ],
        "stream": false,
        "cache_prompt": true,
        "max_tokens": 1,
        "chat_template_kwargs": { "enable_thinking": false }
    })
    .to_string();
    let req = format!(
        "POST /v1/chat/completions HTTP/1.0\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    let r = http_raw(port, &req, Duration::from_secs(60)).ok_or("nessuna risposta")?;
    if r.split_whitespace().nth(1) == Some("200") {
        Ok(())
    } else {
        Err(r.lines().next().unwrap_or("").to_string())
    }
}

/// VRAM occupata sulla GPU (tutta la scheda: `nvidia-smi` non dà il valore per processo).
pub fn gpu_used_mb() -> Option<u32> {
    let mut cmd = Command::new("nvidia-smi");
    cmd.args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let o = cmd.output().ok()?;
    String::from_utf8_lossy(&o.stdout).lines().next()?.trim().parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn righe_dello_stream() {
        assert_eq!(
            parse_sse_line(r#"data: {"choices":[{"delta":{"content":"Testo"}}]}"#),
            SseLine::Delta("Testo".into())
        );
        assert_eq!(parse_sse_line(r#"data: {"choices":[{"delta":{"reasoning_content":"penso"}}]}"#), SseLine::Other);
        assert_eq!(parse_sse_line("data: [DONE]"), SseLine::Done);
        assert_eq!(parse_sse_line(": ping"), SseLine::Other);
    }

    /// Un processo figlio qualsiasi nel job muore quando si chiude l'ultimo handle del job,
    /// cioè quello che fa Windows quando l'app muore.
    #[cfg(windows)]
    #[test]
    fn figlio_nel_job_muore_con_il_job() {
        use std::os::windows::process::CommandExt;
        let job = kill_on_close::create().unwrap();
        let mut child = Command::new("ping")
            .args(["-n", "30", "127.0.0.1"])
            .stdout(Stdio::null())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .unwrap();
        kill_on_close::assign_to(job, &child).unwrap();
        assert!(child.try_wait().unwrap().is_none(), "il figlio deve essere vivo");
        kill_on_close::close(job);
        let t = Instant::now();
        while child.try_wait().unwrap().is_none() {
            assert!(t.elapsed() < Duration::from_secs(5), "il figlio è sopravvissuto alla chiusura del job");
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Prova a mano: avvia llama-server e aspetta 120 s; nel frattempo `taskkill /F` del
    /// processo di test e `tasklist` non deve più mostrare llama-server.
    #[test]
    #[ignore]
    fn job_llama_prova_a_mano() {
        let dir = std::env::temp_dir().join("cleanup-job-prova");
        let eng = LlamaEngine::new(dir, crate::cleanup::null_emitter());
        let s = crate::cleanup::CleanupSettings::default();
        eng.start(&s.llama_bin_dir, &s.llama_model_path, "prova".into());
        let t = Instant::now();
        while !eng.is_ready() && t.elapsed() < Duration::from_secs(60) {
            std::thread::sleep(Duration::from_millis(100));
        }
        let pid = eng.child.lock().unwrap().as_ref().map(|c| c.id());
        eprintln!("PRONTO test pid {} llama-server pid {:?}", std::process::id(), pid);
        std::thread::sleep(Duration::from_secs(120));
    }

    /// Server finto: conta le richieste e risponde 200 solo quando `release` diventa vero.
    fn finto_server(count: Arc<std::sync::atomic::AtomicUsize>, release: Arc<AtomicBool>) -> u16 {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || {
            for conn in l.incoming() {
                let Ok(mut s) = conn else { continue };
                count.fetch_add(1, Ordering::SeqCst);
                // legge la richiesta intera (intestazione più corpo) prima di rispondere
                let mut buf = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    let n = s.read(&mut chunk).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    buf.extend_from_slice(&chunk[..n]);
                    let txt = String::from_utf8_lossy(&buf);
                    if let Some(h) = txt.find("\r\n\r\n") {
                        let len: usize = txt[..h]
                            .lines()
                            .find_map(|l| l.strip_prefix("Content-Length: "))
                            .and_then(|v| v.trim().parse().ok())
                            .unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            break;
                        }
                    }
                }
                while !release.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                let _ = s.write_all(b"HTTP/1.0 200 OK\r\nContent-Length: 2\r\n\r\n{}");
            }
        });
        port
    }

    #[test]
    fn riscaldamento_di_ripresa_uno_alla_volta() {
        use std::sync::atomic::AtomicUsize;
        let eng = LlamaEngine::new(std::env::temp_dir().join("cleanup-rewarm-prova"), crate::cleanup::null_emitter());
        // server non pronto: niente riscaldamento
        assert!(!eng.rewarm("sys".into()));
        let count = Arc::new(AtomicUsize::new(0));
        let release = Arc::new(AtomicBool::new(false));
        eng.attach(finto_server(Arc::clone(&count), Arc::clone(&release)));
        assert!(eng.rewarm("sys".into()), "il primo riscaldamento parte");
        let t = Instant::now();
        while count.load(Ordering::SeqCst) < 1 {
            assert!(t.elapsed() < Duration::from_secs(5), "la richiesta di riscaldamento non arriva");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(eng.is_warming());
        for _ in 0..5 {
            assert!(!eng.rewarm("sys".into()), "un secondo riscaldamento non deve partire");
        }
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(count.load(Ordering::SeqCst), 1, "una sola richiesta in volo, uno slot alla volta");
        release.store(true, Ordering::SeqCst);
        let t = Instant::now();
        while eng.is_warming() {
            assert!(t.elapsed() < Duration::from_secs(5), "il riscaldamento non finisce");
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(count.load(Ordering::SeqCst), SLOTS as usize, "una richiesta per slot");
        assert!(eng.rewarm("sys".into()), "finito il primo, se ne può lanciare un altro");
    }

    #[test]
    fn richiesta_senza_thinking_e_con_cache() {
        let b = request_body("sys", "utente", 100);
        assert_eq!(b["stream"], true);
        assert_eq!(b["cache_prompt"], true);
        assert_eq!(b["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(b["messages"][0]["content"], "sys");
        assert_eq!(b["messages"][1]["content"], "utente");
    }
}
