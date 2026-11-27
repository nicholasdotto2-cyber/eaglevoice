//! Pulizia del testo dettato (modulo `cleanup\`, worker pulizia-llm).
//!
//! Testo grezzo dell'ASR più contesto (app, categoria, formato, livello, stile, testo del campo)
//! in ingresso, testo pulito in uscita, in streaming. Motore locale: `llama-server` con
//! Qwen3.5-4B residente (`llama.rs`); modalità qualità: API Anthropic con lo stesso prompt
//! (`quality.rs`). Il modulo possiede anche il dizionario (`dictionary.rs`).
//!
//! Catena di `clean`: "premi invio", snippet sull'intera dettatura, rete del backtrack,
//! sostituzioni del dizionario, salto del modello per testi corti o già puliti, modello in
//! streaming, controllo dei termini, sostituzioni e snippet, elenchi, formato, maiuscola dal
//! campo, spazi.
#![allow(dead_code)]

pub mod commands;
pub mod dictionary;
pub mod learn;
pub mod llama;
pub mod postprocess;
pub mod prompt;
pub mod quality;
pub mod transforms;

use serde::{Deserialize, Serialize};
use specta::Type;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::Instant;

#[allow(unused_imports)]
pub use dictionary::{Dictionary, DictionaryEntry, DictionarySource, LearnedCandidate, TextChange};

/// Cartella del progetto principale: binari e modelli stanno lì (ignorati da git).
pub const PROJECT_ROOT: &str = concat!(env!("CARGO_MANIFEST_DIR"), r"\..\..");

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
#[derive(Hash)]
pub enum CleanupLevel {
    Exact,
    #[default]
    Tidy,
    Smooth,
    Rewrite,
}

/// Formato della trascrizione: formattazione intelligente, testo grezzo, stile chat, solo minuscole.
#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum TranscriptFormat {
    #[default]
    Smart,
    Raw,
    Chat,
    Lower,
}

/// Stili per categoria.
#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Tone {
    #[default]
    Default,
    Polished,
    Relaxed,
    Loose,
    Lively,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct AppFormat {
    /// Sottostringa del nome del processo o dell'app, senza maiuscole.
    pub app_match: String,
    pub format: TranscriptFormat,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct StyleSettings {
    /// App di chat.
    pub chat: Tone,
    /// Strumenti di lavoro (chat di lavoro, documenti, codice).
    pub team: Tone,
    pub mail: Tone,
    /// Tutto il resto.
    pub general: Tone,
}

impl Default for StyleSettings {
    /// Valori di default: rilassato ovunque.
    fn default() -> Self {
        StyleSettings {
            chat: Tone::Relaxed,
            team: Tone::Relaxed,
            mail: Tone::Relaxed,
            general: Tone::Relaxed,
        }
    }
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct PolishInstruction {
    pub id: String,
    pub label: String,
    pub enabled: bool,
}

/// Impostazioni del modulo. Campo in `AppSettings`: `cleanup`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct CleanupSettings {
    pub level: CleanupLevel,
    pub default_format: TranscriptFormat,
    pub app_formats: Vec<AppFormat>,
    pub styles: StyleSettings,
    pub backtrack_enabled: bool,
    pub press_enter_enabled: bool,
    pub lists_enabled: bool,
    pub quality_mode: bool,
    pub quality_model: String,
    pub polish_instructions: Vec<PolishInstruction>,
    pub command_mode_enabled: bool,
    pub llama_bin_dir: String,
    pub llama_model_path: String,
    pub skip_cleanup_under_words: u32,
}

/// Istruzioni Polish di default, etichette in italiano.
pub fn default_polish_instructions() -> Vec<PolishInstruction> {
    [
        ("trim", "Togli ripetizioni e giri di parole", true),
        ("plain", "Usa parole semplici", true),
        ("order", "Metti prima le informazioni importanti", true),
        ("paragraphs", "Dividi in paragrafi brevi", true),
        ("voice", "Lascia lo stile di chi scrive", false),
        ("ask", "Chiudi con la domanda o la richiesta", false),
    ]
    .into_iter()
    .map(|(id, label, enabled)| PolishInstruction { id: id.into(), label: label.into(), enabled })
    .collect()
}

/// Testo dell'istruzione Polish passato al modello. Gli id delle versioni precedenti, ancora
/// salvati nelle impostazioni, valgono come l'istruzione nuova più vicina.
pub fn polish_instruction_text(id: &str) -> &'static str {
    match id {
        "trim" => "Togli le ripetizioni, i giri di parole e le frasi che non aggiungono nulla.",
        "plain" => "Usa parole semplici e frasi che si capiscono alla prima lettura.",
        "order" => "Metti all'inizio le informazioni che contano di più, poi il resto.",
        "paragraphs" => "Dividi il testo in paragrafi brevi, uno per argomento.",
        "voice" => "Lascia lo stile e il registro di chi ha scritto, senza renderlo più formale.",
        "ask" => "Se il testo contiene una domanda o una richiesta, mettila nell'ultima frase.",
        _ => "",
    }
}

