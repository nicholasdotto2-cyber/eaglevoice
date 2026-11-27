//! Catena della dettatura di eagleVoice.
//!
//! Scorciatoia (`win::hotkeys`) → contesto del campo (`win::context`) → microfono
//! (`AudioRecordingManager`, i campioni vanno all'ASR tramite il tap) → testo grezzo
//! (`asr`) → pulizia e dizionario (`cleanup`) → incolla (`win::paste`) →
//! osservazione delle correzioni (`win::observe`, poi `cleanup::observe_correction`).
//!
//! Ogni dettatura scrive una riga in `latenze.csv` nella cartella dati dell'app:
//! serve a controllare che il testo arrivi nel campo entro 2 s dal rilascio.

use std::io::Write as _;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use log::{debug, error, info, warn};
use once_cell::sync::Lazy;
use serde::Serialize;
use specta::Type;
use tauri::{AppHandle, Emitter, Listener, Manager};

use crate::asr::{SessionConfig, SharedAsr};
use crate::audio_feedback::{play_feedback_sound, SoundType};
use crate::audio_toolkit::VadPolicy;
use crate::cleanup::{CleanupContext, CleanupManager};
use crate::managers::audio::AudioRecordingManager;
use crate::managers::history::HistoryManager;
use crate::settings::get_settings;
use crate::tray::{set_tray_state, TrayIconState};
use crate::win::hotkeys::DictationIntent;
use crate::win::AppContext;

/// Id del binding usato verso il recorder (come la dettatura di Handy).
const BINDING: &str = "transcribe";
pub const EVENT_STATE: &str = "flow:state";
pub const EVENT_PARTIAL: &str = "flow:partial-text";
/// Quanto resta visibile la conferma "Incollato" prima di nascondere la barra.
const PASTED_LINGER: Duration = Duration::from_millis(1400);

#[derive(Debug, Clone, Serialize, Type)]
pub struct FlowStateEvent {
    pub state: String,
    pub detail: Option<String>,
    pub since_ms: u64,
}

/// `flow:dictation-stats`: parole, secondi di audio e parole al minuto della dettatura appena incollata.
pub const EVENT_DICTATION_STATS: &str = "flow:dictation-stats";

#[derive(Debug, Clone, Serialize, Type)]
pub struct DictationStatsEvent {
    pub words: u32,
    pub audio_secs: f64,
    pub wpm: Option<u32>,
}

/// `flow:paste-failed`: l'incolla non è riuscito; la barra mostra il testo da copiare.
pub const EVENT_PASTE_FAILED: &str = "flow:paste-failed";

#[derive(Debug, Clone, Serialize, Type)]
pub struct PasteFailedEvent {
    pub text: String,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Type)]
pub struct PartialTextEvent {
    pub text: String,
}

/// `flow:transform`: un Transform è partito, finito o fallito. `changes` conta i gruppi
/// di parole cambiate; `message` dice perché è fallito.
pub const EVENT_TRANSFORM: &str = "flow:transform";

#[derive(Debug, Clone, Serialize, Type)]
pub struct TransformEvent {
    pub state: String,
    pub name: String,
    pub changes: u32,
    pub before: String,
    pub after: String,
    pub message: Option<String>,
}

/// Ultimo Transform scritto nel campo: serve ad Annulla e Riprova della barra.
#[derive(Clone)]
struct AppliedTransform {
    slot: u8,
    name: String,
    ctx: AppContext,
    original: String,
    transformed: String,
    /// Fatto sull'ultima dettatura (non su una selezione): dopo Annulla o Riprova
    /// l'ultima dettatura registrata segue il testo nel campo.
    on_last_dictation: bool,
}

static LAST_TRANSFORM: Lazy<Mutex<Option<AppliedTransform>>> = Lazy::new(|| Mutex::new(None));

struct Session {
    started: Instant,
    hands_free: bool,
    context: Arc<Mutex<Option<AppContext>>>,
    context_thread: Option<std::thread::JoinHandle<()>>,
    /// Audio da file (`--dictate-file`) al posto del microfono.
    file_samples: Option<Vec<f32>>,
}

static SESSION: Lazy<Mutex<Option<Session>>> = Lazy::new(|| Mutex::new(None));
/// Serializza i passi della catena: una dettatura alla volta.
static CHAIN: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));
static LAST_TEXT: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));
/// Lingua rilevata dall'ASR sull'ultima dettatura.
static LAST_LANGUAGE: Lazy<Mutex<Option<String>>> = Lazy::new(|| Mutex::new(None));
/// Ultimo uso della GPU (dettatura o riscaldamento). Dopo ore di pausa ASR e pulizia ripartono
/// fredde: qualche secondo in più sulla prima dettatura.
static LAST_GPU_USE: Lazy<Mutex<Instant>> = Lazy::new(|| Mutex::new(Instant::now()));
/// Pausa dopo la quale una nuova dettatura riscalda subito ASR e pulizia, mentre si parla.
const REWARM_ON_START_AFTER: Duration = Duration::from_secs(10 * 60);
/// Con l'app ferma, riscaldamento periodico: la GPU non arriva mai fredda alla dettatura.
const REWARM_IDLE_EVERY: Duration = Duration::from_secs(20 * 60);

/// Riscalda whisper-server e llama-server in background (non blocca). `why` va nel log.
fn rewarm_engines(app: &AppHandle, why: &str) {
    let asr = app.try_state::<SharedAsr>().map(|a| a.rewarm()).unwrap_or(false);
    let cleanup = app
        .try_state::<Arc<CleanupManager>>()
        .map(|m| m.rewarm())
        .unwrap_or(false);
    if !(asr || cleanup) {
        // Motori in riavvio o riscaldamento già in corso: il timer ritenta al giro dopo.
        debug!("[flow] riscaldamento ({why}) non partito");
        return;
    }
    *LAST_GPU_USE.lock().unwrap() = Instant::now();
    info!("[flow] riscaldamento ({why}): asr {asr}, pulizia {cleanup}");
}

/// Segna l'uso della GPU quando la catena finisce (trascrizione finale e pulizia comprese):
/// la pausa si misura dall'ultimo lavoro, non dalla pressione del tasto.
struct GpuUseMark;

