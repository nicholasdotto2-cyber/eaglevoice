//! Sostituzione verificata di un testo già incollato (Transforms).
//!
//! Serve a riscrivere l'ultima dettatura senza selezione dell'utente e ad
//! annullare un Transform. Scelta: selezione del testo via UI Automation e
//! incolla sopra, non Ctrl+Z. Motivo: Ctrl+Z non si può verificare prima di
//! premerlo e il suo effetto cambia da app ad app (Electron e ProseMirror
//! raggruppano gli annullamenti, un campo può annullare anche le correzioni
//! fatte a mano dopo l'incolla, o niente); la selezione invece si rilegge
//! prima di toccare il contenuto, e se non torna si rinuncia con il campo intatto.
//!
//! Passi: campo col fuoco della finestra del contesto (no password, no
//! terminali) → il testo da sostituire c'è ancora → occorrenza più vicina
//! prima del cursore (`FindText` all'indietro, riserva per posizione) →
//! `Select` e rilettura della selezione → incolla → rilettura del campo.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use specta::Type;
use windows::core::{Interface, BOOL, BSTR};
use windows::Win32::UI::Accessibility::{
    IUIAutomation, IUIAutomationElement, IUIAutomationTextPattern, IUIAutomationTextPattern2,
    IUIAutomationTextRange, TextPatternRangeEndpoint_End, TextPatternRangeEndpoint_Start,
    TextUnit_Character, UIA_TextPattern2Id, UIA_TextPatternId,
};
use windows::Win32::UI::WindowsAndMessaging::GetForegroundWindow;

use super::context::{focused_element, read_full_text};
use super::paste::paste_text;
use super::uia::FIELD_WORKER;
use super::{AppContext, PasteOutcome};

const FULL_TEXT_MAX: i32 = 60_000;
/// Finestra dell'ultima dettatura per i Transforms senza selezione (60 s).
pub const LAST_DICTATION_MAX_AGE: Duration = Duration::from_secs(60);
const UIA_TIMEOUT: Duration = Duration::from_millis(400);
const VERIFY_DELAY: Duration = Duration::from_millis(120);
const VERIFY_ATTEMPTS: u32 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Type)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum ReplaceError {
    /// Il campo non è stato toccato: il testo nuovo va offerto negli appunti.
    NotVerifiable(String),
    /// Selezione fatta ma incolla non riuscito o risultato diverso dall'atteso:
    /// il campo va controllato a vista; il testo nuovo va offerto negli appunti.
    Unverified(String),
}

