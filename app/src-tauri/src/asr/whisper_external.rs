//! Motore principale: `whisper-server.exe` di whisper.cpp (CUDA) come
//! processo figlio, modello residente, richieste HTTP locali per ogni pezzo.
//!
//! Comando: `whisper-server -m <turbo> --host 127.0.0.1
//! --port <libera> -l it -nt --vad -vm <silero>`; richiesta POST `/inference`
//! con il WAV del pezzo e i campi `prompt`, `language`, `vad`,
//! `response_format=json`, `temperature=0.0`. Per la dettatura `vad=false`
//! (voce bassa); `--vad` all'avvio resta, ma non si usa.
//!
//! Un thread supervisore avvia il server, aspetta che risponda (primo avvio
//! dopo un aggiornamento del driver: decine di secondi di compilazione per la
//! scheda), lo riscalda con una richiesta, poi lo sorveglia e lo riavvia se
//! cade. Lo stato resta `Starting` finché il server non risponde.

use std::fs::OpenOptions;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use log::{error, info, warn};

use super::http_locale::{free_port, get, post_multipart, wav_bytes};
use super::riscaldamento::{noise, RewarmGate};
use super::sessione::{transcribe_clip, DecodedPiece, LiveSession, PieceDecoder, SessionParams};
use super::{
    emit_state, AsrEngine, AsrEngineKind, AsrEvents, AsrSettings, AsrState, EngineStatus,
    RawTranscript, ScorerFactory, SessionConfig,
};

const SERVER_EXE: &str = "whisper-server.exe";
const READY_TIMEOUT: Duration = Duration::from_secs(600);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_LAUNCH_FAILURES: u32 = 5;

/// Job object di Windows con `JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`: l'handle
/// resta aperto per tutta la vita dell'app; quando il processo dell'app muore,
/// anche in crash, il sistema lo chiude e termina whisper-server.
///
/// Dichiarazioni FFI scritte qui perché `Cargo.toml` non
/// abilita la feature `Win32_System_JobObjects` della crate `windows`; le tre
/// funzioni stanno in kernel32.
#[cfg(windows)]
mod kill_on_close {
    use std::ffi::c_void;
    use std::os::windows::io::AsRawHandle;
    use std::sync::OnceLock;

    type Handle = *mut c_void;

    #[repr(C)]
    #[derive(Default)]
    struct BasicLimit {
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
    struct ExtendedLimit {
        basic: BasicLimit,
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
    }

    struct Job(Handle);
    // L'handle si usa solo per assegnare processi: nessuno stato condiviso.
    unsafe impl Send for Job {}
    unsafe impl Sync for Job {}

    fn job() -> Result<&'static Job, String> {
        static JOB: OnceLock<Result<Job, String>> = OnceLock::new();
        JOB.get_or_init(|| unsafe {
            let h = CreateJobObjectW(std::ptr::null_mut(), std::ptr::null());
            if h.is_null() {
                return Err(format!("CreateJobObjectW: {}", std::io::Error::last_os_error()));
            }
            let mut info = ExtendedLimit::default();
            info.basic.limit_flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let ok = SetInformationJobObject(
                h,
                JOB_OBJECT_EXTENDED_LIMIT_INFORMATION,
                &info as *const ExtendedLimit as *const c_void,
                std::mem::size_of::<ExtendedLimit>() as u32,
            );
            if ok == 0 {
                return Err(format!("SetInformationJobObject: {}", std::io::Error::last_os_error()));
            }
            Ok(Job(h))
        })
        .as_ref()
        .map_err(Clone::clone)
    }

    pub fn assign(child: &std::process::Child) -> Result<(), String> {
        let job = job()?;
        let ok = unsafe { AssignProcessToJobObject(job.0, child.as_raw_handle() as Handle) };
        if ok == 0 {
            return Err(format!("AssignProcessToJobObject: {}", std::io::Error::last_os_error()));
        }
        Ok(())
    }
}

struct Inner {
    settings: AsrSettings,
    events: Arc<dyn AsrEvents>,
    make_scorer: Option<ScorerFactory>,
    child: Mutex<Option<Child>>,
    port: AtomicU16,
    state: Mutex<AsrState>,
    session: Mutex<Option<LiveSession>>,
    /// Cambia a ogni riavvio: il supervisore vecchio si ferma.
    epoch: AtomicU64,
    stopping: AtomicBool,
    /// Richieste vere (pezzi di dettatura, clip) in corso sul server.
    in_flight: AtomicUsize,
    rewarm: RewarmGate,
}