impl Default for CleanupSettings {
    fn default() -> Self {
        CleanupSettings {
            level: CleanupLevel::Tidy,
            default_format: TranscriptFormat::Smart,
            app_formats: Vec::new(),
            styles: StyleSettings::default(),
            backtrack_enabled: true,
            press_enter_enabled: false,
            lists_enabled: true,
            quality_mode: false,
            quality_model: quality::DEFAULT_QUALITY_MODEL.to_string(),
            polish_instructions: default_polish_instructions(),
            command_mode_enabled: false,
            llama_bin_dir: format!(r"{}\bin\llama", PROJECT_ROOT),
            llama_model_path: format!(r"{}\models\qwen\Qwen3.5-4B-Q4_K_M.gguf", PROJECT_ROOT),
            skip_cleanup_under_words: 4,
        }
    }
}

impl CleanupSettings {
    /// Formato per l'app attiva (formato per app, altrimenti quello di default).
    pub fn format_for_app(&self, app_name: &str) -> TranscriptFormat {
        let app = app_name.to_lowercase();
        self.app_formats
            .iter()
            .find(|f| !f.app_match.is_empty() && app.contains(&f.app_match.to_lowercase()))
            .map_or(self.default_format, |f| f.format)
    }

    /// Stile per la categoria decisa dal modulo `win\` (`AppCategory`).
    pub fn style_for_category(&self, category: &str) -> Tone {
        match category {
            "email" => self.styles.mail,
            "chat_personal" => self.styles.chat,
            "chat_work" | "document" | "code" => self.styles.team,
            _ => self.styles.general,
        }
    }

    /// Contesto con formato, stile e livello già risolti dalle impostazioni.
    pub fn context_for(&self, app_name: &str, category: &str) -> CleanupContext {
        CleanupContext {
            app_name: app_name.to_string(),
            category: category.to_string(),
            format: self.format_for_app(app_name),
            level: self.level,
            style: self.style_for_category(category),
            language: "it".to_string(),
            ..Default::default()
        }
    }
}

/// Contesto di una dettatura.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
#[serde(default)]
pub struct CleanupContext {
    pub app_name: String,
    /// `AppCategory` del modulo `win\`: email, chat_personal, chat_work, ai, terminal, document,
    /// code, browser, other.
    pub category: String,
    pub url: Option<String>,
    pub field_before: Option<String>,
    pub field_selected: Option<String>,
    pub field_after: Option<String>,
    pub format: TranscriptFormat,
    pub level: CleanupLevel,
    pub style: Tone,
    /// Codice ISO 639-1 ("it").
    pub language: String,
    pub proper_nouns: Vec<String>,
}

/// Testo pulito.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
pub struct Cleaned {
    pub text: String,
    pub press_enter: bool,
    /// Il modello non è partito (testo corto o già pulito, formato "raw", motore non pronto).
    pub skipped: bool,
    pub used_quality: bool,
    /// Sostituzioni ed espansioni del dizionario applicate.
    pub replacements: u32,
    /// Pezzi di grezzo rimessi dal controllo dei termini.
    pub restored_terms: u32,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommandKind {
    EditText,
    DraftText,
    Answer,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct CommandResult {
    pub kind: CommandKind,
    pub text: String,
}

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
// nome TypeScript diverso da `asr::EngineStatus`: in bindings.ts due tipi con lo stesso nome
// facevano fallire tsc (TS2300); i valori JSON restano "stopped", "starting", "ready", "error"
#[specta(rename = "CleanupEngineStatus")]
pub enum EngineStatus {
    Stopped,
    Starting,
    Ready,
    Error,
}

/// Payload di `cleanup:engine-state` (stessa forma di `asr:engine-state`).
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct CleanupState {
    pub state: EngineStatus,
    pub detail: Option<String>,
    pub model: Option<String>,
    pub vram_mb: Option<u32>,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug)]
pub struct PartialTextEvent {
    pub text: String,
}

pub const EVENT_ENGINE_STATE: &str = "cleanup:engine-state";
pub const EVENT_PARTIAL_TEXT: &str = "cleanup:partial-text";
pub const EVENT_DICTIONARY_CHANGED: &str = "cleanup:dictionary-changed";
/// Parola imparata dalle correzioni: `{ id, wrong, right }`; l'avviso di 6 s con "Annulla"
/// chiama `undo_learned_word(id)`.
pub const EVENT_LEARNED: &str = "cleanup:learned";

/// Chi riceve gli eventi: nell'app `app.emit`, nei test nessuno.
pub type Emitter = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync>;

pub fn null_emitter() -> Emitter {
    Arc::new(|_, _| {})
}

/// Stato del modulo: impostazioni, motore locale, dizionario, prompt.
pub struct CleanupManager {
    settings: RwLock<CleanupSettings>,
    pub engine: Arc<llama::LlamaEngine>,
    pub dictionary: Dictionary,
    prompts: prompt::PromptSet,
    emitter: Emitter,
    /// Variante di prova: se falso le sostituzioni ambigue ("cloud") le decide il modello.
    pub ambiguous_deterministic: AtomicBool,
    /// Variante di prova: se falso le dettature lunghe vanno in una richiesta sola.
    pub parallel_enabled: AtomicBool,
    /// Variante di prova: se impostata sostituisce
    /// `dictionary::correction_terms()` nel controllo dei termini dopo il modello.
    pub correction_terms_override: RwLock<Option<Vec<String>>>,
    /// Variante di prova (niente parole cambiate): se falso `revert_changed_words` non gira.
    pub meaning_guard: AtomicBool,
    quality_warned: AtomicBool,
    /// Prompt di sistema fissato per la sessione del motore (livello, lingua): il dizionario
    /// completo entra una volta, così il prefisso in cache non cambia fra una dettatura e
    /// l'altra; le voci nuove valgono subito per le sostituzioni deterministiche e nel prompt
    /// dal prossimo avvio del motore.
    system_cache: std::sync::Mutex<std::collections::HashMap<(CleanupLevel, String), String>>,
    /// Impostazioni dei Transforms:`transforms.load(...)` e `transforms.set_persist(...)`
    /// all'avvio dell'app.
    pub transforms: transforms::TransformsStore,
    transform_prompts: transforms::TransformPrompts,
}

impl CleanupManager {
    /// Apre il dizionario in `data_dir\dictionary.sqlite`. Il motore non parte finché non si
    /// chiama `start_engine`.
    pub fn new(settings: CleanupSettings, data_dir: &Path, emitter: Emitter) -> anyhow::Result<Arc<Self>> {
        std::fs::create_dir_all(data_dir)?;
        let dictionary = Dictionary::open(&data_dir.join("dictionary.sqlite"))?;
        Ok(Self::with_dictionary(settings, dictionary, data_dir, emitter))
    }