impl Drop for GpuUseMark {
    fn drop(&mut self) {
        if let Ok(mut t) = LAST_GPU_USE.lock() {
            *t = Instant::now();
        }
    }
}

fn emit_state(app: &AppHandle, state: &str, detail: Option<String>, since: Instant) {
    let payload = FlowStateEvent {
        state: state.to_string(),
        detail,
        since_ms: since.elapsed().as_millis() as u64,
    };
    if let Err(e) = app.emit(EVENT_STATE, payload) {
        debug!("[flow] evento {EVENT_STATE} non emesso: {e}");
    }
}

fn category_str(ctx: &AppContext) -> String {
    serde_json::to_value(&ctx.category)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_else(|| "other".to_string())
}

/// Da chiamare una volta all'avvio, dopo che asr, cleanup e recorder sono nello stato.
pub fn init(app: &AppHandle) {
    let bar_app = app.clone();
    std::thread::spawn(move || {
        // L'overlay viene creato durante l'avvio: si aspetta che esista.
        std::thread::sleep(Duration::from_millis(1500));
        if SESSION.lock().unwrap().is_none() {
            rest_bar(&bar_app);
        }
    });
    // Il sink gira sul thread delle scorciatoie o del timer del doppio tocco: il lavoro va
    // altrove. Un thread solo esegue in ordine gli intenti brevi, così lo scarto del tocco
    // singolo (DiscardTap, dal timer) finisce prima della Start successiva (dall'hook).
    let (tx, rx) = std::sync::mpsc::channel::<DictationIntent>();
    let worker_app = app.clone();
    std::thread::Builder::new()
        .name("flow-intents".into())
        .spawn(move || {
            for intent in rx {
                match intent {
                    DictationIntent::Start { .. }
                    | DictationIntent::LockHandsFree
                    | DictationIntent::Cancel
                    | DictationIntent::DiscardTap => handle_intent(&worker_app, intent),
                    _ => {
                        let app = worker_app.clone();
                        std::thread::Builder::new()
                            .name("flow".into())
                            .spawn(move || handle_intent(&app, intent))
                            .ok();
                    }
                }
            }
        })
        .ok();
    let tx = Mutex::new(tx);
    crate::win::hotkeys::install_full(move |_app, intent| {
        if let Ok(tx) = tx.lock() {
            let _ = tx.send(intent);
        }
    });

    // Tetto di 20 minuti: lo stop arriva come un rilascio del tasto.
    if let Some(rm) = app.try_state::<Arc<AudioRecordingManager>>() {
        let minutes = get_settings(app).asr.max_recording_minutes;
        let h = app.clone();
        rm.set_duration_limit(
            minutes,
            Some(Arc::new(move || {
                let app = h.clone();
                std::thread::spawn(move || handle_intent(&app, DictationIntent::Stop));
            })),
        );
    }

    // Correzioni osservate dopo l'incolla → parole imparate nel dizionario.
    let h = app.clone();
    app.listen(crate::win::EVENT_TEXT_CORRECTED, move |event| {
        let payload = match serde_json::from_str::<crate::win::TextCorrected>(event.payload()) {
            Ok(p) => p,
            Err(e) => {
                warn!("[flow] correzione osservata non leggibile: {e}");
                return;
            }
        };
        let Some(mgr) = h.try_state::<Arc<CleanupManager>>().map(|m| Arc::clone(&m)) else {
            return;
        };
        let obs = crate::cleanup::learn::Observation::from_text_corrected(&payload);
        tauri::async_runtime::spawn(async move {
            let learned = mgr.learn_from_observation(&obs).await;
            info!(
                "[flow] correzioni osservate in {}: {} parole imparate",
                obs.app_name,
                learned.iter().filter(|l| l.new).count()
            );
        });
    });

    // Candidati del vecchio meccanismo: passano dai filtri e dal classificatore una volta,
    // appena il motore locale è pronto (quelli senza risposta restano per il prossimo avvio).
    let mig_app = app.clone();
    std::thread::spawn(move || {
        for _ in 0..150 {
            std::thread::sleep(Duration::from_secs(2));
            let Some(mgr) = mig_app.try_state::<Arc<CleanupManager>>().map(|m| Arc::clone(&m)) else {
                continue;
            };
            if mgr.state().state == crate::cleanup::EngineStatus::Ready {
                tauri::async_runtime::block_on(mgr.migrate_pending_candidates());
                return;
            }
        }
        warn!("[flow] motore di pulizia non pronto dopo 5 minuti: migrazione dei candidati rimandata");
    });
    // Riscaldamento periodico con l'app ferma (vedi REWARM_IDLE_EVERY).
    let warm_app = app.clone();
    std::thread::spawn(move || loop {
        std::thread::sleep(Duration::from_secs(60));
        let idle = LAST_GPU_USE.lock().unwrap().elapsed();
        // CHAIN occupata: stop() o Polish stanno ancora trascrivendo o pulendo.
        if idle >= REWARM_IDLE_EVERY
            && SESSION.lock().unwrap().is_none()
            && CHAIN.try_lock().is_ok()
        {
            rewarm_engines(&warm_app, "app ferma");
        }
    });
    info!("[flow] catena della dettatura pronta");
}

pub fn handle_intent(app: &AppHandle, intent: DictationIntent) {
    debug!("[flow] intento {intent:?}");
    match intent {
        DictationIntent::Start { hands_free } => start(app, hands_free),
        DictationIntent::LockHandsFree => {
            if let Some(s) = SESSION.lock().unwrap().as_mut() {
                s.hands_free = true;
                emit_state(app, "listening", Some("hands_free".into()), s.started);
            }
        }
        DictationIntent::Stop => stop(app),
        DictationIntent::Cancel => {
            cancel(app);
        }
        DictationIntent::DiscardTap => {
            discard_session(app, "tocco breve senza seconda pressione", Some("tap"), false);
        }
        DictationIntent::PasteLast => paste_last(app),
        DictationIntent::Transform { slot } => transform_hotkey(app, slot),
    }
}

