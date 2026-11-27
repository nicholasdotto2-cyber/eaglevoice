//! Appunti per l'incolla con ricevuta del solo bersaglio.
//!
//! Perché non `paste_tx::publish_standalone`: lì ogni `WM_RENDERFORMAT` vale
//! come lettura, di chiunque. Programmi come AnyDesk, TeamViewer e Chrome
//! Remote Desktop leggono gli appunti appena cambiano: la
//! ricevuta arrivava a 0 ms, prima del Ctrl+V, e l'incolla risultava riuscito
//! con il campo invariato. In più, con il rendering
//! ritardato la prima lettura consuma la promessa: la lettura vera del
//! bersaglio, dopo, non manda più nessun `WM_RENDERFORMAT`.
//!
//! Qui in `WM_RENDERFORMAT` il lettore si riconosce dal processo della
//! finestra che ha aperto gli appunti (`GetOpenClipboardWindow`):
//! - è il bersaglio (stesso PID della finestra o del processo del contesto,
//!   diversi per le app UWP, oppure stesso nome dell'exe) e il Ctrl+V è
//!   partito: si scrive il testo, ricevuta certa;
//! - è un altro processo riconosciuto (es. `remote_assistance_host.exe` di
//!   Chrome Remote Desktop), o il bersaglio prima del Ctrl+V: non si scrive
//!   nulla, la promessa resta armata, il lettore va nel log con l'exe;
//! - finestra assente (lettore sconosciuto): si scrive il testo ma non vale
//!   come ricevuta. Visto dal vivo (`live_lettori_appunti`): Edge, e
//!   quindi Chromium ed Electron (Claude, Code, ChatGPT, Comet), apre gli
//!   appunti senza finestra, come un altro programma del PC che, se riceve
//!   appunti vuoti, riprova ogni 30 ms per circa 300 ms tenendoli occupati.
//!   PID e nome dell'exe qui non si possono leggere, e l'albero dei processi
//!   non aiuterebbe: manca proprio il processo di partenza.
//!   Per questi casi l'incolla è riuscito solo se il campo, riletto via UI
//!   Automation, mostra il testo prima del cursore (`wait_outcome`).
//!
//! Ripristino degli appunti di prima: con la stessa istantanea di `paste_tx`,
//! solo se gli appunti sono ancora nostri (numero di sequenza), poco dopo la
//! ricevuta o allo scadere dell'attesa.

use std::sync::atomic::{AtomicBool, AtomicIsize, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, Once};
use std::time::{Duration, Instant};

use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GlobalFree, SetLastError, ERROR_SUCCESS, HANDLE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardSequenceNumber, GetOpenClipboardWindow,
    OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, PostMessageW, PostQuitMessage, RegisterClassW, SetWindowLongPtrW,
    GWLP_USERDATA, HWND_MESSAGE, MSG, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE, WM_DESTROY,
    WM_DESTROYCLIPBOARD, WM_RENDERALLFORMATS, WM_RENDERFORMAT, WNDCLASSW,
};

use super::category::exe_file_name;
use super::context::{hwnd_from, window_info};
use crate::paste_tx::windows::{
    open_clipboard_retry, restore_clipboard_snapshot, take_clipboard_snapshot, ClipboardSnapshot,
};

const CLASS_NAME: PCWSTR = w!("EagleVoiceTargetClipboard");
/// Dopo la ricevuta del bersaglio, attesa prima di rimettere gli appunti di prima.
const RESTORE_AFTER_RECEIPT: Duration = Duration::from_millis(250);

/// Il bersaglio dell'incolla, come lo legge il contesto.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Target {
    /// PID dei processi che valgono come bersaglio (finestra e app, diversi per UWP).
    pub pids: Vec<u32>,
    /// Nome dell'exe, minuscolo (es. "claude.exe").
    pub exe: String,
}