    pub fn with_dictionary(settings: CleanupSettings, dictionary: Dictionary, data_dir: &Path, emitter: Emitter) -> Arc<Self> {
        let engine = llama::LlamaEngine::new(data_dir.to_path_buf(), emitter.clone());
        Arc::new(CleanupManager {
            settings: RwLock::new(settings),
            engine,
            dictionary,
            prompts: prompt::PromptSet::load(),
            emitter,
            ambiguous_deterministic: AtomicBool::new(true),
            parallel_enabled: AtomicBool::new(true),
            correction_terms_override: RwLock::new(None),
            meaning_guard: AtomicBool::new(true),
            quality_warned: AtomicBool::new(false),
            system_cache: std::sync::Mutex::new(std::collections::HashMap::new()),
            transforms: transforms::TransformsStore::default(),
            transform_prompts: transforms::TransformPrompts::load(),
        })
    }

    pub fn settings(&self) -> CleanupSettings {
        self.settings.read().unwrap().clone()
    }

    /// Nuove impostazioni; se cambiano binario o modello il motore riparte.
    pub fn update_settings(&self, s: CleanupSettings) {
        let restart = {
            let cur = self.settings.read().unwrap();
            cur.llama_bin_dir != s.llama_bin_dir || cur.llama_model_path != s.llama_model_path
        };
        *self.settings.write().unwrap() = s;
        if restart {
            self.start_engine();
        }
    }

    /// Avvia (o riavvia) `llama-server` in background, con riscaldamento del prefisso.
    pub fn start_engine(&self) {
        let s = self.settings();
        self.system_cache.lock().unwrap().clear();
        let warm_system = self.system_prompt(s.level, "it");
        self.engine.start(&s.llama_bin_dir, &s.llama_model_path, warm_system);
    }

    /// Riscaldamento di ripresa (dopo una pausa lunga la GPU riparte fredda): stesso prompt di
    /// sistema dell'avvio, in background, al massimo uno alla volta. Non blocca; falso se il
    /// motore non è pronto, se è attiva la modalità qualità o se un riscaldamento è già in corso.
    pub fn rewarm(&self) -> bool {
        let s = self.settings();
        if s.quality_mode || !self.engine.is_ready() {
            return false;
        }
        let system = self.system_prompt(s.level, "it");
        self.engine.rewarm(system)
    }

    pub fn shutdown(&self) {
        self.engine.stop();
    }

    pub fn state(&self) -> CleanupState {
        self.engine.state()
    }

    /// Prompt di sistema della sessione per livello e lingua (vedi `system_cache`).
    fn system_prompt(&self, level: CleanupLevel, language: &str) -> String {
        // una voce per lingua: il prefisso italiano resta in cache anche dopo una dettatura in inglese
        let language = prompt::normalize_language(language);
        let key = (level, language.to_string());
        if let Some(sp) = self.system_cache.lock().unwrap().get(&key) {
            return sp.clone();
        }
        let entries = self.dictionary.list().unwrap_or_default();
        let sp = self.prompts.cleanup_system(level, &entries, language);
        self.system_cache.lock().unwrap().insert(key, sp.clone());
        sp
    }

    fn rules(&self, entries: &[DictionaryEntry]) -> postprocess::Rules {
        let mut r = postprocess::Rules::from_entries(entries);
        r.apply_ambiguous = self.ambiguous_deterministic.load(Ordering::Relaxed);
        r
    }

    fn emit(&self, event: &str, payload: serde_json::Value) {
        (self.emitter)(event, payload);
    }

    /// Pulizia di una dettatura. `on_partial` riceve il testo parziale del modello mentre arriva.
    pub async fn clean(&self, raw: &str, ctx: &CleanupContext, on_partial: impl FnMut(&str) + Send) -> anyhow::Result<Cleaned> {
        self.clean_inner(raw, ctx, on_partial, None).await
    }

    /// Rigioco per le misure: stessa catena di `clean`, ma al posto del modello usa `model_out`
    /// (un'uscita già registrata; `None` vale come modello che non risponde). Nessuna chiamata al
    /// motore né all'API.
    #[doc(hidden)]
    pub async fn clean_replay(&self, raw: &str, ctx: &CleanupContext, model_out: Option<String>) -> anyhow::Result<Cleaned> {
        self.clean_inner(raw, ctx, |_| {}, Some(model_out)).await
    }

