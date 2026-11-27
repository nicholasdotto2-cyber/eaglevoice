//! Osservazione dopo l'incolla.
//!
//! Subito dopo l'incolla legge il campo col fuoco e cerca il testo incollato
//! (verifica): da lì ricava due ancore, i 40 caratteri prima e dopo. Poi
//! rilegge lo stesso elemento ogni 500 ms, e 100 ms dopo ogni tasto, fino a
//! `observe_seconds` (60 s di default); il testo fra le ancore è la versione
//! corretta a mano. Si chiude prima sugli eventi: incolla
//! successivo, dettatura successiva, Invio (senza Maiusc), campo svuotato,
//! ancore perse, modifica troppo grande o testo troppo lungo. Del campo si
//! tiene l'ultima istantanea buona: in Claude e ChatGPT Invio manda il
//! messaggio e svuota il campo, e la correzione è proprio lì.
//! Niente osservazione nei terminali e nei campi password.
//!
//! A fine osservazione emette sempre `win:observation-ended`, e
//! `win:text-corrected` (lo consuma il dizionario) solo se ci sono parole
//! cambiate, il testo era stato ritrovato e le ancore sono integre.

use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use tauri::AppHandle;
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Accessibility::IUIAutomationElement;

use super::context::{focused_element, read_full_text};
use super::hotkeys::DICTATION_STARTS;
use super::keywatch;
use super::paste::PASTE_GENERATION;
use super::text::{edit_distance, find_anchors, region_between, word_changes, Anchors};
use super::uia::create_automation;
use super::{AppContext, TextCorrected};

const FULL_TEXT_MAX: i32 = 20_000;
const BASELINE_DELAY: Duration = Duration::from_millis(150);
const BASELINE_ATTEMPTS: u32 = 4;
const MAX_READ_FAILURES: u32 = 3;
/// Passo del ciclo: controlla eventi e tasti.
const TICK: Duration = Duration::from_millis(30);
/// Rilettura dopo un tasto (l'utente sta correggendo).
const READ_AFTER_KEY: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy)]
pub struct ObserveOptions {
    /// Rilettura del campo senza tasti premuti.
    pub interval: Duration,
    /// Tetto dell'osservazione.
    pub window: Duration,
}

impl ObserveOptions {
    pub fn from_settings() -> Self {
        Self {
            interval: Duration::from_millis(500),
            window: Duration::from_secs(super::current_settings().observe_seconds),
        }
    }
}

/// Esito dell'osservazione: `verified` dice se il testo incollato è stato
/// ritrovato nel campo dopo l'incolla (uguale a `corrected.verified`).
#[derive(Debug, Clone)]
pub struct ObserveResult {
    pub verified: bool,
    pub corrected: TextCorrected,
}

fn edited_too_much(pasted: &str, edited: &str) -> bool {
    let limit = pasted.chars().count() / 2 + 10;
    edit_distance(pasted, edited) > limit
}

/// La regione è cresciuta troppo per essere una correzione (si controlla
/// prima della distanza di modifica, che su testi lunghi costa).
fn too_long(pasted: &str, region: &str) -> bool {
    region.chars().count() > pasted.chars().count() * 2 + 200
}

/// Stato puro dell'osservazione: riceve le letture del campo e dice quando
/// chiudere. L'ultima regione buona resta in `last_edited`.
#[derive(Debug, Clone)]
pub struct Tracker {
    pasted: String,
    anchors: Anchors,
    pub last_edited: String,
    pub anchors_intact: bool,
}

impl Tracker {
    pub fn new(pasted: &str, anchors: Anchors) -> Self {
        Self {
            pasted: pasted.to_string(),
            anchors,
            last_edited: pasted.to_string(),
            anchors_intact: true,
        }
    }

    /// Una lettura del campo; `Some(motivo)` se l'osservazione deve chiudersi.
    /// Una lettura che chiude non sostituisce l'istantanea buona.
    pub fn on_read(&mut self, text: &str) -> Option<&'static str> {
        if text.trim().is_empty() {
            return Some("textbox_emptied");
        }
        let Some(region) = region_between(text, &self.anchors) else {
            self.anchors_intact = false;
            return Some("anchor_mismatch");
        };
        if too_long(&self.pasted, &region) {
            return Some("text_too_long");
        }
        if edited_too_much(&self.pasted, &region) {
            return Some("edit_distance_exceeded");
        }
        self.last_edited = region;
        None
    }
}

/// Osservazione bloccante (gira sul thread del chiamante, con COM proprio).
pub fn observe_blocking(ctx: &AppContext, pasted: &str, opts: ObserveOptions) -> ObserveResult {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
    }
    let result = observe_inner(ctx, pasted, opts);
    unsafe { CoUninitialize() };
    result
}

