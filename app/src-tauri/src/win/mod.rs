//! Integrazione Windows di eagleVoice.
//!
//! - `hotkeys`: macchina a stati delle scorciatoie (push-to-talk, hands-free,
//!   annulla, incolla l'ultima, Polish) con debounce fra dettature.
//! - `context`: app, processo, URL e testo del campo attivo via UI Automation,
//!   letti alla pressione del tasto con un tetto di tempo.
//! - `paste`: incolla via appunti a rendering ritardato (riuso di `paste_tx`),
//!   Ctrl+Shift+V nei terminali, ripristino degli appunti, Invio simulato.
//! - `observe`: rilettura del campo dopo l'incolla e parole corrette a mano.
//! - `selection`: testo selezionato (UIA, riserva Ctrl+C).
//! - `commands`: comandi Tauri di diagnostica e configurazione.
//!
//! Tutto ciò che tocca Win32 è compilato solo su Windows; tipi, tabella delle
//! categorie e funzioni sul testo sono comuni (e coperti dai test unitari).

// Finché il modulo non è collegato (lib.rs, actions.rs) quasi tutto
// risulta inutilizzato: TODO-collegare, togliere questa riga dopo il collegamento.
#![allow(dead_code)]

pub mod category;
pub mod commands;
pub mod hotkeys;
pub mod text;

#[cfg(target_os = "windows")]
pub mod context;
#[cfg(target_os = "windows")]
pub(crate) mod keys;
#[cfg(target_os = "windows")]
pub(crate) mod cliptx;
#[cfg(target_os = "windows")]
pub(crate) mod keywatch;
#[cfg(target_os = "windows")]
pub mod observe;
#[cfg(target_os = "windows")]
pub mod paste;
#[cfg(target_os = "windows")]
pub mod replace;
#[cfg(target_os = "windows")]
pub mod selection;
#[cfg(target_os = "windows")]
pub(crate) mod uia;

use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::RwLock;
use tauri::{AppHandle, Emitter};

pub use category::{AppCategory, CategoryRule};

/// Pezzi del testo del campo attivo, ciascuno tagliato a 500 caratteri su
/// confine di parola (prima: la coda vicina al cursore; selezione e dopo: la testa).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, Type)]
pub struct FieldText {
    pub before: String,
    pub selected: String,
    pub after: String,
    pub is_editable: bool,
}

/// Contesto letto alla pressione del tasto.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
pub struct AppContext {
    /// HWND della finestra in primo piano (0 se assente).
    pub hwnd: isize,
    pub process_exe: String,
    pub app_name: String,
    pub window_title: String,
    pub url: Option<String>,
    pub category: AppCategory,
    pub field: Option<FieldText>,
    pub proper_nouns: Vec<String>,
    pub captured_in_ms: u64,
    // Campi in più (non rinominano nulla):
    /// PID del processo in primo piano.
    pub pid: u32,
    /// Classe della finestra in primo piano (serve a riconoscere i terminali).
    pub window_class: String,
    /// La finestra è un terminale: incolla con Ctrl+Shift+V, testo del campo non letto.
    pub is_terminal: bool,
    /// Da dove viene il testo del campo: "text_pattern2", "text_pattern",
    /// "value_pattern", oppure il motivo per cui manca ("password", "terminal",
    /// "timeout", "no_focus", "no_text_pattern", "focus_other_process").
    pub field_source: Option<String>,
}

/// Esito di un incolla.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct PasteOutcome {
    /// Vero solo se l'app bersaglio ha letto gli appunti dopo la scorciatoia
    /// (ricevuta `WM_RENDERFORMAT`).
    pub success: bool,
    pub reason: Option<String>,
    /// Durata complessiva di `paste_text`, dalla chiamata al ritorno.
    pub elapsed_ms: u64,
    pub method: crate::settings::PasteMethod,
    // Estensioni:
    /// Millisecondi fra la scorciatoia inviata e la prima lettura degli appunti.
    pub clipboard_read_ms: Option<u64>,
    pub press_enter_sent: bool,
    /// La finestra bersaglio era stata coperta e l'abbiamo riportata in primo piano.
    pub focus_restored: bool,
}

/// Una parola (o un gruppo di parole consecutive) cambiata a mano dopo l'incolla.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct WordChange {
    pub from: String,
    pub to: String,
}