    async fn clean_inner(
        &self,
        raw: &str,
        ctx: &CleanupContext,
        mut on_partial: impl FnMut(&str) + Send,
        replay: Option<Option<String>>,
    ) -> anyhow::Result<Cleaned> {
        let s = self.settings();
        let entries = self.dictionary.list().unwrap_or_default();
        let rules = self.rules(&entries);
        let mut result = Cleaned::default();
        let mut used_ids: Vec<String> = Vec::new();

        // 1. "premi invio" in fondo
        let mut text = raw.trim().to_string();
        if s.press_enter_enabled {
            let (t, enter) = postprocess::strip_press_enter(&text);
            text = t;
            result.press_enter = enter;
        }
        if text.is_empty() {
            result.skipped = true;
            return Ok(result);
        }

        // 2. snippet sull'intera dettatura
        if let Some((exp, id)) = rules.whole_snippet(&text) {
            result.text = postprocess::apply_field_context(&exp, ctx.field_before.as_deref(), &rules.keep_case);
            result.skipped = true;
            result.replacements = 1;
            let _ = self.dictionary.touch(&[id]);
            return Ok(result);
        }

        // 3. backtrack netto e sostituzioni prima del modello
        if s.backtrack_enabled {
            text = postprocess::backtrack(&text);
        }
        let pre = rules.apply_replacements(&text);
        result.replacements += pre.count;
        used_ids.extend(pre.used_ids);
        let pre_text = pre.text;

        // 4. modello, salvo salto
        let skip = ctx.format == TranscriptFormat::Raw
            || postprocess::should_skip(&pre_text, s.skip_cleanup_under_words);
        let mut body = if skip {
            result.skipped = true;
            postprocess::basic_punctuation(&pre_text, ctx.format == TranscriptFormat::Raw)
        } else {
            let lists_allowed = s.lists_enabled && !matches!(ctx.category.as_str(), "terminal" | "chat_personal");
            let (marked, snippets) = rules.mark_snippets(&pre_text);
            let system = self.system_prompt(ctx.level, &ctx.language);
            let user = self.prompts.cleanup_user(ctx.level, ctx, &marked, lists_allowed, &entries);
            // tetto legato all'ingresso: il pulito usa poco più di un token per
            // parola; oltre il doppio è una ripetizione fuori controllo e si scarta
            let max_tokens = (postprocess::word_count(&marked) as u32 * 2 + 64).min(3000);
            let started = Instant::now();
            let emitter = self.emitter.clone();
            let mut partial = |t: &str| {
                let clean = postprocess::sanitize_model_output(t);
                on_partial(&clean);
                emitter(EVENT_PARTIAL_TEXT, serde_json::json!({ "text": clean }));
            };
            // dettature lunghe: due metà in parallelo sui due slot del motore locale; con un
            // riscaldamento di ripresa in corso uno slot è occupato e una richiesta sola senza
            // slot fisso va subito su quello libero, invece di aspettare
            let halves = if replay.is_some()
                || s.quality_mode
                || !self.parallel_enabled.load(Ordering::Relaxed)
                || !self.engine.is_ready()
                || self.engine.is_warming()
                || llama::SLOTS < 2
            {
                None
            } else {
                postprocess::split_halves(&marked)
            };
            let mut out = match halves {
                Some((a, b, at_end)) => self.run_parallel(ctx, &system, &a, &b, at_end, lists_allowed, &entries, &mut partial).await,
                None => None,
            };
            if let Some(r) = replay {
                out = r;
            } else if out.is_none() {
                out = self.run_model(&s, &system, &user, max_tokens, &mut partial, &mut result).await;
                // guardia sul prompt: se l'uscita ricopia pezzi del prompt si rifà una volta,
                // poi si usa il grezzo con le sostituzioni deterministiche
                let leaks = |o: &str| postprocess::prompt_leak(&postprocess::sanitize_model_output(o), &user, &system);
                if out.as_deref().is_some_and(leaks) {
                    log::warn!("pulizia: l'uscita contiene pezzi del prompt, riprovo una volta");
                    let mut sink = |_: &str| {};
                    out = self.run_model(&s, &system, &user, max_tokens, &mut sink, &mut result).await;
                    if out.as_deref().is_some_and(leaks) {
                        log::warn!("pulizia: pezzi del prompt anche al secondo tentativo, uso il grezzo");
                        out = None;
                    }
                }
            }
            log::debug!("pulizia: modello in {:?}", started.elapsed());
            match out {
                Some(o) => {
                    let o = postprocess::sanitize_model_output(&o);
                    // termini del dizionario che non suonano come il grezzo tornano al
                    // grezzo; nei livelli che non accorciano, i tratti tolti senza motivo tornano
                    let (o, far) = postprocess::revert_far_terms(&marked, &o, &rules.protected_terms);
                    let (o, back) = if matches!(ctx.level, CleanupLevel::Exact | CleanupLevel::Tidy) {
                        postprocess::restore_dropped(&marked, &o, &rules.protected_terms)
                    } else {
                        (o, 0)
                    };
                    // niente parole cambiate: sostituzioni e aggiunte non ammesse tornano
                    // al grezzo, negli stessi livelli che non accorciano
                    let (o, changed) = if self.meaning_guard.load(Ordering::Relaxed)
                        && matches!(ctx.level, CleanupLevel::Exact | CleanupLevel::Tidy)
                    {
                        let mut terms = rules.protected_terms.clone();
                        terms.extend(ctx.proper_nouns.iter().cloned());
                        match self.correction_terms_override.read().unwrap().as_ref() {
                            Some(v) => terms.extend(v.iter().cloned()),
                            None => terms.extend(dictionary::correction_terms()),
                        }
                        postprocess::revert_changed_words(&marked, &o, &terms)
                    } else {
                        (o, 0)
                    };
                    if far + back + changed > 0 {
                        log::debug!("pulizia: termini lontani tolti {far}, tratti rimessi {back}, parole cambiate riportate {changed}");
                    }
                    let unmarked = postprocess::unmark_snippets(&o, &snippets);
                    match unmarked {
                        Some(u) if !postprocess::looks_like_answer(&marked, &o) => {
                            // controllo dei termini contro il grezzo già sostituito
                            let mut terms = rules.protected_terms.clone();
                            terms.extend(ctx.proper_nouns.iter().cloned());
                            match self.correction_terms_override.read().unwrap().as_ref() {
                                Some(v) => terms.extend(v.iter().cloned()),
                                None => terms.extend(dictionary::correction_terms()),
                            }
                            let (fixed, n) = postprocess::restore_terms(&pre_text, &u, &terms, &dictionary::correction_spellings());
                            result.restored_terms = n;
                            fixed
                        }
                        _ => {
                            log::warn!("pulizia: uscita del modello scartata, uso il grezzo");
                            result.skipped = true;
                            postprocess::basic_punctuation(&pre_text, false)
                        }
                    }
                }
                None => {
                    result.skipped = true;
                    postprocess::basic_punctuation(&pre_text, false)
                }
            }
        };

        // 5. dopo il modello: sostituzioni, snippet, backtrack residuo
        let post = rules.apply_replacements(&body);
        result.replacements += post.count;
        used_ids.extend(post.used_ids);
        let sn = rules.apply_snippets(&post.text);
        result.replacements += sn.count;
        used_ids.extend(sn.used_ids);
        body = sn.text;
        if s.backtrack_enabled {
            body = postprocess::backtrack(&body);
        }
        if ctx.level != CleanupLevel::Exact && ctx.format != TranscriptFormat::Raw {
            body = postprocess::drop_fillers(&body);
        }

        // 6. elenchi, formato, maiuscole, spazi
        let lists_allowed = s.lists_enabled && !matches!(ctx.category.as_str(), "terminal" | "chat_personal");
        if !lists_allowed {
            body = postprocess::flatten_lists(&body);
        }
        body = postprocess::fix_spacing(&body);
        body = postprocess::fix_sentence_caps(&body);
        body = match ctx.format {
            TranscriptFormat::Chat => postprocess::lowercase_sentences(&body, &rules.keep_case),
            TranscriptFormat::Lower => body.to_lowercase(),
            _ => body,
        };
        if ctx.format != TranscriptFormat::Lower {
            body = postprocess::apply_field_context(&body, ctx.field_before.as_deref(), &rules.keep_case);
        }
        result.text = body;
        used_ids.sort();
        used_ids.dedup();
        if !used_ids.is_empty() {
            let _ = self.dictionary.touch(&used_ids);
        }
        Ok(result)
    }