impl Target {
    pub fn from_window(hwnd: isize, ctx_pid: u32, ctx_exe: &str) -> Self {
        let mut pids = Vec::new();
        if ctx_pid != 0 {
            pids.push(ctx_pid);
        }
        if hwnd != 0 {
            let mut wpid = 0u32;
            unsafe {
                windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(hwnd_from(hwnd), Some(&mut wpid));
            }
            if wpid != 0 && !pids.contains(&wpid) {
                pids.push(wpid);
            }
        }
        Self { pids, exe: exe_file_name(ctx_exe).to_lowercase() }
    }

    /// Nessun bersaglio noto: vale qualunque lettore dopo il Ctrl+V (come prima).
    pub fn is_unknown(&self) -> bool {
        self.pids.is_empty() && self.exe.is_empty()
    }
}

/// Chi ha aperto gli appunti per leggere.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub pid: u32,
    pub exe: String,
}

/// Decisione pura: questa lettura è del bersaglio?
pub fn is_target_reader(target: &Target, reader: Option<&Reader>) -> bool {
    if target.is_unknown() {
        return true;
    }
    let Some(r) = reader else {
        return false; // finestra di lettura assente: non si sa chi è
    };
    if r.pid != 0 && target.pids.contains(&r.pid) {
        return true;
    }
    !target.exe.is_empty() && exe_file_name(&r.exe).to_lowercase() == target.exe
}

struct Shared {
    text: String,
    target: Target,
    own_pid: u32,
    injected: Mutex<Option<Instant>>,
    receipt: Mutex<Option<Instant>>,
    /// Prima lettura di un lettore sconosciuto dopo il Ctrl+V.
    unknown_read: Mutex<Option<Instant>>,
    /// Ultima lettura di chiunque (per aspettare che i monitor finiscano).
    last_read: Mutex<Option<Instant>>,
    /// Letture rifiutate: (exe, ms dalla pubblicazione).
    refused: Mutex<Vec<(String, u64)>>,
    published_at: Instant,
    sequence: AtomicU32,
    ownership_lost: AtomicBool,
    settled: AtomicBool,
    hwnd: AtomicIsize,
    snapshot: Mutex<Option<ClipboardSnapshot>>,
}

/// Transazione in corso (una alla volta: la nuova chiude la vecchia prima
/// dell'istantanea, così l'istantanea vede gli appunti dell'utente).
static PENDING: Mutex<Option<Arc<Shared>>> = Mutex::new(None);

/// Tutti i lettori dell'ultima transazione ("exe (pid N) a M ms"), per la diagnostica.
static LAST_READERS: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn last_readers() -> Vec<String> {
    LAST_READERS.lock().map(|v| v.clone()).unwrap_or_default()
}

/// Come è finito l'incolla.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOutcome {
    /// Il bersaglio ha letto (riconosciuto dalla finestra): ms dal Ctrl+V.
    TargetRead(Duration),
    /// Lettore sconosciuto e testo ritrovato nel campo: ms dal Ctrl+V alla verifica.
    FoundInField(Duration),
    /// Lettore sconosciuto e campo illeggibile: vale la lettura, non verificata.
    Unverifiable(Duration),
    /// Lettore sconosciuto, campo leggibile ma testo non ritrovato entro la scadenza.
    NotInField,
    /// Nessuna lettura valida.
    NotRead,
}

/// Controllo del campo dopo una lettura senza finestra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldCheck {
    /// Il testo c'è (o il campo è cresciuto in modo coerente).
    Found,
    /// Campo leggibile, testo non ancora visto: si aspetta.
    Pending,
    /// Campo non leggibile (prima o adesso): non si può verificare.
    Unreadable,
}

pub struct TargetClipboard {
    shared: Arc<Shared>,
}

unsafe fn render(text: &str) {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2) else { return };
    let ptr = GlobalLock(hg) as *mut u16;
    if ptr.is_null() {
        let _ = GlobalFree(Some(hg));
        return;
    }
    std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
    let _ = GlobalUnlock(hg);
    if SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hg.0))).is_err() {
        let _ = GlobalFree(Some(hg));
    }
}