impl ReplaceError {
    /// Messaggio per la barra.
    pub fn user_message(&self) -> String {
        match self {
            ReplaceError::NotVerifiable(why) => {
                format!("Non tocco il campo ({why}): usa gli appunti per incollare il testo nuovo.")
            }
            ReplaceError::Unverified(why) => {
                format!("Sostituzione non verificata ({why}): controlla il campo, il testo nuovo è da incollare dagli appunti.")
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct ReplaceOutcome {
    pub elapsed_ms: u64,
    /// "find_text" o "offset": come è stata trovata l'occorrenza.
    pub located_by: String,
    pub paste: PasteOutcome,
}

/// Ultima dettatura incollata e osservata (la registra `observe_after_paste`).
#[derive(Debug, Clone)]
pub struct LastDictation {
    pub at: Instant,
    pub text: String,
    pub ctx: AppContext,
}

static LAST_DICTATION: Mutex<Option<LastDictation>> = Mutex::new(None);

pub fn record_dictation(text: &str, ctx: &AppContext) {
    if let Ok(mut slot) = LAST_DICTATION.lock() {
        *slot = Some(LastDictation { at: Instant::now(), text: text.trim().to_string(), ctx: ctx.clone() });
    }
}

/// Ultima dettatura degli ultimi `max_age`, solo se la sua finestra è in primo piano.
pub fn last_dictation_within(max_age: Duration) -> Option<LastDictation> {
    let last = LAST_DICTATION.lock().ok()?.clone()?;
    let fg = unsafe { GetForegroundWindow() }.0 as isize;
    (last.at.elapsed() <= max_age && !last.text.is_empty() && last.ctx.hwnd == fg).then_some(last)
}

/// Posizione (in caratteri, "\r\n" vale due come nel testo letto) dell'occorrenza
/// di `needle` più vicina prima di `caret`; se nessuna sta prima, l'ultima.
pub fn pick_occurrence(text: &str, needle: &str, caret: Option<usize>) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    let starts: Vec<usize> = text
        .match_indices(needle)
        .map(|(b, _)| text[..b].chars().count())
        .collect();
    let n = needle.chars().count();
    match caret {
        Some(c) => starts
            .iter()
            .copied()
            .filter(|s| s + n <= c)
            .max()
            .or_else(|| starts.last().copied()),
        None => starts.last().copied(),
    }
}

/// Il testo cercato così come compare nel campo (gli EDIT Win32 usano "\r\n").
fn as_in_field(text: &str, needle: &str) -> Option<String> {
    let n = needle.trim();
    if n.is_empty() {
        return None;
    }
    if text.contains(n) {
        return Some(n.to_string());
    }
    let crlf = n.replace("\r\n", "\n").replace('\n', "\r\n");
    text.contains(&crlf).then_some(crlf)
}

unsafe fn text_pattern(el: &IUIAutomationElement) -> Option<(IUIAutomationTextPattern, Option<IUIAutomationTextPattern2>)> {
    let tp2 = el.GetCurrentPatternAs::<IUIAutomationTextPattern2>(UIA_TextPattern2Id).ok();
    let tp = match &tp2 {
        Some(p) => p.cast().ok(),
        None => el.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId).ok(),
    }?;
    Some((tp, tp2))
}

unsafe fn range_text(r: &IUIAutomationTextRange) -> String {
    r.GetText(FULL_TEXT_MAX).map(|b| b.to_string()).unwrap_or_default()
}

/// Caratteri prima del cursore (lunghezza del testo dall'inizio al cursore).
unsafe fn caret_offset(
    doc: &IUIAutomationTextRange,
    tp: &IUIAutomationTextPattern,
    tp2: Option<&IUIAutomationTextPattern2>,
) -> Option<(usize, IUIAutomationTextRange)> {
    let caret = tp2
        .and_then(|p| {
            let mut active = BOOL(0);
            p.GetCaretRange(&mut active).ok()
        })
        .or_else(|| {
            let arr = tp.GetSelection().ok()?;
            (arr.Length().unwrap_or(0) > 0).then(|| arr.GetElement(0).ok()).flatten()
        })?;
    let head = doc.Clone().ok()?;
    head.MoveEndpointByRange(TextPatternRangeEndpoint_End, &caret, TextPatternRangeEndpoint_End)
        .ok()?;
    Some((range_text(&head).chars().count(), caret))
}

/// Seleziona l'occorrenza di `needle` scelta; ritorna come è stata trovata.
unsafe fn select_occurrence(a: &IUIAutomation, pid: u32, old: &str) -> Result<&'static str, String> {

    let el = focused_element(a, pid).map_err(|e| e.to_string())?;
    if el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false) {
        return Err("campo password".into());
    }
    let (tp, tp2) = text_pattern(&el).ok_or("campo senza TextPattern")?;
    let doc = tp.DocumentRange().map_err(|e| format!("DocumentRange: {e}"))?;
    let full = range_text(&doc);
    let needle = as_in_field(&full, old).ok_or("il campo non contiene più il testo da sostituire")?;
    let caret = caret_offset(&doc, &tp, tp2.as_ref());
    let start = pick_occurrence(&full, &needle, caret.as_ref().map(|c| c.0)).ok_or("occorrenza non trovata")?;
    let n = needle.chars().count();