/// Testo grezzo di un audio già registrato (riprova dalla cronologia): stesso
/// motore e stesso vocabolario della dettatura.
pub fn retranscribe_raw(app: &AppHandle, samples: Vec<f32>) -> Result<String, String> {
    let asr = app
        .try_state::<SharedAsr>()
        .ok_or_else(|| "motore ASR assente".to_string())?;
    let settings = get_settings(app);
    let vocabulary = app
        .try_state::<Arc<CleanupManager>>()
        .map(|m| m.vocabulary_for_asr())
        .unwrap_or_default();
    let language = (!settings.asr.language_detection)
        .then(|| settings.asr.primary_language.clone())
        .filter(|l| !l.is_empty());
    let raw = asr
        .transcribe_clip(samples, SessionConfig { vocabulary, language })
        .map_err(|e| e.to_string())?
        .text
        .trim()
        .to_string();
    if raw.is_empty() {
        return Err("Recording contains no speech".to_string());
    }
    Ok(raw)
}

/// Pulizia per la cronologia, con lo stile dell'app in cui il testo era stato incollato.
/// Se la pulizia non è disponibile o fallisce, resta il grezzo.
pub async fn clean_for_history(app: &AppHandle, raw: &str, app_name: &str, category: &str) -> String {
    let Some(mgr) = app.try_state::<Arc<CleanupManager>>().map(|m| Arc::clone(&m)) else {
        return raw.to_string();
    };
    let cctx = mgr.settings().context_for(app_name, category);
    match mgr.clean(raw, &cctx, |_| {}).await {
        Ok(c) => c.text,
        Err(e) => {
            warn!("[flow] pulizia del riprova fallita ({e:#}): resta il grezzo");
            raw.to_string()
        }
    }
}

/// Importa un dizionario da un file JSON (`--import-dictionary`) e riavvia
/// il motore di pulizia: il prompt di sistema con il dizionario si costruisce all'avvio.
pub fn import_dictionary(app: &AppHandle, path: &std::path::Path) {
    let Some(mgr) = app.try_state::<Arc<CleanupManager>>() else {
        warn!("[flow] import del dizionario: modulo di pulizia assente");
        return;
    };
    match mgr.dictionary.import_from_json(path) {
        Ok(n) => {
            info!("[flow] dizionario importato da {}: {n} voci nuove", path.display());
            let _ = app.emit(crate::cleanup::EVENT_DICTIONARY_CHANGED, serde_json::json!({}));
            mgr.start_engine();
        }
        Err(e) => error!("[flow] import del dizionario fallito ({}): {e:#}", path.display()),
    }
}

/// Avvia o ferma la dettatura (riga di comando `--toggle-transcription`, menu dell'icona).
pub fn toggle(app: &AppHandle) {
    let active = SESSION.lock().unwrap().is_some();
    if active {
        stop(app);
    } else {
        start(app, true);
    }
}

fn start(app: &AppHandle, hands_free: bool) {
    let rm = app.state::<Arc<AudioRecordingManager>>();
    if rm.is_recording() || SESSION.lock().unwrap().is_some() {
        debug!("[flow] start ignorato: dettatura già in corso");
        return;
    }
    let started = Instant::now();
    // Dopo una pausa lunga i modelli si riscaldano mentre si parla, prima della sessione ASR.
    if LAST_GPU_USE.lock().unwrap().elapsed() >= REWARM_ON_START_AFTER {
        rewarm_engines(app, "dettatura dopo una pausa");
    }
    *LAST_GPU_USE.lock().unwrap() = Instant::now();
    let settings = get_settings(app);
    let (context, context_thread) = open_context_and_asr(app, &settings);

    // 3. Microfono.
    let vad = if settings.vad_enabled {
        VadPolicy::Offline
    } else {
        VadPolicy::Disabled
    };
    match rm.try_start_recording(BINDING, vad) {
        Ok(readiness) => {
            *SESSION.lock().unwrap() = Some(Session {
                started,
                hands_free,
                context,
                context_thread,
                file_samples: None,
            });
            begin_ui(app, hands_free, started);
            let app2 = app.clone();
            let rm2 = Arc::clone(&rm);
            std::thread::spawn(move || {
                if readiness.wait() {
                    crate::overlay::emit_recording_ready(&app2);
                    play_feedback_sound(&app2, SoundType::Start);
                    rm2.apply_mute();
                }
            });
            info!("[flow] dettatura avviata (hands_free={hands_free}) in {:?}", started.elapsed());
        }
        Err(e) => {
            error!("[flow] microfono non avviato: {e:#}");
            if let Some(asr) = app.try_state::<SharedAsr>() {
                asr.cancel();
            }
            let _ = app.emit(
                "recording-error",
                serde_json::json!({
                    "error_type": crate::managers::audio::recording_error_type(&e),
                    "detail": format!("{e:#}"),
                }),
            );
            emit_state(app, "error", Some("microphone".into()), started);
            crate::win::hotkeys::notify_dictation_ended();
        }
    }
}

/// Dettatura da un WAV a 16 kHz mono (`--dictate-file`): stessa catena, l'audio
/// arriva al motore ASR a velocità reale come dal microfono. Serve alle prove
/// senza voce e alle misure ripetibili.
pub fn dictate_file(app: &AppHandle, path: &std::path::Path) {
    let samples = match crate::audio_toolkit::read_wav_samples(path) {
        Ok(s) => s,
        Err(e) => {
            error!("[flow] WAV non letto ({}): {e:#}", path.display());
            return;
        }
    };
    let rm = app.state::<Arc<AudioRecordingManager>>();
    if rm.is_recording() || SESSION.lock().unwrap().is_some() {
        debug!("[flow] dictate_file ignorato: dettatura già in corso");
        return;
    }
    let started = Instant::now();
    let settings = get_settings(app);
    let (context, context_thread) = open_context_and_asr(app, &settings);
    *SESSION.lock().unwrap() = Some(Session {
        started,
        hands_free: true,
        context,
        context_thread,
        file_samples: Some(samples.clone()),
    });
    begin_ui(app, true, started);
    info!("[flow] dettatura da file avviata: {} ({:.1} s)", path.display(), samples.len() as f64 / 16_000.0);
    if let Some(asr) = app.try_state::<SharedAsr>() {
        for frame in samples.chunks(512) {
            if SESSION.lock().unwrap().is_none() {
                return; // annullata
            }
            asr.feed(frame);
            std::thread::sleep(Duration::from_micros(frame.len() as u64 * 1_000_000 / 16_000));
        }
    }
    stop(app);
}