/// Risultato dell'osservazione dopo l'incolla: payload di
/// `win:observation-ended` (sempre) e di `win:text-corrected` (solo con parole
/// cambiate, testo ritrovato e ancore integre). I campi dopo `ended_by` sono
/// stati aggiunti dopo e hanno un default, quindi i listener di prima leggono ancora.
#[derive(Debug, Clone, Default, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct TextCorrected {
    /// Testo incollato (senza spazi ai bordi).
    pub pasted: String,
    /// Testo corretto: la regione dettata fra le ancore all'ultima lettura
    /// buona (prima dello svuotamento del campo, se c'è stato).
    pub edited: String,
    /// Diff a parole (LCS) fra `pasted` e `edited`; l'allineamento vero lo fa `cleanup::learn`.
    pub changes: Vec<WordChange>,
    pub app_name: String,
    /// Motivo di fine: `next_paste_started`, `next_dictation_started`,
    /// `enter_pressed`, `textbox_emptied`, `anchor_mismatch`,
    /// `edit_distance_exceeded`, `text_too_long`, `observation_window_elapsed`;
    /// senza osservazione: `terminal`, `password_field`, `field_unreadable`,
    /// `pasted_text_not_found`.
    pub ended_by: String,
    /// Fino a 40 caratteri del campo subito prima del testo incollato (vuoto se non c'è).
    pub context_before: String,
    /// Fino a 40 caratteri del campo subito dopo il testo incollato.
    pub context_after: String,
    /// Le ancore erano ancora al loro posto all'ultima lettura: con `false` non si impara.
    pub anchors_intact: bool,
    /// Il testo incollato è stato ritrovato nel campo dopo l'incolla.
    pub verified: bool,
    /// Ultimo tasto premuto durante l'osservazione (es. "Invio", "Ctrl+Z").
    pub last_key: Option<String>,
    pub process_exe: String,
    pub category: AppCategory,
    /// Durata dell'osservazione.
    pub observed_ms: u64,
}

/// Payload dell'evento `win:context`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
pub struct ContextEvent {
    pub app_name: String,
    pub category: AppCategory,
    pub url: Option<String>,
    pub has_field: bool,
}

/// Impostazioni del modulo. Campo `win` di `AppSettings`.
#[derive(Debug, Clone, Serialize, Deserialize, Type)]
#[serde(default)]
pub struct WinSettings {
    /// Maschera il rilascio di Win/Alt con un tasto neutro quando il modulo
    /// inietta tasti (incolla, copia della selezione). Le scorciatoie di
    /// handy-keys mascherano sempre.
    pub win_masking: bool,
    pub debounce_ms: u64,
    /// Doppio tocco: un PTT rilasciato prima di questa soglia aspetta una
    /// seconda pressione entro la stessa durata dalla prima. 0 = disattivato.
    pub double_tap_ms: u64,
    pub context_timeout_ms: u64,
    /// Tetto dell'osservazione dopo l'incolla; si chiude prima sugli eventi
    /// (incolla o dettatura successivi, Invio, campo svuotato, ancore perse).
    pub observe_seconds: u64,
    pub category_table: Vec<CategoryRule>,
    pub terminal_processes: Vec<String>,
}

impl Default for WinSettings {
    fn default() -> Self {
        Self {
            win_masking: true,
            debounce_ms: 500,
            double_tap_ms: hotkeys::DEFAULT_DOUBLE_TAP_MS,
            context_timeout_ms: 150,
            observe_seconds: 60,
            category_table: category::default_category_table(),
            terminal_processes: category::default_terminal_processes(),
        }
    }
}

static SETTINGS: RwLock<Option<WinSettings>> = RwLock::new(None);

/// Impostazioni correnti del modulo (default finché l'app non chiama `apply_settings`).
pub fn current_settings() -> WinSettings {
    SETTINGS
        .read()
        .ok()
        .and_then(|s| s.clone())
        .unwrap_or_default()
}

/// Da chiamare all'avvio e a ogni modifica di `AppSettings.win`.
pub fn apply_settings(settings: WinSettings) {
    hotkeys::set_debounce_ms(settings.debounce_ms);
    hotkeys::set_double_tap_ms(settings.double_tap_ms);
    if let Ok(mut slot) = SETTINGS.write() {
        *slot = Some(settings);
    }
}

type PersistHook = Box<dyn Fn(&AppHandle, &WinSettings) + Send + Sync>;
static PERSIST_HOOK: RwLock<Option<PersistHook>> = RwLock::new(None);

/// L'avvio dell'app registra qui come salvare `WinSettings` in `AppSettings`
/// (lo usa `win_set_category_table`).
pub fn set_persist_hook(hook: impl Fn(&AppHandle, &WinSettings) + Send + Sync + 'static) {
    if let Ok(mut slot) = PERSIST_HOOK.write() {
        *slot = Some(Box::new(hook));
    }
}

