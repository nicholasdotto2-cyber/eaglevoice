//! Incolla nella finestra catturata alla pressione.
//!
//! Riusa la transazione di `paste_tx\windows.rs`: testo negli appunti come
//! promessa a rendering ritardato, con i formati
//! `ExcludeClipboardContentFromMonitorProcessing`, `CanIncludeInClipboardHistory=0`
//! e `CanUploadToCloudClipboard=0` (niente cronologia appunti né cloud),
//! ricevuta `WM_RENDERFORMAT` come prova che l'app ha letto, ripristino degli
//! appunti di prima protetto dal numero di sequenza.
//! In più: finestra bersaglio riportata in primo piano se coperta,
//! modificatori ancora premuti rilasciati (con maschera di Win), Ctrl+Shift+V
//! nei terminali, Invio simulato su richiesta, esito sincrono con millisecondi.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows::Win32::System::Threading::{AttachThreadInput, GetCurrentThreadId};
use windows::Win32::UI::WindowsAndMessaging::{
    BringWindowToTop, GetForegroundWindow, GetWindowThreadProcessId, IsIconic, IsWindow,
    SetForegroundWindow, ShowWindow, SW_RESTORE,
};

use super::context::hwnd_from;
use super::keys;
use super::{AppContext, PasteOutcome};
use super::cliptx::{self, FieldCheck, ReadOutcome, Target};
use super::context::{focused_element, read_field, read_full_text};
use super::uia::FIELD_WORKER;
use crate::paste_tx::windows::publish_standalone;
use crate::settings::PasteMethod;

/// Quanto aspettare che l'app legga gli appunti dopo la scorciatoia.
pub const RECEIPT_TIMEOUT: Duration = Duration::from_millis(1500);
/// Tenuta dei modificatori della scorciatoia (Handy usa 100 ms; la ricevuta
/// rende visibile una scorciatoia persa, quindi si può stare più bassi).
pub const CHORD_HOLD: Duration = Duration::from_millis(40);
/// Pausa fra la lettura degli appunti e l'Invio simulato.
pub const ENTER_DELAY: Duration = Duration::from_millis(80);
const FOREGROUND_TIMEOUT: Duration = Duration::from_millis(300);
/// Attesa massima, prima del Ctrl+V, che finiscano le letture dei monitor degli appunti.
const READERS_MAX_WAIT: Duration = Duration::from_millis(40);
/// Silenzio dopo l'ultima lettura che basta per partire.
const READERS_QUIET: Duration = Duration::from_millis(6);
/// Dopo il Ctrl+V, quanto aspettare il testo nel campo prima di riprovare
/// (le app di solito ci mettono molto meno).
const RETRY_AFTER: Duration = Duration::from_millis(400);
const CTRL_V_RETRIES: u32 = 2;

/// Numero dell'ultimo incolla avviato: l'osservazione in corso si ferma
/// (`next_paste_started`) quando cambia.
pub(crate) static PASTE_GENERATION: AtomicU64 = AtomicU64::new(0);