fn begin_ui(app: &AppHandle, hands_free: bool, started: Instant) {
    // Chiude l'osservazione in corso anche per le partenze da barra, vassoio e file.
    crate::win::hotkeys::notify_dictation_started();
    crate::shortcut::register_cancel_shortcut(app);
    set_tray_state(app, TrayIconState::Recording);
    crate::overlay::show_recording_overlay(app);
    emit_state(
        app,
        "listening",
        hands_free.then(|| "hands_free".to_string()),
        started,
    );
}

type ContextSlot = Arc<Mutex<Option<AppContext>>>;

fn open_context_and_asr(
    app: &AppHandle,
    settings: &crate::settings::AppSettings,
) -> (ContextSlot, Option<std::thread::JoinHandle<()>>) {
    // 1. Sessione ASR con il vocabolario del dizionario (già ordinato). Si apre prima
    //    del contesto, così i nomi letti a schermo trovano la sessione già aperta.
    let vocabulary = app
        .try_state::<Arc<CleanupManager>>()
        .map(|m| m.vocabulary_for_asr())
        .unwrap_or_default();
    if let Some(asr) = app.try_state::<SharedAsr>() {
        // Con il rilevamento acceso la lingua non si forza: la sceglie l'ASR fra `languages`.
        let language = (!settings.asr.language_detection)
            .then(|| settings.asr.primary_language.clone())
            .filter(|l| !l.is_empty());
        if let Err(e) = asr.start_session(SessionConfig { vocabulary, language }) {
            warn!("[flow] sessione ASR non aperta: {e}");
        }
    }

    // 2. Contesto in parallelo, con tetto di tempo (fuori dal percorso critico). I nomi
    //    propri a schermo vanno anche all'ASR per i pezzi non ancora decodificati.
    let context = Arc::new(Mutex::new(None));
    let ctx_slot = Arc::clone(&context);
    let ctx_app = app.clone();
    let timeout = Duration::from_millis(settings.win.context_timeout_ms.max(50));
    let context_thread = std::thread::Builder::new()
        .name("flow-context".into())
        .spawn(move || {
            let ctx = crate::win::context::capture_context(timeout);
            crate::win::emit_context(&ctx_app, &ctx);
            let mut extra = ctx.proper_nouns.clone();
            extra.extend(file_names_in_title(&ctx.window_title));
            if !extra.is_empty() {
                if let Some(asr) = ctx_app.try_state::<SharedAsr>() {
                    asr.add_session_vocabulary(extra);
                }
            }
            *ctx_slot.lock().unwrap() = Some(ctx);
        })
        .ok();
    (context, context_thread)
}

/// Nomi di file nel titolo della finestra (per esempio "flow.rs - progetto - Visual Studio Code"):
/// parole con un'estensione di 1-6 caratteri che contiene almeno una lettera.
fn file_names_in_title(title: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in title.split(|c: char| c.is_whitespace() || matches!(c, '|' | '—' | '–' | '(' | ')' | '[' | ']' | '"' | '\'' | ',' | ':' | '/' | '\\')) {
        let w = raw.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        let Some((stem, ext)) = w.rsplit_once('.') else { continue };
        let stem_ok = !stem.is_empty()
            && stem.chars().all(|c| c.is_alphanumeric() || matches!(c, '_' | '-' | '.'));
        let ext_ok = (1..=6).contains(&ext.len())
            && ext.chars().all(|c| c.is_ascii_alphanumeric())
            && ext.chars().any(|c| c.is_ascii_alphabetic());
        if stem_ok && ext_ok && !out.iter().any(|o| o.eq_ignore_ascii_case(w)) {
            out.push(w.to_string());
        }
    }
    out
}

fn take_session() -> Option<Session> {
    SESSION.lock().unwrap().take()
}

fn finish_ui(app: &AppHandle, state: &str, detail: Option<String>, since: Instant, linger: bool) {
    emit_state(app, state, detail, since);
    set_tray_state(app, TrayIconState::Idle);
    if linger {
        let app = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(PASTED_LINGER);
            if SESSION.lock().unwrap().is_none() {
                rest_bar(&app);
            }
        });
    } else {
        rest_bar(app);
    }
    crate::win::hotkeys::notify_dictation_ended();
}

/// Barra di dettatura fra una dettatura e l'altra: a riposo e visibile, oppure nascosta.
fn rest_bar(app: &AppHandle) {
    if get_settings(app).flow_bar_always_visible {
        emit_state(app, "idle", None, Instant::now());
        crate::overlay::show_idle_overlay(app);
    } else {
        crate::overlay::hide_recording_overlay(app);
    }
}

/// La barra a riposo si allarga per mostrare una notifica (chiamato dall'interfaccia).
#[tauri::command]
#[specta::specta]
pub fn flow_expand_bar(app: AppHandle) {
    if SESSION.lock().unwrap().is_none() {
        crate::overlay::show_notice_overlay(&app);
    } else {
        // Avviso durante la dettatura (per esempio la durata): la finestra si allarga.
        crate::overlay::set_notice_open(true);
        crate::overlay::refresh_overlay_geometry(&app);
    }
}

/// La notifica è chiusa: la barra torna a riposo, o sparisce se è nascosta.
#[tauri::command]
#[specta::specta]
pub fn flow_rest_bar(app: AppHandle) {
    crate::overlay::set_notice_open(false);
    if SESSION.lock().unwrap().is_none() {
        rest_bar(&app);
    } else {
        crate::overlay::refresh_overlay_geometry(&app);
    }
}

/// Pressione sulla bolla (pointerdown dall'interfaccia). Un trascinamento sposta la
/// bolla e la aggancia al bordo; un clic avvia o ferma la dettatura a mani libere.
/// Durante la trascrizione e la pulizia il clic non fa nulla.
#[tauri::command]
#[specta::specta]
pub fn flow_bar_press(app: AppHandle) {
    std::thread::spawn(move || {
        if crate::overlay::track_press(&app) {
            return;
        }
        let busy = CHAIN.try_lock().is_err();
        if busy && SESSION.lock().unwrap().is_none() {
            debug!("[flow] clic sulla bolla ignorato: elaborazione in corso");
            return;
        }
        toggle(&app);
    });
}

