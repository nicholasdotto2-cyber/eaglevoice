//! Testo selezionato nell'app attiva (per Polish e modalità comando).
//! UIA `TextPattern.GetSelection`; riserva: Ctrl+C con istantanea e
//! ripristino degli appunti. Nei terminali niente riserva (Ctrl+C interrompe).
//!
//! `select_chars_before_caret`: seleziona gli n caratteri prima del cursore
//! (Polish sull'ultima dettatura). UIA `TextPattern2.GetCaretRange` più
//! `MoveEndpointByUnit` e `Select`; riserva con Maiusc+Freccia sinistra.

use std::time::{Duration, Instant};

use windows::core::{Interface, BOOL};
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationTextPattern, IUIAutomationTextPattern2, IUIAutomationTextRange,
    TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start, TextUnit_Character,
    UIA_TextPattern2Id, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::{
    GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId,
    SendMessageTimeoutW, GUITHREADINFO, SMTO_ABORTIFHUNG, WM_GETTEXT, WM_GETTEXTLENGTH,
};

use super::context::{focused_element, foreground_info, hwnd_from, window_info};
use super::keys;
use super::uia::FIELD_WORKER;
use crate::paste_tx::windows::{
    clipboard_sequence, read_clipboard_text, restore_clipboard_snapshot, take_clipboard_snapshot,
};

/// Esito UIA: `Some(Some(t))` selezione letta, `Some(None)` nessuna selezione,
/// `None` UIA non sa rispondere (serve la riserva).
fn uia_selection(timeout: Duration, pid: u32) -> Option<Option<String>> {
    FIELD_WORKER
        .run(timeout, move |a| unsafe {
            let el = focused_element(a, pid).ok()?;
            let tp = el
                .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
                .ok()?;
            let arr = tp.GetSelection().ok()?;
            let mut parts = Vec::new();
            for i in 0..arr.Length().unwrap_or(0) {
                if let Ok(r) = arr.GetElement(i) {
                    if let Ok(t) = r.GetText(-1) {
                        let t = t.to_string();
                        if !t.is_empty() {
                            parts.push(t);
                        }
                    }
                }
            }
            Some((!parts.is_empty()).then(|| parts.join("\n")))
        })
        .flatten()
}