impl Inner {
    fn set_state(&self, status: EngineStatus, detail: Option<String>) {
        let snapshot = {
            let mut s = self.state.lock().unwrap();
            s.state = status;
            s.detail = detail;
            s.port = match self.port.load(Ordering::Acquire) {
                0 => None,
                p => Some(p),
            };
            s.clone()
        };
        emit_state(self.events.as_ref(), &snapshot);
    }

    fn current(&self, epoch: u64) -> bool {
        !self.stopping.load(Ordering::Acquire) && self.epoch.load(Ordering::Acquire) == epoch
    }

    fn kill_child(&self) {
        if let Some(mut c) = self.child.lock().unwrap().take() {
            let _ = c.kill();
            let _ = c.wait();
        }
        self.port.store(0, Ordering::Release);
    }

    fn launch(&self) -> Result<u16> {
        let bin = Path::new(&self.settings.whisper_bin_dir).join(SERVER_EXE);
        let model = PathBuf::from(&self.settings.whisper_model_path);
        let vad = PathBuf::from(&self.settings.whisper_vad_model_path);
        for (what, p) in [("server", &bin), ("modello", &model), ("modello VAD", &vad)] {
            if !p.is_file() {
                bail!("{what} non trovato: {}", p.display());
            }
        }
        let port = free_port()?;
        let log_path = std::env::temp_dir().join("eaglevoice-whisper-server.log");
        let log = OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&log_path)
            .context("log di whisper-server")?;
        let mut cmd = Command::new(&bin);
        cmd.arg("-m")
            .arg(&model)
            .args(["--host", "127.0.0.1", "--port", &port.to_string()])
            .args(["-l", &self.settings.primary_language, "-nt", "--vad", "-vm"])
            .arg(&vad)
            .current_dir(&self.settings.whisper_bin_dir)
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            cmd.creation_flags(CREATE_NO_WINDOW);
        }
        let child = cmd.spawn().with_context(|| format!("avvio di {}", bin.display()))?;
        #[cfg(windows)]
        if let Err(e) = kill_on_close::assign(&child) {
            warn!("asr: whisper-server fuori dal Job object ({e}): se l'app cade resta acceso");
        }
        info!(
            "asr: whisper-server avviato (pid {}, porta {port}, log {})",
            child.id(),
            log_path.display()
        );
        *self.child.lock().unwrap() = Some(child);
        self.port.store(port, Ordering::Release);
        Ok(port)
    }

    /// `Some(codice)` se il processo è uscito.
    fn exited(&self) -> Option<String> {
        let mut g = self.child.lock().unwrap();
        match g.as_mut().map(|c| c.try_wait()) {
            None => Some("processo assente".into()),
            Some(Ok(Some(status))) => Some(format!("{status}")),
            Some(Ok(None)) => None,
            Some(Err(e)) => Some(e.to_string()),
        }
    }

    fn wait_ready(&self, port: u16, epoch: u64) -> Result<()> {
        let t0 = Instant::now();
        loop {
            if !self.current(epoch) {
                bail!("avvio interrotto");
            }
            if let Some(code) = self.exited() {
                bail!("whisper-server uscito durante l'avvio ({code})");
            }
            if get(port, "/", Duration::from_secs(1)).is_ok() {
                info!("asr: whisper-server pronto in {:.1} s", t0.elapsed().as_secs_f32());
                return Ok(());
            }
            if t0.elapsed() > READY_TIMEOUT {
                bail!("whisper-server non risponde dopo {} s", READY_TIMEOUT.as_secs());
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    }

    /// `vad` è il VAD interno di whisper-server: spento per il riscaldamento
    /// (col VAD acceso il rumore viene scartato e l'encoder sulla GPU non
    /// gira) e per la dettatura (`DICTATION_SERVER_VAD`).
    fn decode_on(
        &self,
        port: u16,
        samples: &[f32],
        prompt: &str,
        language: Option<&str>,
        vad: bool,
    ) -> Result<DecodedPiece> {
        let wav = wav_bytes(samples)?;
        let lang = language.unwrap_or("auto");
        let format = if language.is_none() { "verbose_json" } else { "json" };
        let mut fields: Vec<(&str, &str)> = vec![
            ("response_format", format),
            ("temperature", "0.0"),
            ("language", lang),
            ("vad", if vad { "true" } else { "false" }),
        ];
        if !prompt.is_empty() {
            fields.push(("prompt", prompt));
        }
        let t0 = Instant::now();
        let (status, body) = post_multipart(port, "/inference", &fields, "pezzo.wav", &wav, REQUEST_TIMEOUT)?;
        let elapsed = t0.elapsed().as_millis();
        if status != 200 {
            bail!(
                "whisper-server ha risposto {status}: {}",
                String::from_utf8_lossy(&body).chars().take(200).collect::<String>()
            );
        }
        let j: serde_json::Value = serde_json::from_slice(&body).context("JSON di whisper-server")?;
        if let Some(err) = j.get("error") {
            bail!("whisper-server: {err}");
        }
        let text = j
            .get("text")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        // verbose_json: `detected_language` e `language` portano il nome lungo
        // ("english"); la sessione lo riconduce al codice ISO.
        let language = ["detected_language", "language"]
            .iter()
            .find_map(|k| j.get(*k).and_then(|l| l.as_str()))
            .map(str::to_string);
        info!(
            "asr: richiesta /inference {:.1} s di audio in {elapsed} ms",
            samples.len() as f32 / 16_000.0
        );
        Ok(DecodedPiece { text, language })
    }

    fn vram_used_mb() -> Option<u32> {
        let mut cmd = Command::new("nvidia-smi");
        cmd.args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"]);
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let out = cmd.output().ok()?;
        String::from_utf8_lossy(&out.stdout).lines().next()?.trim().parse().ok()
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.kill_child();
    }
}

/// Campo `vad` delle richieste della dettatura (il riscaldamento lo manda
/// sempre spento).
const DICTATION_SERVER_VAD: bool = false;

struct ServerDecoder(Arc<Inner>);

/// Tiene `in_flight` alto finché vive: scende in ogni uscita, panic compreso,
/// così `rewarm` non resta spento.
struct InFlightGuard<'a>(&'a AtomicUsize);