pub(crate) fn persist(app: &AppHandle, settings: &WinSettings) {
    if let Ok(slot) = PERSIST_HOOK.read() {
        if let Some(hook) = slot.as_ref() {
            hook(app, settings);
            return;
        }
    }
    log::warn!("[win] nessun persist hook registrato: impostazioni win aggiornate solo in memoria");
}

pub const EVENT_CONTEXT: &str = "win:context";
pub const EVENT_PASTE_OUTCOME: &str = "win:paste-outcome";
pub const EVENT_TEXT_CORRECTED: &str = "win:text-corrected";
/// Ogni osservazione finita, anche senza cambi (per `cleanup::learn` e la diagnostica).
pub const EVENT_OBSERVATION_ENDED: &str = "win:observation-ended";

/// Emette `win:context` con il riassunto del contesto.
pub fn emit_context(app: &AppHandle, ctx: &AppContext) {
    let payload = ContextEvent {
        app_name: ctx.app_name.clone(),
        category: ctx.category,
        url: ctx.url.clone(),
        has_field: ctx.field.is_some(),
    };
    if let Err(e) = app.emit(EVENT_CONTEXT, payload) {
        log::warn!("[win] emit {EVENT_CONTEXT} fallito: {e}");
    }
}

/// Emette `win:paste-outcome`.
pub fn emit_paste_outcome(app: &AppHandle, outcome: &PasteOutcome) {
    if let Err(e) = app.emit(EVENT_PASTE_OUTCOME, outcome.clone()) {
        log::warn!("[win] emit {EVENT_PASTE_OUTCOME} fallito: {e}");
    }
}

/// Emette `win:text-corrected`.
pub fn emit_text_corrected(app: &AppHandle, payload: &TextCorrected) {
    if let Err(e) = app.emit(EVENT_TEXT_CORRECTED, payload.clone()) {
        log::warn!("[win] emit {EVENT_TEXT_CORRECTED} fallito: {e}");
    }
}

/// Emette `win:observation-ended`.
pub fn emit_observation_ended(app: &AppHandle, payload: &TextCorrected) {
    if let Err(e) = app.emit(EVENT_OBSERVATION_ENDED, payload.clone()) {
        log::warn!("[win] emit {EVENT_OBSERVATION_ENDED} fallito: {e}");
    }
}

/// Avvia in anticipo i thread UI Automation, così la prima cattura del
/// contesto non paga la creazione di COM e di `CUIAutomation` (decine di ms).
pub fn warm_up() {
    #[cfg(target_os = "windows")]
    uia::warm_up();
}

#[cfg(test)]
mod tests {
    use super::text::word_changes;
    use super::TextCorrected;
    use crate::cleanup::{Dictionary, TextChange};

    /// Contratto dall'osservazione al database: le parole cambiate che
    /// l'osservazione produce, passate per il JSON dell'evento
    /// `win:text-corrected` e convertite come fa il listener di `flow.rs`,
    /// creano il candidato nel dizionario (database temporaneo).
    #[test]
    fn correzione_osservata_arriva_ai_candidati_del_dizionario() {
        let pasted = "parola uno Clod parola due Nome prova fine";
        let edited = "parola uno Claude parola due NomeProva fine";
        let payload = TextCorrected {
            pasted: pasted.into(),
            edited: edited.into(),
            changes: word_changes(pasted, edited),
            app_name: "prova".into(),
            ended_by: "observation_window_elapsed".into(),
            anchors_intact: true,
            verified: true,
            ..Default::default()
        };
        // come app.emit e poi il listener in flow.rs
        let json = serde_json::to_string(&payload).unwrap();
        let got: TextCorrected = serde_json::from_str(&json).unwrap();
        let changes: Vec<TextChange> = got
            .changes
            .iter()
            .map(|c| TextChange { from: c.from.clone(), to: c.to.clone() })
            .collect();

        let dir = tempfile::tempdir().unwrap();
        let dict = Dictionary::open(&dir.path().join("dictionary.sqlite")).unwrap();
        let (n1, visible1) = dict.observe_correction(&got.pasted, &got.edited, &changes).unwrap();
        assert!(n1 >= 1, "almeno un candidato creato: {n1}");
        assert!(!visible1);
        // l'hub mostra un candidato solo dalla seconda volta (CANDIDATE_MIN_SEEN = 2)
        assert!(dict.candidates().unwrap().is_empty());
        let (_, visible2) = dict.observe_correction(&got.pasted, &got.edited, &changes).unwrap();
        assert!(visible2);
        let cands = dict.candidates().unwrap();
        assert!(
            cands.iter().any(|c| c.wrong == "Clod" && c.right == "Claude" && c.seen_count == 2),
            "candidati: {cands:?}"
        );
    }
}