/// Copia la selezione con Ctrl+C e rimette gli appunti com'erano.
pub(crate) fn copy_selection_fallback(timeout: Duration) -> Option<String> {
    let snapshot = take_clipboard_snapshot().ok()?;
    let before = clipboard_sequence();
    let mask = super::current_settings().win_masking;
    if keys::release_held_modifiers(mask).is_err()
        || keys::send_ctrl_chord(keys::VK_C, false, Duration::from_millis(30)).is_err()
    {
        let _ = restore_clipboard_snapshot(snapshot);
        return None;
    }
    let deadline = Instant::now() + timeout;
    let mut copied = None;
    while Instant::now() < deadline {
        if clipboard_sequence() != before {
            std::thread::sleep(Duration::from_millis(20));
            copied = read_clipboard_text();
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let _ = restore_clipboard_snapshot(snapshot);
    copied.filter(|t| !t.is_empty())
}

pub fn get_selected_text(timeout: Duration) -> Option<String> {
    let t0 = Instant::now();
    let info = foreground_info();
    let uia_budget = (timeout / 2).max(Duration::from_millis(50));
    match uia_selection(uia_budget, info.pid) {
        Some(sel) => sel,
        None => {
            let settings = super::current_settings();
            if super::category::is_terminal(&info.class, &info.exe_path, &settings.terminal_processes) {
                return None;
            }
            let remaining = timeout.saturating_sub(t0.elapsed()).max(Duration::from_millis(100));
            copy_selection_fallback(remaining)
        }
    }
}

/// Caratteri come li conta `flow.rs` (`last_dictation_before_caret`): "\r\n" vale uno.
pub(crate) fn chars_normalized(s: &str) -> usize {
    s.replace("\r\n", "\n").chars().count()
}

/// Quanto aspettare che l'utente lasci i modificatori della scorciatoia prima
/// di mandare Maiusc+Freccia (la scorciatoia di Polish scatta alla pressione).
const MODIFIER_RELEASE_WAIT: Duration = Duration::from_millis(1000);

#[derive(Debug)]
enum UiaSelect {
    /// Selezione fatta e riletta con n caratteri; dentro la sorgente del cursore.
    Selected(&'static str),
    /// Nessun TextPattern o nessun cursore: UIA non ha toccato il campo, i tasti possono provare.
    Unsupported(String),
    /// UIA non può o non deve: niente tasti (meno di n caratteri, password,
    /// selezione già cambiata da `Select` ma diversa dall'attesa).
    Refused(String),
}

unsafe fn range_text(r: &IUIAutomationTextRange, n: usize) -> String {
    let max = (n.saturating_mul(2) + 16).min(i32::MAX as usize) as i32;
    r.GetText(max).map(|b| b.to_string()).unwrap_or_default()
}

unsafe fn first_selection(tp: &IUIAutomationTextPattern) -> Option<IUIAutomationTextRange> {
    let arr = tp.GetSelection().ok()?;
    if arr.Length().unwrap_or(0) > 0 {
        arr.GetElement(0).ok()
    } else {
        None
    }
}

/// Lavoro sul thread UIA: cursore, estensione verso sinistra, `Select`, rilettura.
unsafe fn uia_select_before_caret(a: &IUIAutomation, pid: u32, n: usize) -> UiaSelect {
    let el = match focused_element(a, pid) {
        Ok(el) => el,
        Err(r) => return UiaSelect::Unsupported(r.into()),
    };
    if el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) {
        return UiaSelect::Refused("password".into());
    }
    let tp2 = el
        .GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id)
        .ok();
    let tp: Option<IUIAutomationTextPattern> = match &tp2 {
        Some(p) => p.cast().ok(),
        None => el
            .GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId)
            .ok(),
    };
    let Some(tp) = tp else {
        return UiaSelect::Unsupported("no_text_pattern".into());
    };
    let caret = tp2.as_ref().and_then(|p| {
        let mut active = BOOL(0);
        p.GetCaretRange(&mut active).ok()
    });
    // Senza TextPattern2 il cursore è la selezione vuota di TextPattern
    // (se c'è una selezione, il suo estremo destro).
    let (caret, source) = match caret {
        Some(r) => (r, "text_pattern2"),
        None => match first_selection(&tp) {
            Some(r) => (r, "text_pattern"),
            None => return UiaSelect::Unsupported("no_caret".into()),
        },
    };
    let Ok(r) = caret.Clone() else {
        return UiaSelect::Unsupported("clone".into());
    };
    if r
        .MoveEndpointByRange(TextPatternRangeEndpoint_Start, &r, TextPatternRangeEndpoint_End)
        .is_err()
    {
        return UiaSelect::Unsupported("collapse".into());
    }
    // L'unità Character dei provider non sempre coincide con i caratteri di
    // flow.rs ("\r\n" può valere due): si estende finché il testo ne ha n.
    let mut got = 0usize;
    for _ in 0..6 {
        let missing = n - got;
        let moved = r
            .MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, -(missing as i32))
            .unwrap_or(0);
        got = chars_normalized(&range_text(&r, n));
        if got >= n || moved == 0 {
            break;
        }
    }
    if got < n {
        return UiaSelect::Refused(format!("solo {got} caratteri prima del cursore su {n}"));
    }
    if got > n {
        return UiaSelect::Unsupported(format!("unità Character non allineata ({got} invece di {n})"));
    }
    let expected = range_text(&r, n);
    if let Err(e) = r.Select() {
        return UiaSelect::Unsupported(format!("Select: {e}"));
    }
    // Rilettura: la selezione del campo deve essere proprio quel testo.
    let now = first_selection(&tp).map(|s| range_text(&s, n)).unwrap_or_default();
    if now == expected {
        UiaSelect::Selected(source)
    } else {
        UiaSelect::Refused(format!(
            "dopo Select la selezione ha {} caratteri invece di {n}",
            chars_normalized(&now)
        ))
    }
}