    /// Modello di qualità se attivo e con chiave, altrimenti locale. `None` se nessuno risponde.
    async fn run_model(
        &self,
        s: &CleanupSettings,
        system: &str,
        user: &str,
        max_tokens: u32,
        partial: &mut (dyn FnMut(&str) + Send),
        result: &mut Cleaned,
    ) -> Option<String> {
        if s.quality_mode {
            match quality::stream_message(&s.quality_model, system, user, max_tokens, partial).await {
                Ok(t) => {
                    result.used_quality = true;
                    return Some(t);
                }
                Err(e) => self.quality_fallback_notice(&e),
            }
        }
        if !self.engine.is_ready() {
            log::warn!("pulizia: llama-server non pronto, solo regole deterministiche");
            return None;
        }
        match self.engine.stream_chat(system, user, max_tokens, partial).await {
            Ok(t) => Some(t),
            Err(e) => {
                log::warn!("pulizia: llama-server ha risposto con un errore: {e}");
                None
            }
        }
    }

    /// Due metà della dettatura in parallelo sugli slot 0 e 1 di llama-server; `None` se una
    /// delle due fallisce o sembra una risposta (il chiamante rifà tutto in una richiesta sola).
    #[allow(clippy::too_many_arguments)]
    async fn run_parallel(
        &self,
        ctx: &CleanupContext,
        system: &str,
        a: &str,
        b: &str,
        at_end: bool,
        lists_allowed: bool,
        entries: &[DictionaryEntry],
        partial: &mut (dyn FnMut(&str) + Send),
    ) -> Option<String> {
        let ua = self.prompts.cleanup_user(ctx.level, ctx, a, lists_allowed, entries);
        let ub = self.prompts.cleanup_user(ctx.level, ctx, b, lists_allowed, entries);
        let ma = (postprocess::word_count(a) as u32 * 2 + 48).min(3000);
        let mb = (postprocess::word_count(b) as u32 * 2 + 48).min(3000);
        let shared = std::sync::Mutex::new((String::new(), String::new(), partial));
        let push = |which: usize, t: &str| {
            let mut g = shared.lock().unwrap();
            if which == 0 {
                g.0 = t.to_string();
            } else {
                g.1 = t.to_string();
            }
            let joined = format!("{} {}", g.0, g.1);
            (g.2)(joined.trim());
        };
        let mut pa = |t: &str| push(0, t);
        let mut pb = |t: &str| push(1, t);
        let (ra, rb) = futures_util::future::join(
            self.engine.stream_chat_slot(system, &ua, ma, Some(0), &mut pa),
            self.engine.stream_chat_slot(system, &ub, mb, Some(1), &mut pb),
        )
        .await;
        let (oa, ob) = match (ra, rb) {
            (Ok(x), Ok(y)) => (postprocess::sanitize_model_output(&x), postprocess::sanitize_model_output(&y)),
            (ea, eb) => {
                log::warn!("pulizia in parallelo fallita: {:?} {:?}", ea.err(), eb.err());
                return None;
            }
        };
        // guardia sul prompt per ogni metà, prima di riunirle
        let oa = self.guard_half(system, &ua, a, oa, ma).await;
        let ob = self.guard_half(system, &ub, b, ob, mb).await;
        if postprocess::looks_like_answer(a, &oa) || postprocess::looks_like_answer(b, &ob) {
            return None;
        }
        let joined = postprocess::join_halves(&oa, &ob, at_end, b);
        // e sull'uscita intera: se trapela ancora, il chiamante rifà tutto in una richiesta sola
        if postprocess::prompt_leak(&joined, &ua, system) || postprocess::prompt_leak(&joined, &ub, system) {
            return None;
        }
        Some(joined)
    }