    // 1. FindText all'indietro su [inizio, fine dell'occorrenza scelta].
    let mut chosen: Option<(IUIAutomationTextRange, &'static str)> = None;
    if let Ok(search) = doc.Clone() {
        let ok = search
            .MoveEndpointByRange(TextPatternRangeEndpoint_End, &search, TextPatternRangeEndpoint_Start)
            .is_ok()
            && search
                .MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, (start + n) as i32)
                .is_ok();
        let probe = if ok { Some(search) } else { None };
        for range in [probe, Some(doc.clone())].into_iter().flatten() {
            if let Ok(found) = range.FindText(&BSTR::from(needle.as_str()), true, false) {
                if range_text(&found) == needle {
                    // L'occorrenza trovata deve iniziare dove ce l'aspettiamo.
                    if let Ok(head) = doc.Clone() {
                        if head
                            .MoveEndpointByRange(TextPatternRangeEndpoint_End, &found, TextPatternRangeEndpoint_Start)
                            .is_ok()
                            && range_text(&head).chars().count() == start
                        {
                            chosen = Some((found, "find_text"));
                            break;
                        }
                    }
                }
            }
        }
    }
    // 2. Riserva per posizione: unità Character dall'inizio del documento.
    if chosen.is_none() {
        let r = doc.Clone().map_err(|e| format!("Clone: {e}"))?;
        let _ = r.MoveEndpointByRange(TextPatternRangeEndpoint_End, &r, TextPatternRangeEndpoint_Start);
        let _ = r.MoveEndpointByUnit(TextPatternRangeEndpoint_End, TextUnit_Character, (start + n) as i32);
        let _ = r.MoveEndpointByUnit(TextPatternRangeEndpoint_Start, TextUnit_Character, start as i32);
        if range_text(&r) == needle {
            chosen = Some((r, "offset"));
        }
    }
    let (range, how) = chosen.ok_or("posizione del testo non ricostruibile via UI Automation")?;
    range.Select().map_err(|e| format!("Select: {e}"))?;
    // Rilettura: la selezione deve essere proprio il testo da sostituire.
    let arr = tp.GetSelection().map_err(|e| format!("GetSelection: {e}"))?;
    let sel = if arr.Length().unwrap_or(0) > 0 {
        arr.GetElement(0).map(|r| range_text(&r)).unwrap_or_default()
    } else {
        String::new()
    };
    if sel != needle {
        return Err(format!("dopo Select la selezione ha {} caratteri invece di {n}", sel.chars().count()));
    }
    Ok(how)
}

fn count(text: &str, needle: &str) -> usize {
    if needle.is_empty() {
        0
    } else {
        text.matches(needle).count()
    }
}

fn read_field_text(pid: u32) -> Option<String> {
    FIELD_WORKER
        .run(UIA_TIMEOUT, move |a| unsafe {
            focused_element(a, pid).ok().and_then(|el| read_full_text(&el, FULL_TEXT_MAX))
        })
        .flatten()
}

/// Sostituisce nel campo di `ctx` il testo `old` (già incollato) con `new`.
/// Con `Err(NotVerifiable)` il campo non è stato toccato.
pub fn replace_pasted_text(ctx: &AppContext, old: &str, new: &str) -> Result<ReplaceOutcome, ReplaceError> {
    use ReplaceError::*;
    let t0 = Instant::now();
    if ctx.is_terminal {
        return Err(NotVerifiable("terminale: niente UI Automation sul testo".into()));
    }
    if old.trim().is_empty() || new.trim().is_empty() {
        return Err(NotVerifiable("testo vuoto".into()));
    }
    let fg = unsafe { GetForegroundWindow() }.0 as isize;
    if ctx.hwnd == 0 || fg != ctx.hwnd {
        return Err(NotVerifiable("in primo piano c'è un'altra finestra".into()));
    }
    let before = read_field_text(ctx.pid).ok_or_else(|| NotVerifiable("campo non leggibile".into()))?;
    let Some(needle) = as_in_field(&before, old) else {
        return Err(NotVerifiable("il campo non contiene più il testo da sostituire".into()));
    };
    let pid = ctx.pid;
    let old_owned = needle.clone();
    let located = FIELD_WORKER
        .run(UIA_TIMEOUT, move |a| unsafe { select_occurrence(a, pid, &old_owned) })
        .ok_or_else(|| NotVerifiable(format!("UI Automation oltre {UIA_TIMEOUT:?}")))?
        .map_err(NotVerifiable)?;

    let paste = paste_text(new, ctx, false);
    if !paste.success {
        return Err(Unverified(format!(
            "incolla non riuscito ({}): il testo di prima è ancora selezionato nel campo",
            paste.reason.clone().unwrap_or_default()
        )));
    }
    // Rilettura: una occorrenza in meno del vecchio, il nuovo presente.
    let (old_n, new_n) = (count(&before, &needle), count(&before, new.trim()));
    for _ in 0..VERIFY_ATTEMPTS {
        std::thread::sleep(VERIFY_DELAY);
        let Some(after) = read_field_text(ctx.pid) else { continue };
        let new_in = as_in_field(&after, new).map(|n| count(&after, &n)).unwrap_or(0);
        let old_ok = new.contains(needle.trim()) || count(&after, &needle) + 1 == old_n;
        if new_in > new_n && old_ok {
            return Ok(ReplaceOutcome {
                elapsed_ms: t0.elapsed().as_millis() as u64,
                located_by: located.to_string(),
                paste,
            });
        }
    }
    Err(Unverified(format!(
        "dopo l'incolla il campo non mostra il testo nuovo al posto del vecchio (incolla {} ms, lettura appunti {:?} ms, trovato con {located})",
        paste.elapsed_ms, paste.clipboard_read_ms
    )))
}