/// Bordo e punto della bolla, letti dall'interfaccia all'avvio.
#[tauri::command]
#[specta::specta]
pub fn flow_bar_dock(app: AppHandle) -> crate::overlay::FlowDockEvent {
    crate::overlay::dock_info(&app)
}

/// Mostra o nasconde la barra di dettatura a riposo (impostazione "nascondi barra").
#[tauri::command]
#[specta::specta]
pub fn set_flow_bar_visible(app: AppHandle, visible: bool) {
    let mut settings = get_settings(&app);
    settings.flow_bar_always_visible = visible;
    crate::settings::write_settings(&app, settings);
    if SESSION.lock().unwrap().is_none() {
        rest_bar(&app);
    }
}

/// Parole al minuto nella pillola all'incolla (impostazione "p/m all'incolla").
#[tauri::command]
#[specta::specta]
pub fn set_flow_bar_paste_wpm(app: AppHandle, enabled: bool) {
    let mut settings = get_settings(&app);
    settings.flow_bar_paste_wpm = enabled;
    crate::settings::write_settings(&app, settings);
}

fn stop(app: &AppHandle) {
    let Some(mut session) = take_session() else {
        debug!("[flow] stop senza dettatura in corso");
        return;
    };
    let _chain = CHAIN.lock().unwrap();
    let _gpu = GpuUseMark;
    let released = Instant::now();
    crate::shortcut::unregister_cancel_shortcut(app);

    let rm = app.state::<Arc<AudioRecordingManager>>();
    rm.remove_mute();
    play_feedback_sound(app, SoundType::Stop);
    set_tray_state(app, TrayIconState::Transcribing);
    crate::overlay::show_transcribing_overlay(app);
    emit_state(app, "transcribing", None, released);

    let samples = match session.file_samples.take() {
        Some(s) => s,
        None => {
            let generation = rm.cancel_generation();
            rm.stop_recording(BINDING, generation).unwrap_or_default()
        }
    };
    let audio_secs = samples.len() as f64 / 16_000.0;

    // Testo grezzo: l'ultimo pezzo, gli altri sono già stati decodificati.
    let t_asr = Instant::now();
    let raw = match app.try_state::<SharedAsr>() {
        Some(asr) => match asr.finish() {
            Ok(t) => {
                *LAST_LANGUAGE.lock().unwrap() = t.language.clone();
                t.text.trim().to_string()
            }
            Err(e) => {
                error!("[flow] trascrizione fallita: {e:#}");
                let _ = app.emit("transcription-error", e.to_string());
                finish_ui(app, "error", Some("asr".into()), released, false);
                return;
            }
        },
        None => String::new(),
    };
    let asr_ms = t_asr.elapsed().as_millis() as u64;

    // Contesto (di solito pronto da tempo).
    if let Some(t) = session.context_thread.take() {
        let _ = t.join();
    }
    let ctx = session.context.lock().unwrap().clone().unwrap_or_default();

    if raw.is_empty() {
        info!("[flow] nessun parlato ({audio_secs:.1} s di audio)");
        finish_ui(app, "cancelled", Some("empty".into()), released, false);
        return;
    }

    // Pulizia (o modalità comando con testo selezionato).
    emit_state(app, "cleaning", None, released);
    crate::overlay::show_processing_overlay(app);
    let t_clean = Instant::now();
    let mgr = app.try_state::<Arc<CleanupManager>>().map(|m| Arc::clone(&m));
    let selected = ctx
        .field
        .as_ref()
        .map(|f| f.selected.trim().to_string())
        .filter(|s| !s.is_empty());
    let command_mode = mgr.as_ref().is_some_and(|m| m.settings().command_mode_enabled) && selected.is_some();
    let (mut text, press_enter, used_quality) = match &mgr {
        Some(mgr) if command_mode => {
            let sel = selected.clone().unwrap_or_default();
            match tauri::async_runtime::block_on(mgr.command(&sel, &raw)) {
                Ok(r) => (r.text, false, false),
                Err(e) => {
                    warn!("[flow] modalità comando fallita ({e:#}): incollo il grezzo");
                    (raw.clone(), false, false)
                }
            }
        }
        Some(mgr) => {
            let mut cctx = build_cleanup_context(mgr, &ctx);
            if let Some(lang) = LAST_LANGUAGE.lock().unwrap().clone().filter(|l| !l.is_empty()) {
                cctx.language = lang;
            }
            let app_p = app.clone();
            let on_partial = move |p: &str| {
                let _ = app_p.emit(EVENT_PARTIAL, PartialTextEvent { text: p.to_string() });
            };
            match tauri::async_runtime::block_on(mgr.clean(&raw, &cctx, on_partial)) {
                Ok(c) => (c.text, c.press_enter, c.used_quality),
                Err(e) => {
                    warn!("[flow] pulizia fallita ({e:#}): incollo il grezzo");
                    (raw.clone(), false, false)
                }
            }
        }
        None => (raw.clone(), false, false),
    };

    // Transform applicato in automatico dopo la dettatura: sul testo pulito, prima
    // dell'incolla, così nel campo entra una volta sola. Se fallisce si incolla il pulito.
    let mut auto_applied: Option<(AppliedTransform, u32)> = None;
    if let Some(mgr) = mgr.as_ref().filter(|_| !command_mode && !text.trim().is_empty()) {
        let ts = mgr.transforms.get();
        if ts.enabled && ts.auto_apply.active {
            let slot = ts.auto_apply.slot;
            let name = ts.slots.iter().find(|s| s.slot == slot).map(|s| s.name.clone()).unwrap_or_default();
            let cctx = build_cleanup_context(mgr, &ctx);
            emit_transform(app, "running", &name, 0, &text, "", None);
            match tauri::async_runtime::block_on(mgr.transform(slot, &text, &cctx)) {
                Ok(r) if r.changed && !r.text.trim().is_empty() => {
                    info!("[flow] Transform automatico {name} in {} ms, {} modifiche", r.ms, r.changes.len());
                    auto_applied = Some((
                        AppliedTransform {
                            slot,
                            name,
                            ctx: ctx.clone(),
                            original: text.clone(),
                            transformed: r.text.clone(),
                            on_last_dictation: true,
                        },
                        r.changes.len() as u32,
                    ));
                    text = r.text;
                }
                Ok(_) => emit_transform(app, "done", &name, 0, &text, &text, None),
                Err(e) => {
                    warn!("[flow] Transform automatico {name} fallito ({e:#}): incollo il testo pulito");
                    emit_transform(app, "error", &name, 0, &text, "", Some(format!("{e:#}")));
                }
            }
        }
    }
    let clean_ms = t_clean.elapsed().as_millis() as u64;

    if text.trim().is_empty() {
        finish_ui(app, "cancelled", Some("empty".into()), released, false);
        return;
    }

    // Incolla nella finestra catturata alla pressione.
    emit_state(app, "pasting", None, released);
    let t_paste = Instant::now();
    let outcome = crate::win::paste::paste_text(&text, &ctx, press_enter);
    let paste_ms = t_paste.elapsed().as_millis() as u64;
    let total_ms = released.elapsed().as_millis() as u64;
    crate::win::emit_paste_outcome(app, &outcome);
    info!(
        "[flow] latenza rilascio→incolla {total_ms} ms (asr {asr_ms}, pulizia {clean_ms}, incolla {paste_ms}) audio {audio_secs:.1} s, app {}, esito {}",
        ctx.app_name, outcome.success
    );
    record_latency(app, &ctx, audio_secs, asr_ms, clean_ms, paste_ms, total_ms, outcome.success, used_quality);

    *LAST_TEXT.lock().unwrap() = Some(text.clone());
    let words = text.split_whitespace().count() as u32;
    let wpm = (audio_secs >= 1.0).then(|| (words as f64 / (audio_secs / 60.0)).round() as u32);
    let _ = app.emit(EVENT_DICTATION_STATS, DictationStatsEvent { words, audio_secs, wpm });
    if outcome.success {
        play_feedback_sound(app, SoundType::Paste);
        crate::win::observe::observe_after_paste(app, &ctx, text.clone());
        if let Some((applied, changes)) = auto_applied {
            emit_transform(app, "done", &applied.name, changes, &applied.original, &applied.transformed, None);
            *LAST_TRANSFORM.lock().unwrap() = Some(applied);
        }
    } else {
        let _ = app.emit("paste-error", ());
        // La barra mostra il testo, che resta anche negli appunti con Incolla ultima.
        let _ = app.emit(EVENT_PASTE_FAILED, PasteFailedEvent { text: text.clone(), reason: outcome.reason.clone() });
    }

    // Cronologia: WAV più testo grezzo e pulito.
    let meta = (ctx.app_name.clone(), category_str(&ctx), total_ms as i64, used_quality, LAST_LANGUAGE.lock().unwrap().clone());
    save_history(app, samples, raw, text, meta);

    if outcome.success {
        finish_ui(app, "pasted", None, released, true);
    } else {
        // La notifica col testo deve restare leggibile: niente ritorno a riposo
        // finché l'interfaccia non chiama `flow_rest_bar` (chiusa o copiata).
        emit_state(app, "error", outcome.reason.clone(), released);
        set_tray_state(app, TrayIconState::Idle);
        crate::overlay::show_notice_overlay(app);
        crate::win::hotkeys::notify_dictation_ended();
    }
}