/// Gli appunti non sono aperti da nessuno in questo momento.
fn clipboard_free() -> bool {
    unsafe {
        if OpenClipboard(None).is_ok() {
            let _ = CloseClipboard();
            true
        } else {
            false
        }
    }
}

fn reader_now() -> Option<Reader> {
    let hwnd = unsafe { GetOpenClipboardWindow() }.ok()?;
    if hwnd.0.is_null() {
        return None;
    }
    let info = window_info(hwnd.0 as isize);
    Some(Reader { pid: info.pid, exe: exe_file_name(&info.exe_path).to_string() })
}

unsafe extern "system" fn wnd_proc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const Shared;
    match msg {
        WM_RENDERFORMAT if !ptr.is_null() && wp.0 as u32 == CF_UNICODETEXT.0 as u32 => {
            let s = &*ptr;
            let reader = reader_now();
            let injected = s.injected.lock().ok().and_then(|g| *g);
            let ms = s.published_at.elapsed().as_millis() as u64;
            // Il nostro processo (es. la webview dell'app) non è un bersaglio,
            // salvo che lo sia davvero (le finestre delle prove).
            let own = reader.as_ref().is_some_and(|r| r.pid == s.own_pid)
                && !s.target.pids.contains(&s.own_pid);
            let who = reader
                .as_ref()
                .map(|r| format!("{} (pid {})", r.exe, r.pid))
                .unwrap_or_else(|| "sconosciuto (nessuna finestra)".into());
            if let Ok(mut l) = s.last_read.lock() {
                *l = Some(Instant::now());
            }
            if let Ok(mut all) = LAST_READERS.lock() {
                all.push(format!("{who} a {ms} ms"));
            }
            if reader.is_none() {
                // Lettore sconosciuto: testo sì (può essere il bersaglio
                // Chromium/Electron), ricevuta no.
                // Da qui la promessa è consumata: una lettura successiva del
                // bersaglio non manda più `WM_RENDERFORMAT`, l'esito lo dà il campo.
                render(&s.text);
                if let Ok(mut u) = s.unknown_read.lock() {
                    u.get_or_insert(Instant::now());
                }
            } else if injected.is_some() && !own && is_target_reader(&s.target, reader.as_ref()) {
                render(&s.text);
                if let Ok(mut r) = s.receipt.lock() {
                    r.get_or_insert(Instant::now());
                }
            } else {
                log::info!(
                    "[win] appunti letti da {who} a {ms} ms, {}: non vale come incolla",
                    if injected.is_some() { "non è il bersaglio" } else { "prima del Ctrl+V" }
                );
                if let Ok(mut v) = s.refused.lock() {
                    v.push((who, ms));
                }
            }
            LRESULT(0)
        }
        WM_RENDERFORMAT => LRESULT(0),
        // Finestra distrutta con la promessa ancora negli appunti: non si
        // scrive nulla (nessuno ha letto davvero).
        WM_RENDERALLFORMATS => LRESULT(0),
        WM_DESTROYCLIPBOARD => {
            if !ptr.is_null() {
                (*ptr).ownership_lost.store(true, Ordering::SeqCst);
            }
            LRESULT(0)
        }
        WM_CLOSE => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wp, lp),
    }
}

/// Marcatori che tengono il testo fuori dalla cronologia degli appunti (Win+V),
/// dalla sincronizzazione cloud e dai programmi che sorvegliano gli appunti.
/// Gli appunti devono essere già aperti e svuotati.
unsafe fn set_exclusion_markers() {
    for (name, value) in [
        (w!("ExcludeClipboardContentFromMonitorProcessing"), 1u32),
        (w!("CanIncludeInClipboardHistory"), 0u32),
        (w!("CanUploadToCloudClipboard"), 0u32),
    ] {
        let format = RegisterClipboardFormatW(name);
        if format == 0 {
            continue;
        }
        if let Ok(hg) = GlobalAlloc(GMEM_MOVEABLE, 4) {
            let p = GlobalLock(hg) as *mut u32;
            if p.is_null() {
                let _ = GlobalFree(Some(hg));
                continue;
            }
            *p = value;
            let _ = GlobalUnlock(hg);
            if SetClipboardData(format, Some(HANDLE(hg.0))).is_err() {
                let _ = GlobalFree(Some(hg));
            }
        }
    }
}