impl<'a> InFlightGuard<'a> {
    fn new(counter: &'a AtomicUsize) -> Self {
        counter.fetch_add(1, Ordering::AcqRel);
        Self(counter)
    }
}

impl Drop for InFlightGuard<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

impl PieceDecoder for ServerDecoder {
    fn decode(&self, samples: &[f32], prompt: &str, language: Option<&str>) -> Result<DecodedPiece> {
        let port = self.0.port.load(Ordering::Acquire);
        if port == 0 || self.0.state.lock().unwrap().state != EngineStatus::Ready {
            return Err(anyhow!("whisper-server non pronto"));
        }
        let _guard = InFlightGuard::new(&self.0.in_flight);
        // VAD interno spento anche per la dettatura: sulla voce bassa
        // tagliava parlato vero e il grezzo usciva peggiore. Contro il silenzio restano il
        // VAD di cattura, la soglia di parlato dei pezzi e
        // `filtri::strip_unsupported_closing`.
        self.0.decode_on(port, samples, prompt, language, DICTATION_SERVER_VAD)
    }
}

fn supervise(inner: Arc<Inner>, epoch: u64) {
    let mut failures = 0u32;
    while inner.current(epoch) {
        inner.set_state(EngineStatus::Starting, Some("avvio di whisper-server".into()));
        let outcome = inner.launch().and_then(|port| {
            inner.wait_ready(port, epoch)?;
            // Riscaldamento: la prima richiesta paga l'inizializzazione di CUDA.
            // VAD spento, altrimenti il rumore viene scartato (GPU ferma).
            let t0 = Instant::now();
            if let Err(e) = inner.decode_on(port, &noise(), "", Some("it"), false) {
                warn!("asr: riscaldamento fallito: {e}");
            }
            info!("asr: riscaldamento in {} ms", t0.elapsed().as_millis());
            Ok(port)
        });
        match outcome {
            Ok(_) if inner.current(epoch) => {
                failures = 0;
                inner.state.lock().unwrap().vram_mb = Inner::vram_used_mb();
                inner.set_state(EngineStatus::Ready, None);
                // Sorveglianza: il processo è ancora vivo?
                loop {
                    std::thread::sleep(Duration::from_secs(1));
                    if !inner.current(epoch) {
                        return;
                    }
                    if let Some(code) = inner.exited() {
                        error!("asr: whisper-server è caduto ({code}), riavvio");
                        inner.kill_child();
                        inner.set_state(EngineStatus::Error, Some(format!("whisper-server è caduto ({code})")));
                        break;
                    }
                }
            }
            Ok(_) => return,
            Err(e) => {
                if !inner.current(epoch) {
                    return;
                }
                failures += 1;
                error!("asr: whisper-server non parte ({e}), tentativo {failures}");
                inner.kill_child();
                inner.set_state(EngineStatus::Error, Some(e.to_string()));
                if failures >= MAX_LAUNCH_FAILURES {
                    return;
                }
            }
        }
        let wait = Duration::from_secs(2 * failures.max(1) as u64);
        let t0 = Instant::now();
        while t0.elapsed() < wait {
            if !inner.current(epoch) {
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

pub struct WhisperExternal {
    inner: Arc<Inner>,
}

impl WhisperExternal {
    /// Crea il motore e avvia il server in un thread: non blocca.
    pub fn start(
        settings: AsrSettings,
        events: Arc<dyn AsrEvents>,
        make_scorer: Option<ScorerFactory>,
    ) -> Arc<Self> {
        let model = Path::new(&settings.whisper_model_path)
            .file_name()
            .map(|f| f.to_string_lossy().to_string());
        let inner = Arc::new(Inner {
            settings,
            events,
            make_scorer,
            child: Mutex::new(None),
            port: AtomicU16::new(0),
            state: Mutex::new(AsrState {
                state: EngineStatus::Stopped,
                model,
                engine: AsrEngineKind::WhisperExternal,
                ..Default::default()
            }),
            session: Mutex::new(None),
            epoch: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            rewarm: RewarmGate::default(),
        });
        let me = Arc::new(Self { inner });
        me.spawn_supervisor();
        me
    }

    fn spawn_supervisor(&self) {
        let epoch = self.inner.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        let inner = Arc::clone(&self.inner);
        std::thread::Builder::new()
            .name("asr-whisper-server".into())
            .spawn(move || supervise(inner, epoch))
            .expect("thread supervisore asr");
    }

    /// Aspetta che il server sia pronto (prove e CLI).
    pub fn wait_until_ready(&self, timeout: Duration) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < timeout {
            if self.is_ready() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        false
    }

    fn decoder(&self) -> Arc<dyn PieceDecoder> {
        Arc::new(ServerDecoder(Arc::clone(&self.inner)))
    }
}

impl AsrEngine for WhisperExternal {
    fn start_session(&self, cfg: SessionConfig) -> Result<()> {
        let params = SessionParams::new(&self.inner.settings, &cfg);
        let session = LiveSession::start(
            params,
            self.decoder(),
            Arc::clone(&self.inner.events),
            self.inner.make_scorer.clone(),
        );
        if let Some(old) = self.inner.session.lock().unwrap().replace(session) {
            old.cancel();
        }
        Ok(())
    }

    fn feed(&self, samples: &[f32]) {
        if let Some(s) = self.inner.session.lock().unwrap().as_ref() {
            s.feed(samples);
        }
    }

    fn finish(&self) -> Result<RawTranscript> {
        let session = self
            .inner
            .session
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow!("nessuna sessione asr aperta"))?;
        let t0 = Instant::now();
        let out = session.finish();
        info!("asr: finish in {} ms", t0.elapsed().as_millis());
        out
    }

    fn cancel(&self) {
        if let Some(s) = self.inner.session.lock().unwrap().take() {
            s.cancel();
        }
    }

    fn add_session_vocabulary(&self, extra: Vec<String>) {
        if let Some(s) = self.inner.session.lock().unwrap().as_ref() {
            s.add_vocabulary(extra);
        }
    }

    fn is_ready(&self) -> bool {
        self.inner.state.lock().unwrap().state == EngineStatus::Ready
    }

    fn state(&self) -> AsrState {
        self.inner.state.lock().unwrap().clone()
    }

    fn transcribe_clip(&self, samples: Vec<f32>, cfg: SessionConfig) -> Result<RawTranscript> {
        let params = SessionParams::new(&self.inner.settings, &cfg);
        transcribe_clip(
            params,
            self.decoder(),
            Arc::clone(&self.inner.events),
            self.inner.make_scorer.clone(),
            &samples,
        )
    }

    fn rewarm(&self) -> bool {
        let port = self.inner.port.load(Ordering::Acquire);
        if port == 0 || !self.is_ready() {
            return false;
        }
        // Una richiesta vera in corso scalda già la GPU; aggiungerne una
        // allungherebbe la coda del server (una richiesta alla volta).
        if self.inner.in_flight.load(Ordering::Acquire) > 0 {
            return false;
        }
        let inner = Arc::clone(&self.inner);
        let started = self.inner.rewarm.try_spawn(move || {
            let t0 = Instant::now();
            match inner.decode_on(port, &noise(), "", Some("it"), false) {
                Ok(_) => info!("asr: riscaldamento dopo pausa in {} ms", t0.elapsed().as_millis()),
                Err(e) => warn!("asr: riscaldamento dopo pausa fallito: {e}"),
            }
        });
        if !started {
            info!("asr: riscaldamento già in corso, saltato");
        }
        started
    }

    fn restart(&self) -> Result<()> {
        self.inner.epoch.fetch_add(1, Ordering::AcqRel);
        self.inner.kill_child();
        self.inner.set_state(EngineStatus::Starting, Some("riavvio".into()));
        self.spawn_supervisor();
        Ok(())
    }

    fn shutdown(&self) {
        self.inner.stopping.store(true, Ordering::Release);
        self.inner.kill_child();
        self.inner.set_state(EngineStatus::Stopped, None);
    }
}

#[cfg(test)]
mod tests {
    //! Prove con il server vero, ignorate di default:
    //! `cargo test -- --ignored asr_ --nocapture --test-threads=1`.
    use super::*;
    use crate::asr::NoEvents;
    use crate::audio_toolkit::read_wav_samples;

    /// Figlio della prova sotto: avvia il server, stampa il pid e aspetta di
    /// essere ucciso. Non fa nulla senza `ASR_JOB_CHILD=1`.
    #[test]
    #[ignore]
    fn asr_job_figlio() {
        if std::env::var("ASR_JOB_CHILD").as_deref() != Ok("1") {
            return;
        }
        let e = WhisperExternal::start(AsrSettings::default(), Arc::new(NoEvents), None);
        loop {
            if let Some(c) = e.inner.child.lock().unwrap().as_ref() {
                println!("SERVER_PID={}", c.id());
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        std::thread::sleep(Duration::from_secs(120));
    }

    fn pid_alive(pid: u32) -> bool {
        let out = Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).contains("whisper-server")
    }

    /// Il test figlio viene terminato con forza: whisper-server deve sparire.
    #[test]
    #[ignore]
    fn asr_job_server_muore_con_l_app() {
        use std::io::{BufRead, BufReader};
        let exe = std::env::current_exe().unwrap();
        let mut child = Command::new(exe)
            .args(["asr_job_figlio", "--ignored", "--exact", "asr::whisper_external::tests::asr_job_figlio", "--nocapture"])
            .env("ASR_JOB_CHILD", "1")
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let mut pid = None;
        for line in BufReader::new(child.stdout.take().unwrap()).lines() {
            let line = line.unwrap();
            if let Some(p) = line.split("SERVER_PID=").nth(1) {
                pid = Some(p.trim().parse::<u32>().unwrap());
                break;
            }
        }
        let pid = pid.expect("pid del server");
        std::thread::sleep(Duration::from_secs(2));
        assert!(pid_alive(pid), "whisper-server {pid} non partito");
        println!("tasklist prima: whisper-server {pid} vivo");
        let kill = Command::new("taskkill")
            .args(["/F", "/PID", &child.id().to_string()])
            .output()
            .unwrap();
        println!("taskkill: {}", String::from_utf8_lossy(&kill.stdout).trim());
        let _ = child.wait();
        let t0 = Instant::now();
        while pid_alive(pid) && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(200));
        }
        let alive = pid_alive(pid);
        println!("tasklist dopo {} ms: whisper-server {pid} vivo = {alive}", t0.elapsed().as_millis());
        assert!(!alive, "whisper-server orfano");
    }

    /// Rilevamento contro un whisper-server già attivo (quello dell'app),
    /// senza avviarne uno: `ASR_LIVE_PORT` e `ASR_LIVE_WAV` (16 kHz mono),
    /// `ASR_LIVE_EXPECT` è il codice atteso.
    #[test]
    #[ignore]
    fn asr_lingua_dal_server_vivo() {
        let port: u16 = std::env::var("ASR_LIVE_PORT").unwrap().parse().unwrap();
        let samples = read_wav_samples(std::env::var("ASR_LIVE_WAV").unwrap()).unwrap();
        let expect = std::env::var("ASR_LIVE_EXPECT").unwrap();
        let inner = Inner {
            settings: AsrSettings::default(),
            events: Arc::new(NoEvents),
            make_scorer: None,
            child: Mutex::new(None),
            port: AtomicU16::new(port),
            state: Mutex::new(AsrState::default()),
            session: Mutex::new(None),
            epoch: AtomicU64::new(0),
            stopping: AtomicBool::new(false),
            in_flight: AtomicUsize::new(0),
            rewarm: RewarmGate::default(),
        };
        let d = inner.decode_on(port, &samples, "", None, true).unwrap();
        let code = d.language.as_deref().and_then(crate::asr::sessione::language_code);
        println!("server: {:?} -> codice {:?}; testo: {}", d.language, code, d.text);
        assert_eq!(code.as_deref(), Some(expect.as_str()));
    }
}