fn build_cleanup_context(mgr: &CleanupManager, ctx: &AppContext) -> CleanupContext {
    let mut c = mgr.settings().context_for(&ctx.app_name, &category_str(ctx));
    c.url = ctx.url.clone();
    if let Some(f) = &ctx.field {
        c.field_before = Some(f.before.clone()).filter(|s| !s.is_empty());
        c.field_selected = Some(f.selected.clone()).filter(|s| !s.is_empty());
        c.field_after = Some(f.after.clone()).filter(|s| !s.is_empty());
    }
    c.proper_nouns = ctx.proper_nouns.clone();
    c
}

fn save_history(
    app: &AppHandle,
    samples: Vec<f32>,
    raw: String,
    cleaned: String,
    (app_name, category, latency_ms, used_quality, language): (String, String, i64, bool, Option<String>),
) {
    let Some(hm) = app.try_state::<Arc<HistoryManager>>() else {
        return;
    };
    let hm = Arc::clone(&hm);
    std::thread::spawn(move || {
        let file_name = format!("flow-{}.wav", chrono::Utc::now().timestamp_millis());
        let path = hm.recordings_dir().join(&file_name);
        if let Err(e) = crate::audio_toolkit::save_wav_file(&path, &samples) {
            warn!("[flow] WAV non salvato: {e}");
        }
        match hm.save_entry(file_name, raw, true, Some(cleaned), None) {
            Ok(entry) => {
                if let Err(e) = hm.set_flow_meta(entry.id, &app_name, &category, latency_ms, used_quality, language.as_deref()) {
                    warn!("[flow] dati della cronologia non salvati: {e}");
                }
            }
            Err(e) => warn!("[flow] cronologia non salvata: {e}"),
        }
    });
}

#[allow(clippy::too_many_arguments)]
fn record_latency(
    app: &AppHandle,
    ctx: &AppContext,
    audio_secs: f64,
    asr_ms: u64,
    clean_ms: u64,
    paste_ms: u64,
    total_ms: u64,
    success: bool,
    used_quality: bool,
) {
    let Ok(dir) = crate::portable::app_data_dir(app) else {
        return;
    };
    let path = dir.join("latenze.csv");
    let new_file = !path.exists();
    let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) else {
        return;
    };
    if new_file {
        let _ = writeln!(
            f,
            "timestamp,app,categoria,audio_s,asr_ms,pulizia_ms,incolla_ms,totale_ms,incollato,qualita"
        );
    }
    let _ = writeln!(
        f,
        "{},{},{},{:.2},{},{},{},{},{},{}",
        chrono::Local::now().format("%Y-%m-%dT%H:%M:%S"),
        ctx.app_name.replace(',', " "),
        category_str(ctx),
        audio_secs,
        asr_ms,
        clean_ms,
        paste_ms,
        total_ms,
        success,
        used_quality
    );
}

/// Annulla la dettatura della catena flow, se ce n'è una. Vale per Esc, `--cancel`, la X della
/// pillola e "Annulla" nel menu della tray (via `utils::cancel_current_operation`).
pub fn cancel(app: &AppHandle) -> bool {
    discard_session(app, "dettatura annullata", None, true)
}

