//! Motore di trascrizione dell'app: whisper.cpp esterno sulla GPU come
//! motore principale, transcribe-cpp di Handy in-process come riserva.
//!
//! Interfaccia verso il resto dell'app: una sessione per dettatura
//! (`start_session`, `feed`, `finish`, `cancel`), testo grezzo in uscita
//! (`RawTranscript`), vocabolario e lingua in ingresso (`SessionConfig`).
//! I parametri vengono dalle prove sul motore: turbo Q8_0, prompt dal
//! dizionario, pezzi da 25 s tagliati sulle pause del VAD, VAD sempre acceso,
//! lingua forzata a `it`.

use std::sync::Arc;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use specta::Type;

pub mod commands;
pub mod filtri;
pub mod handy_inprocess;
mod http_locale;
pub mod microfoni;
pub mod pezzi;
pub mod prompt;
mod riscaldamento;
pub mod selettore;
pub mod sessione;
pub mod whisper_external;

pub use selettore::FallbackEngine;

/// Radice del progetto principale (due livelli sopra `app\src-tauri`): binari e modelli
/// stanno qui (fuori da git).
pub const PROJECT_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), r"\..\..");

pub const EVENT_ENGINE_STATE: &str = "asr:engine-state";
pub const EVENT_PARTIAL_TEXT: &str = "asr:partial-text";
pub const EVENT_LANGUAGE_DETECTED: &str = "asr:language-detected";
pub const EVENT_DURATION_WARNING: &str = "asr:duration-warning";
/// Parte quando la registrazione tocca il tetto.
pub const EVENT_DURATION_LIMIT: &str = "asr:duration-limit";

/// Campioni a 16 kHz mono.
pub const SAMPLE_RATE: usize = 16_000;

/// Fabbrica di un VAD nuovo per ogni sessione (lo stato del Silero non si
/// eredita fra due dettature).
pub type ScorerFactory =
    Arc<dyn Fn() -> Result<Box<dyn crate::audio_toolkit::SpeechScorer>> + Send + Sync>;