fn ensure_foreground(expected_hwnd: isize) -> Result<(), String> {
    let fg = unsafe { GetForegroundWindow() }.0 as isize;
    if expected_hwnd != 0 && fg == expected_hwnd {
        Ok(())
    } else {
        Err(format!(
            "in primo piano non c'è più la finestra del contesto ({fg:#x} invece di {expected_hwnd:#x}): nessun tasto inviato"
        ))
    }
}

/// Riserva con i tasti: Maiusc+Freccia sinistra n volte, in un solo `SendInput`.
///
/// Perché il Maiusc si perdeva in `flow.rs::select_left` (riprodotto in
/// `live_selezione_prima_del_cursore`): Polish scatta alla pressione, con
/// Ctrl+Alt+Maiusc ancora sotto le dita. `release_held_modifiers` li rilascia
/// per finta, poi parte Maiusc giù e le frecce; il rilascio vero del Maiusc
/// arriva mentre le frecce sono ancora in viaggio (circa 1,5 ms l'una con gli
/// hook di tastiera attivi), le frecce dopo di lui muovono senza Maiusc e
/// annullano la selezione: cursore spostato di n, niente selezionato.
/// Qui si aspetta prima che l'utente lasci i modificatori (finché non abbiamo
/// iniettato nulla lo stato asincrono è quello fisico); solo se li tiene oltre
/// il tetto li rilascia d'ufficio e ripete Maiusc giù prima di ogni freccia.
pub(crate) fn select_with_keys(expected_hwnd: isize, n: usize) -> Result<(), String> {
    use super::keys::{key_input_scan, VK_LEFT, VK_LSHIFT};
    ensure_foreground(expected_hwnd)?;
    let released = keys::wait_modifiers_released(MODIFIER_RELEASE_WAIT);
    ensure_foreground(expected_hwnd)?;
    if !released {
        let mask = super::current_settings().win_masking;
        let held = keys::release_held_modifiers(mask)?;
        log::debug!("[win] modificatori ancora tenuti dopo {MODIFIER_RELEASE_WAIT:?}: rilasciati {held:x?}");
        std::thread::sleep(Duration::from_millis(30));
    }
    let mut inputs = Vec::with_capacity(n * 3 + 2);
    inputs.push(key_input_scan(VK_LSHIFT, false, false));
    for _ in 0..n {
        // Dita ancora sui tasti: un loro rilascio può arrivare a metà invio,
        // quindi Maiusc giù di nuovo prima di ogni freccia.
        if !released {
            inputs.push(key_input_scan(VK_LSHIFT, false, false));
        }
        inputs.push(key_input_scan(VK_LEFT, false, true));
        inputs.push(key_input_scan(VK_LEFT, true, true));
    }
    inputs.push(key_input_scan(VK_LSHIFT, true, false));
    ensure_foreground(expected_hwnd)?;
    keys::send(&inputs)?;
    std::thread::sleep(Duration::from_millis(60));
    Ok(())
}

/// Testo selezionato in un controllo EDIT o RichEdit Win32 (`EM_GETSEL`,
/// `WM_GETTEXT`), per controllare la riserva con i tasti dove UIA non arriva.
pub(crate) fn edit_selected_text(edit: isize) -> Option<String> {
    const EM_GETSEL: u32 = 0x00B0;
    let hwnd = hwnd_from(edit);
    let send = |msg: u32, wp: usize, lp: isize| -> Option<usize> {
        let mut out = 0usize;
        let r = unsafe {
            SendMessageTimeoutW(hwnd, msg, WPARAM(wp), LPARAM(lp), SMTO_ABORTIFHUNG, 200, Some(&mut out))
        };
        (r.0 != 0).then_some(out)
    };
    let mut start = 0u32;
    let mut end = 0u32;
    send(EM_GETSEL, &mut start as *mut u32 as usize, &mut end as *mut u32 as isize)?;
    let len = send(WM_GETTEXTLENGTH, 0, 0)?;
    let mut buf = vec![0u16; len + 1];
    let copied = send(WM_GETTEXT, buf.len(), buf.as_mut_ptr() as isize)?;
    let (s, e) = (start as usize, (end as usize).min(copied));
    (s <= e).then(|| String::from_utf16_lossy(&buf[s..e]))
}