    /// Una metà con pezzi del prompt si rifà una volta in una richiesta sola; se trapela
    /// ancora, resta il grezzo di quella metà (le sostituzioni deterministiche arrivano dopo).
    async fn guard_half(&self, system: &str, user: &str, raw: &str, out: String, max_tokens: u32) -> String {
        let engine = Arc::clone(&self.engine);
        guard_leak(out, user, system, raw, move || async move {
            let mut sink = |_: &str| {};
            engine.stream_chat_slot(system, user, max_tokens, None, &mut sink).await.ok()
        })
        .await
    }

    /// Senza chiave o senza rete l'app resta in locale: solo un evento di stato, nessun errore.
    fn quality_fallback_notice(&self, e: &quality::QualityError) {
        let detail = match e {
            quality::QualityError::KeyMissing => "key_missing".to_string(),
            quality::QualityError::Network(m) => format!("network:{m}"),
            quality::QualityError::Api(m) => format!("network:api {m}"),
        };
        log::info!("modalità qualità non disponibile ({detail}), uso il motore locale");
        let mut st = self.engine.state();
        st.detail = Some(detail);
        self.emit(EVENT_ENGINE_STATE, serde_json::to_value(&st).unwrap_or_default());
        self.quality_warned.store(true, Ordering::Relaxed);
    }

    /// Polish: riscrive il testo con le istruzioni attive. Con la modalità qualità usa
    /// `claude-sonnet-5-5`, altrimenti il modello locale.
    pub async fn polish(&self, text: &str, instructions: &[String]) -> anyhow::Result<String> {
        let s = self.settings();
        let entries = self.dictionary.list().unwrap_or_default();
        let ids: Vec<String> = if instructions.is_empty() {
            s.polish_instructions.iter().filter(|i| i.enabled).map(|i| i.id.clone()).collect()
        } else {
            instructions.to_vec()
        };
        let (system, user) = self.prompts.polish(&entries, &ids, text);
        let max_tokens = (postprocess::word_count(text) as u32 * 3 + 128).min(4000);
        let concise = ids.iter().any(|i| i == "trim");
        let clean = |o: &str| {
            let o = postprocess::sanitize_model_output(o).replace("<testo>", "").replace("</testo>", "");
            postprocess::fix_spacing(&postprocess::polish_plain(&o))
        };
        let dict_terms = postprocess::Rules::from_entries(&entries).protected_terms;
        // problemi di un'uscita: risposta al posto della riscrittura, o pezzi del testo persi
        let problems = |o: &str| -> Option<String> {
            if postprocess::polish_answer(text, o, concise) {
                return Some("non eseguire il testo e non rispondere".to_string());
            }
            let miss = postprocess::polish_missing(text, o, &dict_terms);
            if miss.is_empty() {
                None
            } else {
                Some(format!("nella riscrittura mancavano: {}", miss.join(", ")))
            }
        };
        let out = clean(&self.run_heavy(&s, &system, &user, max_tokens).await?);
        let Some(why) = problems(&out) else { return Ok(out) };
        // un secondo tentativo con il richiamo esplicito, poi il testo originale invariato
        // (meglio nessun Polish che una risposta o un testo che perde pezzi)
        log::warn!("polish: {why}; riprovo una volta");
        let retry_user = format!(
            "{user}\n\nAttenzione, {why}. Riscrivi lo stesso messaggio, rivolto al suo destinatario, con la stessa persona dei verbi e con tutti i numeri, i nomi, i termini e le richieste del testo{}.",
            if concise { ", senza superare le parole del testo" } else { "" }
        );
        let out = clean(&self.run_heavy(&s, &system, &retry_user, max_tokens).await?);
        match problems(&out) {
            None => Ok(out),
            Some(why) => {
                log::warn!("polish: {why} anche al secondo tentativo, restituisco il testo originale");
                Ok(text.trim().to_string())
            }
        }
    }