/// Copia esplicita (menu del tray, pulsanti Copia dell'app): il testo va negli
/// appunti subito, senza rendering ritardato, con gli stessi marcatori di
/// esclusione dell'incolla.
pub(crate) fn write_protected_text(text: &str) -> Result<(), String> {
    unsafe {
        open_clipboard_retry(None)?;
        let result = (|| {
            EmptyClipboard().map_err(|e| format!("EmptyClipboard: {e}"))?;
            set_exclusion_markers();
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let hg = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2)
                .map_err(|e| format!("GlobalAlloc: {e}"))?;
            let p = GlobalLock(hg) as *mut u16;
            if p.is_null() {
                let _ = GlobalFree(Some(hg));
                return Err("GlobalLock fallito".to_string());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(hg);
            // Se riesce, SetClipboardData diventa proprietario della memoria.
            if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(hg.0))) {
                let _ = GlobalFree(Some(hg));
                return Err(format!("SetClipboardData: {e}"));
            }
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}

unsafe fn publish_formats(hwnd: HWND) -> Result<u32, String> {
    OpenClipboard(Some(hwnd)).map_err(|e| format!("OpenClipboard: {e}"))?;
    let result = (|| {
        EmptyClipboard().map_err(|e| format!("EmptyClipboard: {e}"))?;
        set_exclusion_markers();
        // Handle NULL = rendering ritardato (successo anche se il crate dà Err con codice 0).
        SetLastError(ERROR_SUCCESS);
        if let Err(e) = SetClipboardData(CF_UNICODETEXT.0 as u32, None) {
            if e.code().is_err() {
                return Err(format!("SetClipboardData: {e}"));
            }
        }
        Ok(())
    })();
    let _ = CloseClipboard();
    result?;
    Ok(GetClipboardSequenceNumber())
}

/// Rimette gli appunti di prima (se sono ancora nostri) e chiude la finestra.
fn settle(s: &Arc<Shared>) {
    if s.settled.swap(true, Ordering::SeqCst) {
        return;
    }
    let ours = !s.ownership_lost.load(Ordering::SeqCst)
        && unsafe { GetClipboardSequenceNumber() } == s.sequence.load(Ordering::SeqCst);
    let snapshot = s.snapshot.lock().ok().and_then(|mut g| g.take());
    if ours {
        if let Some(snap) = snapshot {
            if let Err(e) = restore_clipboard_snapshot(snap) {
                log::warn!("[win] ripristino degli appunti fallito: {e}");
            }
        }
    } else {
        log::info!("[win] appunti cambiati da altri dopo l'incolla: non li tocco");
    }
    let hwnd = s.hwnd.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(hwnd_from(hwnd)), WM_CLOSE, WPARAM(0), LPARAM(0));
        }
    }
    if let Ok(mut p) = PENDING.lock() {
        if p.as_ref().is_some_and(|x| Arc::ptr_eq(x, s)) {
            *p = None;
        }
    }
}

fn ensure_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| unsafe {
        let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: hinstance.into(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        RegisterClassW(&wc);
    });
}