/// Ingresso di una sessione: vocabolario già unito dal chiamante
/// (dizionario, parole stellate, nomi propri dal contesto) in ordine di
/// importanza, il primo è il più importante; lingua forzata o `None` per la
/// lingua principale delle impostazioni.
#[derive(Debug, Clone, Default)]
pub struct SessionConfig {
    /// GIÀ ORDINATO dal chiamante: il primo termine è il più importante e
    /// finisce in coda al prompt di Whisper; il modulo non riordina, taglia
    /// solo i meno importanti quando il prompt supera il tetto di token.
    pub vocabulary: Vec<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
pub struct Segment {
    pub start_ms: u32,
    pub end_ms: u32,
    pub text: String,
}

/// Testo grezzo di una dettatura: i pezzi uniti con uno spazio.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Default)]
pub struct RawTranscript {
    pub text: String,
    pub language: Option<String>,
    pub segments: Vec<Segment>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum EngineStatus {
    #[default]
    Stopped,
    Starting,
    Ready,
    Error,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Type, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum AsrEngineKind {
    #[default]
    WhisperExternal,
    HandyInprocess,
}

/// Stato del motore, uguale al payload di `asr:engine-state`. Il testo
/// dell'errore sta in `detail`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq, Default)]
pub struct AsrState {
    pub state: EngineStatus,
    pub detail: Option<String>,
    pub model: Option<String>,
    pub vram_mb: Option<u32>,
    /// Quale motore risponde e su quale porta.
    pub engine: AsrEngineKind,
    pub port: Option<u16>,
}

/// Impostazioni del modulo, campo `asr` di `AppSettings`.
#[derive(Debug, Clone, Serialize, Deserialize, Type, PartialEq)]
#[serde(default)]
pub struct AsrSettings {
    pub engine: AsrEngineKind,
    pub whisper_model_path: String,
    pub whisper_bin_dir: String,
    /// Modello Silero per il `--vad` di whisper-server.
    pub whisper_vad_model_path: String,
    pub languages: Vec<String>,
    pub primary_language: String,
    /// Rilevamento della lingua sul primo pezzo fra `languages`. Non ancora
    /// provato: spento.
    pub language_detection: bool,
    /// Pezzi decodificati durante la registrazione.
    pub chunked_decode: bool,
    pub chunk_max_seconds: u32,
    /// Se maggiore di zero, chiude un pezzo anche prima del massimo quando il
    /// VAD vede un silenzio di almeno questa durata. Non misurato: spento.
    pub chunk_min_silence_ms: u32,
    /// Sotto questa quantità di parlato il pezzo non va al motore.
    pub min_speech_ms: u32,
    /// Testo dei pezzi precedenti nel prompt. Non misurato: spento.
    pub carry_prompt: bool,
    pub max_recording_minutes: u32,
    pub prompt_max_tokens: u32,
    /// Microfoni in ordine di preferenza (nomi dei dispositivi), usati quando
    /// non c'è un microfono forzato: vince il primo presente all'apertura.
    pub ranked_microphones: Vec<String>,
}

impl Default for AsrSettings {
    fn default() -> Self {
        Self {
            engine: AsrEngineKind::WhisperExternal,
            whisper_model_path: format!(r"{PROJECT_ROOT}\models\whisper\ggml-large-v3-turbo-q8_0.bin"),
            whisper_bin_dir: format!(r"{PROJECT_ROOT}\bin\whisper"),
            whisper_vad_model_path: format!(r"{PROJECT_ROOT}\models\whisper\ggml-silero-v5.1.2.bin"),
            languages: vec!["it".into(), "en".into()],
            primary_language: "it".into(),
            language_detection: false,
            chunked_decode: true,
            chunk_max_seconds: 25,
            chunk_min_silence_ms: 0,
            min_speech_ms: 300,
            carry_prompt: false,
            max_recording_minutes: 20,
            prompt_max_tokens: 224,
            ranked_microphones: Vec::new(),
        }
    }
}

/// Uscita degli eventi verso il frontend; finta nei test.
pub trait AsrEvents: Send + Sync {
    fn emit_json(&self, event: &str, payload: serde_json::Value);
}

impl AsrEvents for tauri::AppHandle {
    fn emit_json(&self, event: &str, payload: serde_json::Value) {
        use tauri::Emitter;
        if let Err(e) = self.emit(event, payload) {
            log::debug!("asr: evento {event} non emesso: {e}");
        }
    }
}

/// Nessun evento (prove, CLI).
pub struct NoEvents;

impl AsrEvents for NoEvents {
    fn emit_json(&self, _event: &str, _payload: serde_json::Value) {}
}

pub(crate) fn emit_state(events: &dyn AsrEvents, state: &AsrState) {
    events.emit_json(
        EVENT_ENGINE_STATE,
        serde_json::to_value(state).unwrap_or(serde_json::Value::Null),
    );
}

/// Un motore di trascrizione con una sessione per dettatura.
pub trait AsrEngine: Send + Sync {
    /// Apre la sessione (chiude quella precedente, se c'è). Non blocca.
    fn start_session(&self, cfg: SessionConfig) -> Result<()>;
    /// Campioni 16 kHz mono f32 in arrivo dal recorder. Non blocca.
    fn feed(&self, samples: &[f32]);
    /// Chiude la sessione: decodifica l'ultimo pezzo e restituisce il testo.
    fn finish(&self) -> Result<RawTranscript>;
    fn cancel(&self);
    fn is_ready(&self) -> bool;
    fn state(&self) -> AsrState;
    /// Trascrive un audio intero fuori dalla sessione (file, "ritenta").
    fn transcribe_clip(&self, samples: Vec<f32>, cfg: SessionConfig) -> Result<RawTranscript>;
    /// Termini in più per la sessione aperta (nomi propri letti a schermo),
    /// validi per i pezzi non ancora decodificati. Nel prompt vanno prima del
    /// vocabolario d'apertura, che resta in coda; stesso tetto di token,
    /// doppioni scartati. Senza sessione aperta non fa nulla.
    fn add_session_vocabulary(&self, _extra: Vec<String>) {}
    /// Riscalda la GPU in un thread in background (1 s di rumore, lingua
    /// "it", VAD spento). Non blocca. `true` se è partito; `false` se il
    /// motore non è pronto, se ne gira già uno o se una richiesta vera è in
    /// corso (la GPU sta già lavorando). whisper-server serve una richiesta
    /// alla volta: una richiesta vera che arriva durante il riscaldamento
    /// aspetta la sua fine (circa 100 ms da caldo). Il motore di riserva non
    /// lo usa: `false`.
    fn rewarm(&self) -> bool {
        false
    }
    /// Riavvia il motore (processo esterno). Non blocca.
    fn restart(&self) -> Result<()> {
        Ok(())
    }
    /// Arresto alla chiusura dell'app.
    fn shutdown(&self) {}
}

pub type SharedAsr = Arc<dyn AsrEngine>;

/// Sceglie il motore e lo avvia. Va chiamato dopo che `TranscriptionManager`
/// è nello stato di Tauri, così la riserva c'è.
pub fn build_engine(settings: &AsrSettings, app: &tauri::AppHandle) -> SharedAsr {
    use crate::managers::transcription::TranscriptionManager;
    use tauri::Manager;

    let events: Arc<dyn AsrEvents> = Arc::new(app.clone());
    let backup: Option<SharedAsr> = app
        .try_state::<Arc<TranscriptionManager>>()
        .map(|tm| {
            Arc::new(handy_inprocess::HandyInprocess::new(Arc::clone(tm.inner()))) as SharedAsr
        });
    if backup.is_none() {
        log::warn!("asr: TranscriptionManager assente, nessun motore di riserva");
    }
    let primary: Option<SharedAsr> = match settings.engine {
        AsrEngineKind::WhisperExternal => {
            let app_vad = app.clone();
            let make_scorer: ScorerFactory = Arc::new(move || {
                crate::managers::audio::create_detector(
                    crate::settings::VadBackend::Silero,
                    &app_vad,
                )
            });
            Some(whisper_external::WhisperExternal::start(
                settings.clone(),
                Arc::clone(&events),
                Some(make_scorer),
            ) as SharedAsr)
        }
        AsrEngineKind::HandyInprocess => None,
    };
    Arc::new(FallbackEngine::new(primary, backup, events))
}