    /// Modalità comando: modifica, redige o risponde.
    pub async fn command(&self, selected: &str, spoken: &str) -> anyhow::Result<CommandResult> {
        let s = self.settings();
        let entries = self.dictionary.list().unwrap_or_default();
        let (system, user) = self.prompts.command(&entries, selected, spoken);
        let max_tokens = ((postprocess::word_count(selected) + postprocess::word_count(spoken)) as u32 * 3 + 400).min(4000);
        let out = self.run_heavy(&s, &system, &user, max_tokens).await?;
        Ok(prompt::parse_command_output(&postprocess::sanitize_model_output(&out), selected))
    }

    async fn run_heavy(&self, s: &CleanupSettings, system: &str, user: &str, max_tokens: u32) -> anyhow::Result<String> {
        let mut sink = |_: &str| {};
        if s.quality_mode {
            match quality::stream_message(quality::HEAVY_QUALITY_MODEL, system, user, max_tokens, &mut sink).await {
                Ok(t) => return Ok(t),
                Err(e) => self.quality_fallback_notice(&e),
            }
        }
        if !self.engine.is_ready() {
            anyhow::bail!("motore locale non pronto");
        }
        self.engine.stream_chat(system, user, max_tokens, &mut sink).await
    }

    /// Vocabolario per il prompt dell'ASR: frasi del dizionario non snippet, stellate per prime,
    /// poi per frequenza.
    pub fn vocabulary_for_asr(&self) -> Vec<String> {
        self.dictionary.vocabulary().unwrap_or_default()
    }

    /// Apprendimento dalle correzioni: filtri locali, classificatore sul Qwen locale con
    /// tetto di 1,5 s (mai l'API, anche con la modalità qualità accesa), voci `user_edits` subito
    /// nel dizionario, evento `cleanup:learned` per ogni parola nuova.
    pub async fn learn_from_observation(&self, obs: &learn::Observation) -> Vec<learn::Learned> {
        self.note_kept_forms(&obs.edited);
        let cands = match learn::prepare(obs, &self.dictionary) {
            Ok(c) => c,
            Err(why) => {
                log::debug!("apprendimento: niente da imparare in {} ({why:?})", obs.app_name);
                return Vec::new();
            }
        };
        let Some(decisions) = self.classify(&cands).await else {
            log::info!("apprendimento: classificatore non disponibile, nessuna parola imparata");
            return Vec::new();
        };
        let learned = learn::commit(&cands, &decisions, &self.dictionary);
        // la forma sbagliata appena imparata compare ancora, tenuta dall'utente, altrove nel testo
        self.note_kept_forms(&obs.edited);
        for l in learned.iter().filter(|l| l.new) {
            self.emit(EVENT_LEARNED, serde_json::json!({ "id": l.id, "wrong": l.wrong, "right": l.right }));
        }
        if !learned.is_empty() {
            self.emit(EVENT_DICTIONARY_CHANGED, serde_json::json!({}));
        }
        learned
    }

    /// Sostituzioni imparate la cui forma sbagliata l'utente ha scritto di suo: solo vocabolario.
    fn note_kept_forms(&self, edited: &str) {
        match self.dictionary.note_kept_forms(edited) {
            Ok(0) => {}
            Ok(n) => {
                log::info!("apprendimento: {n} sostituzioni imparate diventano solo vocabolario (forma tenuta nel testo corretto)");
                self.system_cache.lock().unwrap().clear();
                self.emit(EVENT_DICTIONARY_CHANGED, serde_json::json!({}));
            }
            Err(e) => log::warn!("apprendimento: forme tenute non controllate: {e}"),
        }
    }

    /// Migrazione una tantum dei candidati `pending` del vecchio meccanismo (da chiamare a motore
    /// pronto): filtri locali, poi classificatore a gruppi di 4. Restituisce (imparate, scartate
    /// dai filtri, scartate dal classificatore, rimaste in attesa perché il classificatore non
    /// ha risposto: si riprovano al prossimo avvio).
    pub async fn migrate_pending_candidates(&self) -> (u32, u32, u32, u32) {
        let pending = self.dictionary.pending_candidates().unwrap_or_default();
        let (keep, drop) = learn::triage_pending(&pending, &self.dictionary);
        for (id, why) in &drop {
            log::debug!("migrazione: candidato {id} scartato ({why})");
            let _ = self.dictionary.set_candidate_status(id, "filtered");
        }
        let (mut learned, mut rejected, mut left) = (0, 0, 0);
        for batch in keep.chunks(learn::MAX_CANDIDATES) {
            let cands: Vec<learn::Candidate> = batch.iter().map(|(_, c)| c.clone()).collect();
            let Some(d) = self.classify(&cands).await else {
                left += batch.len() as u32;
                continue;
            };
            for l in learn::commit(&cands, &d, &self.dictionary) {
                if l.new {
                    learned += 1;
                    self.emit(EVENT_LEARNED, serde_json::json!({ "id": l.id, "wrong": l.wrong, "right": l.right }));
                }
            }
            for ((id, _), ok) in batch.iter().zip(&d) {
                let _ = self.dictionary.set_candidate_status(id, if *ok { "accepted" } else { "rejected" });
                if !ok {
                    rejected += 1;
                }
            }
        }
        if learned > 0 {
            self.emit(EVENT_DICTIONARY_CHANGED, serde_json::json!({}));
        }
        log::info!("migrazione dei candidati: {learned} imparate, {} filtrate, {rejected} rifiutate, {left} in attesa", drop.len());
        (learned, drop.len() as u32, rejected, left)
    }