unsafe fn is_password(el: &IUIAutomationElement) -> bool {
    el.CurrentIsPassword().map(|b| b.as_bool()).unwrap_or(false)
}

fn observe_inner(ctx: &AppContext, pasted: &str, opts: ObserveOptions) -> ObserveResult {
    let paste_gen = PASTE_GENERATION.load(Ordering::SeqCst);
    let dictation_gen = DICTATION_STARTS.load(Ordering::SeqCst);
    let t0 = Instant::now();
    let pasted_trim = pasted.trim().to_string();
    let mut out = TextCorrected {
        pasted: pasted_trim.clone(),
        edited: pasted_trim.clone(),
        app_name: ctx.app_name.clone(),
        process_exe: ctx.process_exe.clone(),
        category: ctx.category,
        ended_by: "field_unreadable".into(),
        ..Default::default()
    };
    let finish = |mut o: TextCorrected| {
        o.observed_ms = t0.elapsed().as_millis() as u64;
        ObserveResult { verified: o.verified, corrected: o }
    };
    if ctx.is_terminal {
        out.ended_by = "terminal".into();
        return finish(out);
    }
    if ctx.field_source.as_deref() == Some("password") {
        out.ended_by = "password_field".into();
        return finish(out);
    }
    let Ok(automation) = create_automation() else {
        return finish(out);
    };
    // L'hook dei tasti si accende prima della lettura di base: un Invio
    // premuto subito dopo l'incolla deve contare.
    let watch = keywatch::start();
    let keys0 = keywatch::snapshot();

    // Lettura di base: elemento col fuoco e posizione del testo incollato.
    let hint = ctx.field.as_ref().map(|f| f.before.as_str());
    let mut baseline: Option<(IUIAutomationElement, Anchors)> = None;
    let mut found_text = false;
    for _ in 0..BASELINE_ATTEMPTS {
        std::thread::sleep(BASELINE_DELAY);
        let read = unsafe {
            focused_element(&automation, ctx.pid).ok().and_then(|el| {
                if is_password(&el) {
                    return Some((el, None));
                }
                let t = read_full_text(&el, FULL_TEXT_MAX);
                t.map(|t| (el, Some(t)))
            })
        };
        match read {
            Some((_, None)) => {
                out.ended_by = "password_field".into();
                return finish(out);
            }
            Some((el, Some(text))) => {
                found_text = true;
                if let Some(anchors) = find_anchors(&text, &pasted_trim, hint) {
                    baseline = Some((el, anchors));
                    break;
                }
            }
            None => {}
        }
    }
    let Some((element, anchors)) = baseline else {
        out.ended_by = if found_text { "pasted_text_not_found" } else { "field_unreadable" }.into();
        return finish(out);
    };
    out.verified = true;
    out.context_before = anchors.before.clone();
    out.context_after = anchors.after.clone();

    let mut tracker = Tracker::new(&pasted_trim, anchors);
    let mut failures = 0u32;
    let mut last_read = Instant::now();
    let mut keys_at_read = keywatch::snapshot();
    let mut last_key_change: Option<Instant> = None;
    let ended_by: &'static str = loop {
        std::thread::sleep(TICK);
        if PASTE_GENERATION.load(Ordering::SeqCst) != paste_gen {
            break "next_paste_started";
        }
        if DICTATION_STARTS.load(Ordering::SeqCst) != dictation_gen {
            break "next_dictation_started";
        }
        let keys = keywatch::snapshot();
        let enter = keys.enter_seq != keys0.enter_seq;
        if keys.seq != keys_at_read.seq && last_key_change.is_none() {
            last_key_change = Some(Instant::now());
        }
        let due = enter
            || last_read.elapsed() >= opts.interval
            || last_key_change.is_some_and(|t| t.elapsed() >= READ_AFTER_KEY);
        if !due {
            continue;
        }
        last_read = Instant::now();
        keys_at_read = keys;
        last_key_change = None;
        match unsafe { read_full_text(&element, FULL_TEXT_MAX) } {
            None => {
                failures += 1;
                if failures >= MAX_READ_FAILURES {
                    break "field_unreadable";
                }
            }
            Some(text) => {
                failures = 0;
                // Dopo Invio la lettura vale solo se il campo non è già stato
                // svuotato; in ogni caso il motivo è l'Invio.
                let stop = tracker.on_read(&text);
                if enter {
                    break "enter_pressed";
                }
                if let Some(reason) = stop {
                    break reason;
                }
            }
        }
        if enter {
            break "enter_pressed";
        }
        if t0.elapsed() >= opts.window {
            break "observation_window_elapsed";
        }
    };
    let keys_end = keywatch::snapshot();
    out.last_key = (keys_end.seq != keys0.seq).then(|| keys_end.last_key()).flatten();
    drop(watch);
    out.changes = word_changes(&pasted_trim, &tracker.last_edited);
    out.edited = tracker.last_edited;
    out.anchors_intact = tracker.anchors_intact;
    out.ended_by = ended_by.into();
    finish(out)
}