/// Transform sull'ultima dettatura (senza selezione): solo entro
/// `LAST_DICTATION_MAX_AGE` e nella stessa finestra. Ritorna anche il testo
/// sostituito, che serve per l'annulla.
pub fn replace_last_dictation(new: &str) -> Result<(ReplaceOutcome, LastDictation), ReplaceError> {
    let last = last_dictation_within(LAST_DICTATION_MAX_AGE).ok_or_else(|| {
        ReplaceError::NotVerifiable("nessuna dettatura negli ultimi 60 secondi in questa finestra".into())
    })?;
    let out = replace_pasted_text(&last.ctx, &last.text, new)?;
    record_dictation(new, &last.ctx);
    Ok((out, last))
}

/// Annulla di un Transform: rimette `original` al posto di `transformed`, con
/// la stessa verifica.
pub fn undo_transform(ctx: &AppContext, original: &str, transformed: &str) -> Result<ReplaceOutcome, ReplaceError> {
    replace_pasted_text(ctx, transformed, original)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn occorrenza_piu_vicina_prima_del_cursore() {
        let t = "nome prova. nome prova. fine";
        assert_eq!(pick_occurrence(t, "nome prova", Some(28)), Some(12));
        assert_eq!(pick_occurrence(t, "nome prova", Some(11)), Some(0));
        // Cursore dentro la prima: nessuna prima del cursore, si prende l'ultima.
        assert_eq!(pick_occurrence(t, "nome prova", Some(3)), Some(12));
        assert_eq!(pick_occurrence(t, "nome prova", None), Some(12));
        assert_eq!(pick_occurrence(t, "assente", None), None);
        assert_eq!(pick_occurrence("è là, e lì. è là, e lì", "è là, e lì", Some(11)), Some(0));
    }

    #[test]
    fn testo_cercato_con_a_capo_del_campo() {
        assert_eq!(as_in_field("Nota: riga uno\r\nriga due", "riga uno\nriga due").as_deref(), Some("riga uno\r\nriga due"));
        assert_eq!(as_in_field("abc", " abc ").as_deref(), Some("abc"));
        assert_eq!(as_in_field("abc", "xyz"), None);
    }

    #[test]
    fn senza_finestra_o_in_terminale_il_campo_non_si_tocca() {
        let term = AppContext { is_terminal: true, hwnd: 1, ..Default::default() };
        assert!(matches!(replace_pasted_text(&term, "a", "b"), Err(ReplaceError::NotVerifiable(_))));
        let other = AppContext { hwnd: 0x7fff_fff0, ..Default::default() };
        let e = replace_pasted_text(&other, "vecchio", "nuovo").unwrap_err();
        assert!(matches!(e, ReplaceError::NotVerifiable(_)));
        assert!(e.user_message().contains("appunti"), "{}", e.user_message());
        // Nessuna dettatura registrata in questa finestra: niente.
        assert!(matches!(replace_last_dictation("nuovo"), Err(ReplaceError::NotVerifiable(_))));
    }
}