/// Scarta la dettatura in corso senza trascriverla né incollarla e senza suoni: Annulla, e il
/// tocco singolo del doppio tocco. Gira in modo sincrono sul thread degli intenti, prima
/// della Start successiva.
fn discard_session(app: &AppHandle, why: &str, detail: Option<&str>, linger: bool) -> bool {
    let Some(session) = take_session() else {
        return false;
    };
    crate::shortcut::unregister_cancel_shortcut(app);
    let rm = app.state::<Arc<AudioRecordingManager>>();
    rm.remove_mute();
    rm.cancel_recording();
    if let Some(asr) = app.try_state::<SharedAsr>() {
        asr.cancel();
    }
    info!("[flow] {why} dopo {:?}", session.started.elapsed());
    finish_ui(app, "cancelled", detail.map(str::to_string), session.started, linger);
    true
}

/// Ultima dettatura: quella di questa sessione, o dopo un riavvio l'ultima della cronologia
/// (testo pulito se c'è, altrimenti il grezzo). Serve a "incolla ultima" e a Polish.
fn last_text(app: &AppHandle) -> Option<String> {
    if let Some(t) = LAST_TEXT.lock().unwrap().clone() {
        return Some(t);
    }
    let hm = app.try_state::<Arc<HistoryManager>>()?;
    let entry = hm.get_latest_completed_entry().ok()??;
    let text = entry
        .post_processed_text
        .filter(|t| !t.trim().is_empty())
        .unwrap_or(entry.transcription_text);
    (!text.trim().is_empty()).then_some(text)
}

fn paste_last(app: &AppHandle) {
    let Some(text) = last_text(app) else {
        debug!("[flow] nessuna dettatura da reincollare");
        return;
    };
    let ctx = crate::win::context::capture_context(Duration::from_millis(150));
    let outcome = crate::win::paste::paste_text(&text, &ctx, false);
    crate::win::emit_paste_outcome(app, &outcome);
}

fn emit_transform(app: &AppHandle, state: &str, name: &str, changes: u32, before: &str, after: &str, message: Option<String>) {
    let payload = TransformEvent {
        state: state.to_string(),
        name: name.to_string(),
        changes,
        before: before.to_string(),
        after: after.to_string(),
        message,
    };
    if let Err(e) = app.emit(EVENT_TRANSFORM, payload) {
        debug!("[flow] evento {EVENT_TRANSFORM} non emesso: {e}");
    }
}

/// Esegue il modello del Transform con barra ed eventi: `None` (evento `error` già emesso) se
/// fallisce, se il testo è troppo lungo o se il risultato è uguale all'entrata (evento `done`
/// con 0 modifiche): in tutti e tre i casi il campo non si tocca.
fn run_transform_model(
    app: &AppHandle,
    mgr: &CleanupManager,
    slot: u8,
    name: &str,
    ctx: &AppContext,
    text: &str,
    since: Instant,
) -> Option<crate::cleanup::transforms::TransformResult> {
    emit_transform(app, "running", name, 0, text, "", None);
    crate::overlay::show_processing_overlay(app);
    emit_state(app, "cleaning", Some("transform".into()), since);
    let cctx = build_cleanup_context(mgr, ctx);
    match tauri::async_runtime::block_on(mgr.transform(slot, text, &cctx)) {
        Ok(r) if r.changed && !r.text.trim().is_empty() => Some(r),
        Ok(_) => {
            info!("[flow] Transform {name}: nessuna modifica");
            emit_transform(app, "done", name, 0, text, text, None);
            finish_ui(app, "cancelled", Some("transform_unchanged".into()), since, true);
            None
        }
        Err(e) => {
            warn!("[flow] Transform {name} fallito: {e:#}");
            emit_transform(app, "error", name, 0, text, "", Some(format!("{e:#}")));
            finish_ui(app, "error", Some("transform".into()), since, true);
            None
        }
    }
}

/// Transform da scorciatoia (Win+Alt+1..9): sul testo selezionato, altrimenti sull'ultima
/// dettatura degli ultimi 60 secondi nella stessa finestra. L'ultima dettatura si sostituisce
/// con rilettura del campo: se non si può verificare, il campo resta com'era.
fn transform_hotkey(app: &AppHandle, slot: u8) {
    let Some(mgr) = app.try_state::<Arc<CleanupManager>>().map(|m| Arc::clone(&m)) else {
        return;
    };
    let ts = mgr.transforms.get();
    if !ts.enabled {
        info!("[flow] Transforms disattivati: scorciatoia dello slot {slot} ignorata");
        return;
    }
    let Some(def) = ts.slots.iter().find(|s| s.slot == slot).cloned() else {
        info!("[flow] nessun Transform nello slot {slot}");
        return;
    };
    // Dopo una dettatura appena rilasciata aspetta il suo incolla, poi lavora su di lei.
    let _chain = CHAIN.lock().unwrap();
    let _gpu = GpuUseMark;
    let since = Instant::now();
    let selection = crate::win::selection::get_selected_text(Duration::from_millis(300))
        .filter(|s| !s.trim().is_empty());
    let ctx = crate::win::context::capture_context(Duration::from_millis(150));
    let (text, on_last_dictation) = match selection {
        Some(s) => (s, false),
        None => match crate::win::replace::last_dictation_within(crate::win::replace::LAST_DICTATION_MAX_AGE) {
            Some(last) => (last.text, true),
            None => {
                let msg = "Seleziona il testo da trasformare: senza selezione il Transform vale solo sull'ultima dettatura, entro 60 secondi e nella stessa finestra".to_string();
                info!("[flow] Transform {}: niente selezione né dettatura recente", def.name);
                emit_transform(app, "error", &def.name, 0, "", "", Some(msg));
                return;
            }
        },
    };
    let Some(r) = run_transform_model(app, &mgr, slot, &def.name, &ctx, &text, since) else {
        return;
    };
    let written = if on_last_dictation {
        crate::win::replace::replace_last_dictation(&r.text)
            .map(|(out, last)| (out.paste.success, last.ctx))
            .map_err(|e| e.user_message())
    } else {
        // La selezione è ancora attiva: l'incolla la sostituisce.
        let outcome = crate::win::paste::paste_text(&r.text, &ctx, false);
        crate::win::emit_paste_outcome(app, &outcome);
        if outcome.success {
            Ok((true, ctx.clone()))
        } else {
            Err(outcome.reason.unwrap_or_else(|| "incolla non riuscito".into()))
        }
    };
    match written {
        Ok((_, target_ctx)) => {
            info!("[flow] Transform {} scritto nel campo in {:?} ({} modifiche)", def.name, since.elapsed(), r.changes.len());
            emit_transform(app, "done", &def.name, r.changes.len() as u32, &text, &r.text, None);
            *LAST_TRANSFORM.lock().unwrap() = Some(AppliedTransform {
                slot,
                name: def.name.clone(),
                ctx: target_ctx,
                original: text,
                transformed: r.text,
                on_last_dictation,
            });
            finish_ui(app, "pasted", Some("transform".into()), since, true);
        }
        Err(msg) => {
            warn!("[flow] Transform {}: campo non sostituito ({msg})", def.name);
            emit_transform(app, "error", &def.name, 0, &text, &r.text, Some(msg));
            finish_ui(app, "error", Some("transform".into()), since, true);
        }
    }
}