/// Mette `text` negli appunti come promessa riservata a `target`.
pub fn publish(text: &str, target: Target) -> Result<TargetClipboard, String> {
    // Chiude la transazione precedente prima dell'istantanea.
    let previous = PENDING.lock().ok().and_then(|mut p| p.take());
    if let Some(prev) = previous {
        settle(&prev);
    }
    let snapshot = take_clipboard_snapshot()?;
    if let Ok(mut all) = LAST_READERS.lock() {
        all.clear();
    }
    let shared = Arc::new(Shared {
        text: text.to_string(),
        target,
        own_pid: std::process::id(),
        injected: Mutex::new(None),
        receipt: Mutex::new(None),
        unknown_read: Mutex::new(None),
        last_read: Mutex::new(None),
        refused: Mutex::new(Vec::new()),
        published_at: Instant::now(),
        sequence: AtomicU32::new(0),
        ownership_lost: AtomicBool::new(false),
        settled: AtomicBool::new(false),
        hwnd: AtomicIsize::new(0),
        snapshot: Mutex::new(Some(snapshot)),
    });
    let (tx, rx) = std::sync::mpsc::channel::<Result<(), String>>();
    let for_thread = shared.clone();
    std::thread::Builder::new()
        .name("win-clipboard".into())
        .spawn(move || unsafe {
            ensure_class();
            let hinstance = GetModuleHandleW(PCWSTR::null()).unwrap_or_default();
            let hwnd = match CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS_NAME,
                w!(""),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(hinstance.into()),
                None,
            ) {
                Ok(h) => h,
                Err(e) => {
                    let _ = tx.send(Err(format!("CreateWindowExW: {e}")));
                    return;
                }
            };
            let raw = Arc::into_raw(for_thread.clone());
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, raw as isize);
            for_thread.hwnd.store(hwnd.0 as isize, Ordering::SeqCst);
            match publish_formats(hwnd) {
                Ok(seq) => {
                    for_thread.sequence.store(seq, Ordering::SeqCst);
                    let _ = tx.send(Ok(()));
                }
                Err(e) => {
                    let _ = tx.send(Err(e));
                    for_thread.settled.store(true, Ordering::SeqCst);
                    let _ = DestroyWindow(hwnd);
                }
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
                DispatchMessageW(&msg);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Arc::from_raw(raw));
        })
        .map_err(|e| format!("thread: {e}"))?;
    match rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => {
            if let Ok(mut p) = PENDING.lock() {
                *p = Some(shared.clone());
            }
            Ok(TargetClipboard { shared })
        }
        Ok(Err(e)) => {
            // La promessa non è partita: rimetti l'istantanea se l'abbiamo tolta.
            if let Some(snap) = shared.snapshot.lock().ok().and_then(|mut g| g.take()) {
                let _ = restore_clipboard_snapshot(snap);
            }
            Err(e)
        }
        Err(_) => Err("clipboard thread non risponde".into()),
    }
}