    /// Classificatore locale: `None` se il motore non è pronto, non risponde in tempo o l'uscita
    /// non passa la validazione.
    pub async fn classify(&self, cands: &[learn::Candidate]) -> Option<Vec<bool>> {
        if !self.engine.is_ready() {
            return None;
        }
        let t = &self.prompts.classifier;
        let user = learn::classifier_user(&t.user, cands);
        let started = Instant::now();
        let out = self
            .engine
            .chat_json(&t.system, &user, 16 + 8 * cands.len() as u32, learn::classifier_schema(cands.len()), learn::CLASSIFIER_TIMEOUT)
            .await;
        // la richiesta ha preso uno slot: il prefisso della pulizia torna in cache in background
        self.rewarm();
        match out {
            Ok(raw) => {
                let d = learn::parse_decisions(&raw, cands.len());
                log::info!("apprendimento: classificatore in {:?}, {} candidati, esito {:?}", started.elapsed(), cands.len(), d);
                d
            }
            Err(e) => {
                log::info!("apprendimento: classificatore fallito in {:?}: {e}", started.elapsed());
                None
            }
        }
    }

    /// Consuma `win:text-corrected`: aggiorna i candidati appresi. Restituisce quanti candidati
    /// sono stati creati o incrementati.
    pub fn observe_correction(&self, pasted: &str, edited: &str, changes: &[TextChange]) -> u32 {
        match self.dictionary.observe_correction(pasted, edited, changes) {
            Ok((n, visible)) => {
                if visible {
                    self.emit(EVENT_DICTIONARY_CHANGED, serde_json::json!({}));
                }
                n
            }
            Err(e) => {
                log::warn!("dizionario: osservazione non registrata: {e}");
                0
            }
        }
    }
}

/// Guardia sul prompt di un'uscita già ripulita (`sanitize_model_output`): se non ricopia pezzi
/// del prompt la restituisce; altrimenti chiama `retry` una volta; se anche quella trapela (o
/// fallisce), restituisce il grezzo con la punteggiatura di base.
pub async fn guard_leak<F, Fut>(out: String, user: &str, system: &str, raw: &str, retry: F) -> String
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Option<String>>,
{
    if !postprocess::prompt_leak(&out, user, system) {
        return out;
    }
    log::warn!("pulizia: una metà contiene pezzi del prompt, la rifaccio in una richiesta sola");
    if let Some(r) = retry().await {
        let r = postprocess::sanitize_model_output(&r);
        if !r.trim().is_empty() && !postprocess::prompt_leak(&r, user, system) && !postprocess::looks_like_answer(raw, &r) {
            return r;
        }
    }
    log::warn!("pulizia: pezzi del prompt anche al secondo tentativo, uso il grezzo della metà");
    postprocess::basic_punctuation(raw, false)
}

#[cfg(test)]
mod tests {
    use super::*;

    const USER: &str = "<contesto>\nFormato: testo normale.\n</contesto>\n<trascrizione>\nparola uno e parola due\n</trascrizione>\nTesto pulito:";

    #[tokio::test]
    async fn guardia_rifà_una_volta_poi_usa_il_grezzo() {
        let raw = "parola uno e parola due";
        // uscita pulita: nessun secondo tentativo
        let ok = guard_leak("Parola uno e parola due.".into(), USER, "", raw, || async { panic!("non deve riprovare") }).await;
        assert_eq!(ok, "Parola uno e parola due.");
        // eco del prompt: secondo tentativo pulito, si tiene quello
        let retry = guard_leak("Formato: testo normale.\nParola uno.".into(), USER, "", raw, || async {
            Some("Parola uno e parola due.".to_string())
        })
        .await;
        assert_eq!(retry, "Parola uno e parola due.");
        // eco anche al secondo tentativo: grezzo con la punteggiatura di base
        let fallback = guard_leak("<contesto>".into(), USER, "", raw, || async { Some("Dizionario:\n- CODEX".to_string()) }).await;
        assert_eq!(fallback, "Parola uno e parola due");
    }

    // catena intera con un'uscita del modello registrata: le parole cambiate tornano, la
    // sostituzione del dizionario e la punteggiatura restano; con la guardia spenta no
    #[tokio::test]
    async fn catena_riporta_le_parole_cambiate() {
        let dict = Dictionary::open_in_memory().unwrap();
        dict.add(DictionaryEntry {
            id: String::new(),
            phrase: "nome prova".into(),
            replacement: Some("NomeProva".into()),
            is_snippet: false,
            starred: false,
            frequency: 0,
            last_used: None,
            source: DictionarySource::Manual,
            ambiguous: false,
        })
        .unwrap();
        let dir = std::env::temp_dir().join("eaglevoice-test-senso");
        let mgr = CleanupManager::with_dictionary(CleanupSettings::default(), dict, &dir, null_emitter());
        let ctx = mgr.settings().context_for("Claude", "ai");
        let raw = "parola uno e gli ho mostrato nome prova parola due";
        let model = "Parola uno egli mi ha mostrato NomeProva, parola due.".to_string();
        let c = mgr.clean_replay(raw, &ctx, Some(model.clone())).await.unwrap();
        assert_eq!(c.text, "Parola uno e gli ho mostrato NomeProva, parola due.");
        mgr.meaning_guard.store(false, Ordering::Relaxed);
        let c = mgr.clean_replay(raw, &ctx, Some(model)).await.unwrap();
        assert_eq!(c.text, "Parola uno egli mi ha mostrato NomeProva, parola due.");
    }
}

impl Drop for CleanupManager {
    fn drop(&mut self) {
        self.engine.stop();
    }
}