/// Annulla dell'ultimo Transform (barra): rimette il testo di prima, con rilettura del campo.
fn undo_last_transform(app: &AppHandle) -> Result<(), String> {
    let _chain = CHAIN.lock().unwrap();
    let last = LAST_TRANSFORM
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Nessun Transform da annullare".to_string())?;
    crate::win::replace::undo_transform(&last.ctx, &last.original, &last.transformed).map_err(|e| e.user_message())?;
    if last.on_last_dictation {
        crate::win::replace::record_dictation(&last.original, &last.ctx);
    }
    *LAST_TRANSFORM.lock().unwrap() = None;
    info!("[flow] Transform {} annullato", last.name);
    emit_transform(app, "done", &last.name, 0, &last.transformed, &last.original, None);
    Ok(())
}

/// Riprova dell'ultimo Transform (barra): di nuovo sul testo di partenza, poi il risultato
/// nuovo prende il posto di quello di prima nel campo.
fn retry_last_transform(app: &AppHandle) -> Result<(), String> {
    let mgr = app
        .try_state::<Arc<CleanupManager>>()
        .map(|m| Arc::clone(&m))
        .ok_or_else(|| "Modulo di pulizia assente".to_string())?;
    let _chain = CHAIN.lock().unwrap();
    let _gpu = GpuUseMark;
    let since = Instant::now();
    let last = LAST_TRANSFORM
        .lock()
        .unwrap()
        .clone()
        .ok_or_else(|| "Nessun Transform da riprovare".to_string())?;
    let Some(r) = run_transform_model(app, &mgr, last.slot, &last.name, &last.ctx, &last.original, since) else {
        return Ok(());
    };
    if r.text.trim() == last.transformed.trim() {
        emit_transform(app, "done", &last.name, r.changes.len() as u32, &last.original, &r.text, None);
        finish_ui(app, "pasted", Some("transform".into()), since, true);
        return Ok(());
    }
    if let Err(e) = crate::win::replace::replace_pasted_text(&last.ctx, &last.transformed, &r.text) {
        let msg = e.user_message();
        emit_transform(app, "error", &last.name, 0, &last.original, &r.text, Some(msg.clone()));
        finish_ui(app, "error", Some("transform".into()), since, true);
        return Err(msg);
    }
    if last.on_last_dictation {
        crate::win::replace::record_dictation(&r.text, &last.ctx);
    }
    emit_transform(app, "done", &last.name, r.changes.len() as u32, &last.original, &r.text, None);
    *LAST_TRANSFORM.lock().unwrap() = Some(AppliedTransform { transformed: r.text, ..last });
    finish_ui(app, "pasted", Some("transform".into()), since, true);
    Ok(())
}

// ---------------------------------------------------------------- comandi

#[tauri::command]
#[specta::specta]
pub fn flow_stop(app: AppHandle) {
    std::thread::spawn(move || handle_intent(&app, DictationIntent::Stop));
}

#[tauri::command]
#[specta::specta]
pub fn flow_paste_last(app: AppHandle) {
    std::thread::spawn(move || handle_intent(&app, DictationIntent::PasteLast));
}

/// "Annulla" nel riquadro del Transform: rimette nel campo il testo di prima.
#[tauri::command]
#[specta::specta]
pub async fn undo_transform(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || undo_last_transform(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// "Riprova" nel riquadro del Transform: lo stesso Transform di nuovo sul testo di partenza.
#[tauri::command]
#[specta::specta]
pub async fn retry_transform(app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || retry_last_transform(&app))
        .await
        .map_err(|e| e.to_string())?
}

/// Aggiorna una sezione delle impostazioni nuove (`asr`, `cleanup`, `win`) e la applica.
#[tauri::command]
#[specta::specta]
pub fn update_settings_section(
    app: AppHandle,
    section: String,
    value: serde_json::Value,
) -> Result<(), String> {
    let mut settings = get_settings(&app);
    match section.as_str() {
        "asr" => {
            settings.asr = serde_json::from_value(value).map_err(|e| e.to_string())?;
        }
        "cleanup" => {
            let s: crate::cleanup::CleanupSettings =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
            if let Some(mgr) = app.try_state::<Arc<CleanupManager>>() {
                mgr.update_settings(s.clone());
            }
            settings.cleanup = s;
        }
        "win" => {
            let s: crate::win::WinSettings =
                serde_json::from_value(value).map_err(|e| e.to_string())?;
            crate::win::apply_settings(s.clone());
            settings.win = s;
        }
        other => return Err(format!("sezione sconosciuta: {other}")),
    }
    crate::settings::write_settings(&app, settings);
    Ok(())
}

#[cfg(test)]
mod tests_titolo {
    use super::file_names_in_title;

    #[test]
    fn nomi_file_dal_titolo_della_finestra() {
        assert_eq!(
            file_names_in_title("flow.rs - eagleVoice - Visual Studio Code"),
            vec!["flow.rs".to_string()]
        );
        assert_eq!(
            file_names_in_title("ricette.md, menu.txt - cucina (Workspace)"),
            vec!["ricette.md".to_string(), "menu.txt".to_string()]
        );
        assert!(file_names_in_title("Claude").is_empty());
        assert!(file_names_in_title("Versione 1.2 del 18.10").is_empty());
    }
}