impl TargetClipboard {
    /// Prima del Ctrl+V: lascia finire i programmi che leggono gli appunti
    /// appena cambiano (pochi ms dopo la pubblicazione). Se il Ctrl+V
    /// arriva mentre uno di loro li tiene aperti, l'app bersaglio non riesce
    /// ad aprirli e non incolla nulla. Aspetta al
    /// massimo `max`, e `quiet` dopo l'ultima lettura.
    pub fn wait_readers_quiet(&self, max: Duration, quiet: Duration) {
        let t0 = Instant::now();
        while t0.elapsed() < max {
            let last = self.shared.last_read.lock().ok().and_then(|g| *g);
            // Dopo la lettura il monitor può tenere gli appunti aperti ancora
            // per un po': si parte solo quando si riescono ad aprire (e
            // richiudere subito, senza cambiare nulla).
            if last.is_some_and(|t| t.elapsed() >= quiet) && clipboard_free() {
                return;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// Subito prima della scorciatoia: da qui le letture del bersaglio contano.
    pub fn mark_injected(&self) {
        if let Ok(mut g) = self.shared.injected.lock() {
            *g = Some(Instant::now());
        }
    }

    /// Esito dell'incolla entro `timeout`: ricevuta del bersaglio, oppure,
    /// se un lettore sconosciuto ha consumato la promessa, il testo ritrovato
    /// nel campo da `in_field` (chiamata al massimo ogni `VERIFY_EVERY`).
    pub fn wait_outcome(&self, timeout: Duration, mut in_field: impl FnMut() -> FieldCheck) -> ReadOutcome {
        const VERIFY_EVERY: Duration = Duration::from_millis(15);
        let deadline = Instant::now() + timeout;
        let mut last_verify: Option<Instant> = None;
        loop {
            let injected = self.shared.injected.lock().ok().and_then(|g| *g);
            let receipt = self.shared.receipt.lock().ok().and_then(|g| *g);
            if let (Some(i), Some(r)) = (injected, receipt) {
                return ReadOutcome::TargetRead(r.saturating_duration_since(i));
            }
            let unknown = self.shared.unknown_read.lock().ok().and_then(|g| *g);
            if unknown.is_some() && last_verify.is_none_or(|t| t.elapsed() >= VERIFY_EVERY) {
                last_verify = Some(Instant::now());
                let since = injected.map(|i| i.elapsed()).unwrap_or_default();
                match in_field() {
                    FieldCheck::Found => return ReadOutcome::FoundInField(since),
                    FieldCheck::Unreadable => return ReadOutcome::Unverifiable(since),
                    FieldCheck::Pending => {}
                }
            }
            if self.shared.ownership_lost.load(Ordering::SeqCst) || Instant::now() >= deadline {
                return if unknown.is_some() { ReadOutcome::NotInField } else { ReadOutcome::NotRead };
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Aspetta la lettura del bersaglio; ritorna i ms dal Ctrl+V.
    pub fn wait_target_read(&self, timeout: Duration) -> Option<Duration> {
        let deadline = Instant::now() + timeout;
        loop {
            let injected = self.shared.injected.lock().ok().and_then(|g| *g);
            let receipt = self.shared.receipt.lock().ok().and_then(|g| *g);
            if let (Some(i), Some(r)) = (injected, receipt) {
                return Some(r.saturating_duration_since(i));
            }
            if self.shared.ownership_lost.load(Ordering::SeqCst) || Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// Letture rifiutate finora: "exe (pid N)" e ms dalla pubblicazione.
    pub fn refused_readers(&self) -> Vec<(String, u64)> {
        self.shared.refused.lock().map(|v| v.clone()).unwrap_or_default()
    }

    pub fn ownership_lost(&self) -> bool {
        self.shared.ownership_lost.load(Ordering::SeqCst)
    }
}

impl Drop for TargetClipboard {
    /// Ripristino in background: subito se il bersaglio non ha letto, poco
    /// dopo la ricevuta se ha letto.
    fn drop(&mut self) {
        let s = self.shared.clone();
        let read = s.receipt.lock().ok().and_then(|g| *g).is_some()
            || s.unknown_read.lock().ok().and_then(|g| *g).is_some();
        std::thread::spawn(move || {
            if read {
                std::thread::sleep(RESTORE_AFTER_RECEIPT);
            }
            settle(&s);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(pid: u32, exe: &str) -> Reader {
        Reader { pid, exe: exe.into() }
    }

    #[test]
    fn lettore_bersaglio_per_pid_o_nome_dell_exe() {
        let t = Target { pids: vec![100, 200], exe: "claude.exe".into() };
        assert!(is_target_reader(&t, Some(&r(100, "x.exe"))), "pid dell'app");
        assert!(is_target_reader(&t, Some(&r(200, "ApplicationFrameHost.exe"))), "pid della finestra");
        assert!(is_target_reader(&t, Some(&r(999, "Claude.exe"))), "altro processo della stessa app");
        assert!(!is_target_reader(&t, Some(&r(555, "AnyDesk.exe"))));
        assert!(!is_target_reader(&t, Some(&r(556, "TeamViewer.exe"))));
        assert!(!is_target_reader(&t, None), "lettore senza finestra: sconosciuto");
        // Bersaglio sconosciuto (nessuna finestra nel contesto): come prima.
        assert!(is_target_reader(&Target::default(), None));
    }
}