/// Porta `hwnd` in primo piano (trucco `AttachThreadInput`) e attende che lo
/// sia davvero. `Ok(true)` se è stato necessario riattivarla.
pub fn bring_to_foreground(hwnd_raw: isize, timeout: Duration) -> Result<bool, String> {
    let target = hwnd_from(hwnd_raw);
    unsafe {
        if GetForegroundWindow() == target {
            return Ok(false);
        }
        if !IsWindow(Some(target)).as_bool() {
            return Err("target_window_gone".into());
        }
        if IsIconic(target).as_bool() {
            let _ = ShowWindow(target, SW_RESTORE);
        }
        let me = GetCurrentThreadId();
        let fg_thread = GetWindowThreadProcessId(GetForegroundWindow(), None);
        let target_thread = GetWindowThreadProcessId(target, None);
        let attach_fg = fg_thread != 0 && fg_thread != me;
        let attach_target = target_thread != 0 && target_thread != me && target_thread != fg_thread;
        if attach_fg {
            let _ = AttachThreadInput(me, fg_thread, true);
        }
        if attach_target {
            let _ = AttachThreadInput(me, target_thread, true);
        }
        let _ = BringWindowToTop(target);
        let _ = SetForegroundWindow(target);
        if attach_target {
            let _ = AttachThreadInput(me, target_thread, false);
        }
        if attach_fg {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
        let deadline = Instant::now() + timeout;
        let retry_at = Instant::now() + Duration::from_millis(60);
        let mut retried = false;
        while Instant::now() < deadline {
            if GetForegroundWindow() == target {
                return Ok(true);
            }
            if !retried && Instant::now() >= retry_at {
                // Il blocco del primo piano si toglie quando il processo
                // ha generato l'ultimo input: il tasto neutro 0xE8 basta e
                // non fa nulla nell'app.
                retried = true;
                let _ = keys::send(&[
                    keys::key_input(keys::MASK_VK, false),
                    keys::key_input(keys::MASK_VK, true),
                ]);
                let _ = SetForegroundWindow(target);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    Err("foreground_not_restored".into())
}

fn outcome(t0: Instant, method: PasteMethod) -> PasteOutcome {
    PasteOutcome {
        success: false,
        reason: None,
        elapsed_ms: t0.elapsed().as_millis() as u64,
        method,
        clipboard_read_ms: None,
        press_enter_sent: false,
        focus_restored: false,
    }
}

/// Incolla `text` nella finestra di `ctx`. Sincrono: ritorna quando l'app ha
/// letto gli appunti (o allo scadere di `RECEIPT_TIMEOUT`); il ripristino
/// degli appunti finisce in background poco dopo.
pub fn paste_text(text: &str, ctx: &AppContext, press_enter: bool) -> PasteOutcome {
    let t0 = Instant::now();
    PASTE_GENERATION.fetch_add(1, Ordering::SeqCst);
    let settings = super::current_settings();
    let method = if ctx.is_terminal {
        PasteMethod::CtrlShiftV
    } else {
        PasteMethod::CtrlV
    };
    let mut out = outcome(t0, method);
    let fail = |mut o: PasteOutcome, reason: String| {
        o.reason = Some(reason);
        o.elapsed_ms = t0.elapsed().as_millis() as u64;
        log::warn!("[win] incolla fallito: {:?}", o.reason);
        o
    };

    if text.is_empty() {
        return fail(out, "empty_text".into());
    }
    if ctx.hwnd != 0 {
        match bring_to_foreground(ctx.hwnd, FOREGROUND_TIMEOUT) {
            Ok(restored) => out.focus_restored = restored,
            Err(e) => return fail(out, e),
        }
    }
    if let Err(e) = keys::release_held_modifiers(settings.win_masking) {
        return fail(out, format!("release_modifiers: {e}"));
    }

    // La ricevuta vale solo se a leggere è il bersaglio (vedi `cliptx`).
    let target = Target::from_window(ctx.hwnd, ctx.pid, &ctx.process_exe);
    if target.is_unknown() {
        // Nessuna finestra nel contesto: non si sa chi deve leggere, resta la
        // ricevuta di `paste_tx` (qualunque lettore dopo il Ctrl+V).
        return paste_any_reader(text, method, press_enter, out, t0);
    }
    // Un altro processo può tenere aperti gli appunti per qualche ms.
    let mut tx = Err(String::new());
    for attempt in 0..3 {
        tx = cliptx::publish(text, target.clone());
        if tx.is_ok() {
            break;
        }
        if attempt < 2 {
            std::thread::sleep(Duration::from_millis(30));
        }
    }
    let tx = match tx {
        Ok(tx) => tx,
        Err(e) => return fail(out, format!("clipboard_publish: {e}")),
    };

    tx.wait_readers_quiet(READERS_MAX_WAIT, READERS_QUIET);
    let pid = ctx.pid;
    // Il campo prima del Ctrl+V: serve a capire se la scorciatoia non ha avuto effetto.
    let baseline = field_snapshot(pid, text);
    let check = |base: &Option<FieldSnap>| check_field(text, base.as_ref(), field_snapshot(pid, text).as_ref());
    tx.mark_injected();
    if let Err(e) = keys::send_ctrl_chord(keys::VK_V, method == PasteMethod::CtrlShiftV, CHORD_HOLD) {
        return fail(out, format!("send_input: {e}"));
    }
    let t_inject = Instant::now();
    let mut outcome = tx.wait_outcome(RETRY_AFTER, || check(&baseline));
    if outcome == ReadOutcome::NotRead {
        // Nessuna lettura ancora: l'app può essere lenta, si aspetta il resto.
        outcome = tx.wait_outcome(RECEIPT_TIMEOUT.saturating_sub(t_inject.elapsed()), || check(&baseline));
    }
    // Testo letto da un lettore sconosciuto ma campo IDENTICO a prima: la
    // scorciatoia è andata persa (visto dal vivo, con un programma che
    // rilegge gli appunti ogni 30 ms e li tiene occupati). Il testo è ancora
    // negli appunti: si riprova, al massimo due volte. Con il campo cambiato
    // in altro modo, o illeggibile, mai.
    let mut retries = 0;
    while outcome == ReadOutcome::NotInField
        && retries < CTRL_V_RETRIES
        && !tx.ownership_lost()
        && baseline.is_some()
        && field_snapshot(pid, text) == baseline
        && unsafe { GetForegroundWindow() }.0 as isize == ctx.hwnd
    {
        retries += 1;
        log::info!("[win] Ctrl+V senza effetto nel campo: nuovo tentativo {retries}");
        if keys::send_ctrl_chord(keys::VK_V, method == PasteMethod::CtrlShiftV, CHORD_HOLD).is_err() {
            break;
        }
        outcome = tx.wait_outcome(RETRY_AFTER, || check(&baseline));
    }
    LAST_CTRL_V_RETRIES.store(retries, Ordering::SeqCst);
    match outcome {
        ReadOutcome::TargetRead(read) | ReadOutcome::FoundInField(read) => {
            out.success = true;
            out.clipboard_read_ms = Some(read.as_millis() as u64);
        }
        ReadOutcome::Unverifiable(read) => {
            // Campo illeggibile: vale la lettura, come prima, ma resta nel log.
            out.success = true;
            out.clipboard_read_ms = Some(read.as_millis() as u64);
            out.reason = Some("non verificabile nel campo".into());
            log::info!("[win] incolla in {}: letto da un programma senza finestra, campo non leggibile: non verificabile nel campo", ctx.app_name);
        }
        ReadOutcome::NotInField | ReadOutcome::NotRead => {
            // Il campo non ha ricevuto il testo (o non si può provare che
            // l'abbia ricevuto): gli appunti tornano come prima e il testo
            // resta per Incolla ultima.
            let others: Vec<String> = tx.refused_readers().into_iter().map(|(who, _)| who).collect();
            let why = if tx.ownership_lost() {
                "appunti cambiati da un altro programma prima che l'app leggesse".to_string()
            } else if outcome == ReadOutcome::NotInField {
                format!(
                    "appunti letti da un programma non riconoscibile ma testo non ritrovato nel campo entro {} ms",
                    RECEIPT_TIMEOUT.as_millis()
                )
            } else if others.is_empty() {
                format!("l'app non ha letto gli appunti entro {} ms", RECEIPT_TIMEOUT.as_millis())
            } else {
                format!(
                    "l'app non ha letto gli appunti entro {} ms (letture di altri programmi, ignorate: {})",
                    RECEIPT_TIMEOUT.as_millis(),
                    others.join(", ")
                )
            };
            return fail(out, format!("target_did_not_read: {why}"));
        }
    }
    let others = tx.refused_readers();
    if !others.is_empty() {
        log::debug!("[win] letture degli appunti ignorate durante l'incolla: {others:?}");
    }
    if press_enter {
        std::thread::sleep(ENTER_DELAY);
        match keys::send_enter() {
            Ok(()) => out.press_enter_sent = true,
            Err(e) => out.reason = Some(format!("press_enter: {e}")),
        }
    }
    out.elapsed_ms = t0.elapsed().as_millis() as u64;
    log::info!(
        "[win] incollato in {} con {:?}: {} ms (lettura appunti {:?} ms)",
        ctx.app_name,
        method,
        out.elapsed_ms,
        out.clipboard_read_ms
    );
    out
}

/// Nuovi Ctrl+V dell'ultimo incolla (per le prove: `PasteOutcome` non cambia forma).
pub(crate) static LAST_CTRL_V_RETRIES: AtomicU32 = AtomicU32::new(0);

/// Spazi, a capo, NBSP e caratteri di controllo ridotti a un solo spazio
/// (ProseMirror in Claude riscrive a capo e spazi).
pub(crate) fn normalize_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut gap = false;
    for c in s.chars() {
        if c.is_whitespace() || c.is_control() || c == '\u{a0}' || c == '\u{200b}' || c == '\u{feff}' {
            gap = true;
        } else {
            if gap && !out.is_empty() {
                out.push(' ');
            }
            gap = false;
            out.push(c);
        }
    }
    out
}

/// Il campo col fuoco, normalizzato: testo prima del cursore (o valore intero
/// con il solo ValuePattern) e testo intero.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FieldSnap {
    pub before: String,
    pub full: String,
}

/// Lettura del campo col fuoco di `pid` (UI Automation, tetto 150 ms); `None` se illeggibile.
fn field_snapshot(pid: u32, text: &str) -> Option<FieldSnap> {
    let limit = (text.chars().count() * 2 + 200).min(i32::MAX as usize) as i32;
    FIELD_WORKER
        .run(Duration::from_millis(150), move |a| unsafe {
            let el = focused_element(a, pid).ok()?;
            let f = read_field(a, &el, limit).ok()?;
            let full = read_full_text(&el, 200_000)
                .unwrap_or_else(|| format!("{}{}{}", f.before, f.selected, f.after));
            Some(FieldSnap { before: normalize_ws(&f.before), full: normalize_ws(&full) })
        })
        .flatten()
}

/// Esito della verifica per un lettore senza finestra:
/// campo illeggibile prima o adesso: non verificabile; testo normalizzato
/// prima del cursore o nel valore (anche senza spazi, se il campo li ha
/// tolti fra i paragrafi): trovato; campo cresciuto di almeno metà del testo
/// rispetto a prima del Ctrl+V: trovato; altrimenti si aspetta.
pub(crate) fn check_field(text: &str, baseline: Option<&FieldSnap>, now: Option<&FieldSnap>) -> FieldCheck {
    let (Some(base), Some(now)) = (baseline, now) else {
        return FieldCheck::Unreadable;
    };
    let want = normalize_ws(text);
    if want.is_empty() {
        return FieldCheck::Unreadable;
    }
    // Una occorrenza in più del testo rispetto a prima (così un testo già
    // presente nel campo non basta), con o senza gli spazi.
    let compact = |s: &str| s.chars().filter(|c| *c != ' ').collect::<String>();
    let more = |hay_now: &str, hay_base: &str, needle: &str| {
        !needle.is_empty() && hay_now.matches(needle).count() > hay_base.matches(needle).count()
    };
    if more(&now.full, &base.full, &want) || more(&now.before, &base.before, &want) {
        return FieldCheck::Found;
    }
    if more(&compact(&now.full), &compact(&base.full), &compact(&want)) {
        return FieldCheck::Found;
    }
    let grown = now.full.chars().count() as i64 - base.full.chars().count() as i64;
    if grown >= (want.chars().count() as i64 + 1) / 2 {
        return FieldCheck::Found;
    }
    FieldCheck::Pending
}

/// Incolla senza bersaglio noto (contesto senza finestra): ricevuta di
/// `paste_tx`, valida per qualunque lettore dopo il Ctrl+V.
fn paste_any_reader(
    text: &str,
    method: PasteMethod,
    press_enter: bool,
    mut out: PasteOutcome,
    t0: Instant,
) -> PasteOutcome {
    let tx = match publish_standalone(text) {
        Ok(tx) => tx,
        Err(e) => {
            out.reason = Some(format!("clipboard_publish: {e}"));
            out.elapsed_ms = t0.elapsed().as_millis() as u64;
            return out;
        }
    };
    tx.mark_injected();
    if let Err(e) = keys::send_ctrl_chord(keys::VK_V, method == PasteMethod::CtrlShiftV, CHORD_HOLD) {
        tx.mark_injection_failed();
        out.reason = Some(format!("send_input: {e}"));
        out.elapsed_ms = t0.elapsed().as_millis() as u64;
        return out;
    }
    match tx.wait_first_receipt(RECEIPT_TIMEOUT) {
        Some(read) => {
            out.success = true;
            out.clipboard_read_ms = Some(read.as_millis() as u64);
        }
        None => out.reason = Some("no_clipboard_read".into()),
    }
    if out.success && press_enter {
        std::thread::sleep(ENTER_DELAY);
        match keys::send_enter() {
            Ok(()) => out.press_enter_sent = true,
            Err(e) => out.reason = Some(format!("press_enter: {e}")),
        }
    }
    out.elapsed_ms = t0.elapsed().as_millis() as u64;
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(before: &str, full: &str) -> FieldSnap {
        FieldSnap { before: normalize_ws(before), full: normalize_ws(full) }
    }

    #[test]
    fn verifica_tollerante_per_i_lettori_senza_finestra() {
        let testo = "Prima riga.\r\n\r\nSeconda  riga\ncon\u{a0}spazio.";
        assert_eq!(normalize_ws(testo), "Prima riga. Seconda riga con spazio.");
        let base = snap("Testo ", "Testo ");
        // ProseMirror: paragrafi, spazi doppi ridotti.
        let pm = snap("Testo Prima riga.\nSeconda riga\ncon spazio.", "Testo Prima riga.\nSeconda riga\ncon spazio.");
        assert_eq!(check_field(testo, Some(&base), Some(&pm)), FieldCheck::Found);
        // Paragrafi senza nessuno spazio fra loro: vale il confronto senza spazi.
        let attaccato = snap("Testo Prima riga.Seconda rigacon spazio.", "Testo Prima riga.Seconda rigacon spazio.");
        assert_eq!(check_field(testo, Some(&base), Some(&attaccato)), FieldCheck::Found);
        // Campo cresciuto di almeno metà del testo, testo riscritto in altro modo.
        let cresciuto = snap("Testo xxxxxxxxxxxxxxxxxxxx", "Testo xxxxxxxxxxxxxxxxxxxx");
        assert_eq!(check_field(testo, Some(&base), Some(&cresciuto)), FieldCheck::Found);
        // Identico a prima: si aspetta (e poi, alla scadenza, nuovo Ctrl+V).
        assert_eq!(check_field(testo, Some(&base), Some(&base)), FieldCheck::Pending);
        // Testo già presente prima del Ctrl+V: non basta.
        let gia = snap("Prima riga. Seconda riga con spazio. ", "Prima riga. Seconda riga con spazio. ");
        assert_eq!(check_field(testo, Some(&gia), Some(&gia)), FieldCheck::Pending);
        // Campo illeggibile prima o dopo: non verificabile.
        assert_eq!(check_field(testo, None, Some(&pm)), FieldCheck::Unreadable);
        assert_eq!(check_field(testo, Some(&base), None), FieldCheck::Unreadable);
    }
}