/// L'osservazione è buona per il dizionario: testo ritrovato, ancore integre,
/// almeno una parola cambiata.
pub fn worth_learning(c: &TextCorrected) -> bool {
    c.verified && c.anchors_intact && !c.changes.is_empty()
}

/// Avvia l'osservazione in background. A fine osservazione emette
/// `win:observation-ended` e, se `worth_learning`, `win:text-corrected`.
/// Registra anche la dettatura per i Transforms senza selezione (`replace`).
pub fn observe_after_paste(app: &AppHandle, ctx: &AppContext, pasted: String) {
    super::replace::record_dictation(&pasted, ctx);
    let app = app.clone();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("win-observe".into())
        .spawn(move || {
            let res = observe_blocking(&ctx, &pasted, ObserveOptions::from_settings());
            log::info!(
                "[win] osservazione finita: verificato={} motivo={} ancore_integre={} cambi={} ultimo_tasto={:?} ms={}",
                res.verified,
                res.corrected.ended_by,
                res.corrected.anchors_intact,
                res.corrected.changes.len(),
                res.corrected.last_key,
                res.corrected.observed_ms
            );
            super::emit_observation_ended(&app, &res.corrected);
            if worth_learning(&res.corrected) {
                super::emit_text_corrected(&app, &res.corrected);
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tracker(field: &str, pasted: &str) -> Tracker {
        let a = find_anchors(field, pasted, None).unwrap();
        Tracker::new(pasted, a)
    }

    #[test]
    fn svuotamento_tiene_l_ultima_istantanea() {
        // Claude: il campo contiene solo la dettatura, Invio lo svuota.
        let pasted = "parola uno Clod parola due Nome prova fine";
        let mut t = tracker(pasted, pasted);
        assert_eq!(t.on_read("parola uno Claude parola due Nome prova fine"), None);
        assert_eq!(t.on_read(""), Some("textbox_emptied"));
        assert_eq!(t.last_edited, "parola uno Claude parola due Nome prova fine");
        assert!(t.anchors_intact);
    }

    #[test]
    fn ancore_perse_non_integre() {
        let field = "Inizio: campo di prova. parola uno Clod fine. Chiusura, Nome";
        let mut t = tracker(field, "parola uno Clod fine.");
        assert_eq!(t.on_read("Inizio: campo di prova. parola uno Claude fine. Chiusura, Nome"), None);
        // L'utente riscrive l'inizio del campo: l'ancora prima non c'è più.
        assert_eq!(t.on_read("Tutt'altro testo parola uno Claude fine. Chiusura, Nome"), Some("anchor_mismatch"));
        assert!(!t.anchors_intact);
        assert_eq!(t.last_edited, "parola uno Claude fine.");
    }

    #[test]
    fn riscrittura_e_testo_lungo_chiudono() {
        let pasted = "testo di prova numero uno per la verifica";
        let mut t = tracker(pasted, pasted);
        assert_eq!(t.on_read("frase diversa scritta da capo con altre parole ancora"), Some("edit_distance_exceeded"));
        assert_eq!(t.last_edited, pasted, "la riscrittura non entra nell'istantanea");
        let lungo = format!("{pasted} {}", "e poi altro testo ".repeat(30));
        assert_eq!(t.on_read(&lungo), Some("text_too_long"));
    }

    #[test]
    fn imparabile_solo_con_ancore_integre_e_cambi() {
        let mut c = TextCorrected {
            verified: true,
            anchors_intact: true,
            changes: word_changes("Clod", "Claude"),
            ..Default::default()
        };
        assert!(worth_learning(&c));
        c.anchors_intact = false;
        assert!(!worth_learning(&c));
        c.anchors_intact = true;
        c.changes.clear();
        assert!(!worth_learning(&c));
    }

    #[test]
    fn niente_osservazione_in_terminali_e_password() {
        let opts = ObserveOptions { interval: Duration::from_millis(10), window: Duration::from_millis(10) };
        let term = AppContext { is_terminal: true, ..Default::default() };
        let r = observe_blocking(&term, "git status", opts);
        assert_eq!(r.corrected.ended_by, "terminal");
        assert!(!r.verified);
        let pw = AppContext { field_source: Some("password".into()), ..Default::default() };
        assert_eq!(observe_blocking(&pw, "segreto", opts).corrected.ended_by, "password_field");
    }
}