/// Controllo col fuoco nella finestra `hwnd`, se è un EDIT o RichEdit.
fn focused_edit(hwnd: isize) -> Option<isize> {
    unsafe {
        let thread = GetWindowThreadProcessId(hwnd_from(hwnd), None);
        let mut gui = GUITHREADINFO {
            cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
            ..Default::default()
        };
        GetGUIThreadInfo(thread, &mut gui).ok()?;
        let focus: HWND = gui.hwndFocus;
        if focus.0.is_null() {
            return None;
        }
        let mut class = [0u16; 64];
        let len = GetClassNameW(focus, &mut class).max(0) as usize;
        let class = String::from_utf16_lossy(&class[..len]).to_lowercase();
        (class == "edit" || class.starts_with("richedit")).then_some(focus.0 as isize)
    }
}

/// Seleziona gli `n` caratteri prima del cursore nel campo col fuoco della
/// finestra in primo piano. Vedi `select_chars_before_caret_in`.
pub fn select_chars_before_caret(n: usize, timeout: Duration) -> Result<(), String> {
    select_chars_before_caret_in(foreground_info().hwnd, n, timeout)
}

/// Seleziona gli `n` caratteri prima del cursore (contati come in `flow.rs`:
/// "\r\n" vale uno) nel campo col fuoco di `expected_hwnd`, la finestra letta
/// nel contesto. Prima UI Automation (`TextPattern2.GetCaretRange`, oppure il
/// cursore di `TextPattern`), `MoveEndpointByUnit`, `Select` e rilettura della
/// selezione; se il campo non ha TextPattern, Maiusc+Freccia sinistra.
/// Nessun tasto se in primo piano c'è un'altra finestra o se UIA scade
/// (un `Select` arrivato in ritardo più i tasti selezionerebbe 2n caratteri).
pub fn select_chars_before_caret_in(
    expected_hwnd: isize,
    n: usize,
    timeout: Duration,
) -> Result<(), String> {
    if n == 0 {
        return Ok(());
    }
    let t0 = Instant::now();
    ensure_foreground(expected_hwnd)?;
    let pid = window_info(expected_hwnd).pid;
    let uia = FIELD_WORKER.run(timeout, move |a| unsafe { uia_select_before_caret(a, pid, n) });
    match uia {
        None => Err(format!("UI Automation oltre {timeout:?}: nessun tasto inviato")),
        Some(UiaSelect::Selected(source)) => {
            log::debug!("[win] {n} caratteri selezionati via {source} in {:?}", t0.elapsed());
            Ok(())
        }
        Some(UiaSelect::Refused(why)) => Err(why),
        Some(UiaSelect::Unsupported(why)) => {
            log::debug!("[win] selezione UIA non disponibile ({why}): Maiusc+Freccia sinistra");
            select_with_keys(expected_hwnd, n)?;
            // Rilettura dove si può (EDIT Win32); altrove la selezione resta non verificata.
            if let Some(edit) = focused_edit(expected_hwnd) {
                let got = edit_selected_text(edit).map(|t| chars_normalized(&t)).unwrap_or(0);
                if got != n {
                    return Err(format!("dopo i tasti la selezione ha {got} caratteri invece di {n}"));
                }
            }
            log::debug!("[win] {n} caratteri selezionati con i tasti in {:?}", t0.elapsed());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caratteri_contati_come_flow() {
        assert_eq!(chars_normalized("Nota: riga"), 10);
        assert_eq!(chars_normalized("riga uno\r\nriga due"), 17);
        assert_eq!(chars_normalized("à è ì ò ù xyz"), 13);
    }

    #[test]
    fn niente_tasti_se_la_finestra_non_e_in_primo_piano() {
        // HWND inesistente: la funzione si ferma prima di UIA e di SendInput.
        let err = select_chars_before_caret_in(0x7fff_fff0, 5, Duration::from_millis(100)).unwrap_err();
        assert!(err.contains("nessun tasto"), "{err}");
        assert!(select_chars_before_caret_in(0x7fff_fff0, 0, Duration::from_millis(100)).is_ok());
    }
}
