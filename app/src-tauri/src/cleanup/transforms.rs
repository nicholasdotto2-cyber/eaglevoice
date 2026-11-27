//! Motore dei Transforms: un prompt applicato a un testo già scritto, su scorciatoia.
//!
//! Slot 1 Polish (le istruzioni a casella di oggi), slot 2 Prompt Builder (prompt nostro,
//! `prompts\prompt-builder.txt`), slot 3-9 personalizzati (nome, prompt, scorciatoia, fino a
//! 5 esempi di 50-500 parole). Gira sul Qwen locale; con la modalità qualità accesa e la chiave
//! presente va su Haiku via `quality.rs`. Con la modalità qualità spenta nessun dato esce dal PC.
//!
//! Limiti: 1-3000 parole. llama-server gira con `-c 8192 -np 2`, cioè 4096 token per slot fra
//! entrata e uscita: il limite di una richiesta locale si calcola dai token del prompt di
//! sistema (contati da `/tokenize`) e da `TOKENS_PER_WORD` stimato; oltre, il Polish si spezza
//! per paragrafi (due pezzi alla volta, uno per slot), gli altri Transform rifiutano con un
//! messaggio. Con un errore del modello il chiamante riceve `Err` e il testo resta com'è.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::sync::{Arc, RwLock};
use std::time::Instant;

use super::postprocess::{self, tokens, word_count};
use super::prompt::Template;
use super::{learn, quality, CleanupContext, CleanupManager, CleanupSettings, TextChange};

pub const MIN_SLOT: u8 = 1;
pub const MAX_SLOT: u8 = 9;
pub const POLISH_SLOT: u8 = 1;
pub const PROMPT_ENGINEER_SLOT: u8 = 2;
pub const FIRST_CUSTOM_SLOT: u8 = 3;
pub const MAX_WORDS: usize = 3000;
pub const MAX_EXAMPLES: usize = 5;
pub const EXAMPLE_MIN_WORDS: usize = 50;
pub const EXAMPLE_MAX_WORDS: usize = 500;
/// Token per slot di llama-server (`-c 8192 -np 2`).
pub const SLOT_CONTEXT_TOKENS: usize = 4096;
/// Token per parola del tokenizer di Qwen3.5 su testo italiano dettato, con un po' di margine
/// sopra il valore tipico (si controlla con `prova_transforms_latenza_e_limiti`).
pub const TOKENS_PER_WORD: f64 = 1.35;
/// Margine per il modello di chat e i tag del messaggio utente.
const OVERHEAD_TOKENS: usize = 96;

/// Uscita prevista rispetto all'entrata, per il tetto di `max_tokens` e il limite di parole.
fn output_ratio(slot: u8) -> f64 {
    match slot {
        POLISH_SLOT => 1.3,
        PROMPT_ENGINEER_SLOT => 2.0,
        _ => 1.6,
    }
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct TransformSlot {
    pub slot: u8,
    pub name: String,
    /// Istruzioni del Transform; vuote per Polish (usa le istruzioni a casella) e fisse per
    /// Prompt Builder.
    pub prompt: String,
    /// Scorciatoia nel formato delle altre (`super+alt+1`); la registra `shortcut\`.
    pub shortcut: String,
    /// Esempi del risultato voluto, fino a 5, ognuno di 50-500 parole.
    pub examples: Vec<String>,
    pub builtin: bool,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct AutoApply {
    pub active: bool,
    pub slot: u8,
}

/// Campo `transforms` di `AppSettings`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
#[serde(default)]
pub struct TransformsSettings {
    pub enabled: bool,
    pub auto_apply: AutoApply,
    pub slots: Vec<TransformSlot>,
}

impl Default for TransformsSettings {
    fn default() -> Self {
        TransformsSettings {
            enabled: true,
            auto_apply: AutoApply { active: false, slot: POLISH_SLOT },
            slots: builtin_slots(),
        }
    }
}

/// Risposta di `get_transforms`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct TransformsState {
    pub enabled: bool,
    pub auto_apply: AutoApply,
    pub slots: Vec<TransformSlot>,
    /// Parole al massimo per un Transform (3000).
    pub max_words: u32,
    /// Parole al massimo in una richiesta sola sul motore locale (oltre: Polish a pezzi, gli
    /// altri rifiutano), calcolate con il prompt di Prompt Builder.
    pub local_max_words: u32,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct TransformResult {
    pub text: String,
    pub changed: bool,
    /// Gruppi di parole cambiate, per il diff "Vedi modifiche" (un lato vuoto: parole aggiunte o tolte).
    pub changes: Vec<TextChange>,
    pub ms: u32,
}

pub fn default_shortcut(slot: u8) -> String {
    format!("super+alt+{slot}")
}

pub fn builtin_slots() -> Vec<TransformSlot> {
    vec![
        TransformSlot {
            slot: POLISH_SLOT,
            name: "Polish".into(),
            prompt: String::new(),
            shortcut: default_shortcut(POLISH_SLOT),
            examples: Vec::new(),
            builtin: true,
        },
        TransformSlot {
            slot: PROMPT_ENGINEER_SLOT,
            name: "Prompt Builder".into(),
            prompt: String::new(),
            shortcut: default_shortcut(PROMPT_ENGINEER_SLOT),
            examples: Vec::new(),
            builtin: true,
        },
    ]
}

/// Controlli di un Transform personalizzato prima del salvataggio.
pub fn validate_custom(s: &TransformSlot) -> Result<(), String> {
    if !(FIRST_CUSTOM_SLOT..=MAX_SLOT).contains(&s.slot) {
        return Err(format!("i Transform personalizzati vanno negli slot da {FIRST_CUSTOM_SLOT} a {MAX_SLOT}"));
    }
    if s.name.trim().is_empty() {
        return Err("serve un nome".into());
    }
    if s.prompt.trim().is_empty() {
        return Err("serve un prompt".into());
    }
    if s.examples.len() > MAX_EXAMPLES {
        return Err(format!("al massimo {MAX_EXAMPLES} esempi"));
    }
    for (i, e) in s.examples.iter().enumerate() {
        let n = word_count(e);
        if !(EXAMPLE_MIN_WORDS..=EXAMPLE_MAX_WORDS).contains(&n) {
            return Err(format!("l'esempio {} ha {n} parole: ne servono da {EXAMPLE_MIN_WORDS} a {EXAMPLE_MAX_WORDS}", i + 1));
        }
    }
    Ok(())
}

pub type PersistFn = Arc<dyn Fn(&TransformsSettings) + Send + Sync>;

/// Impostazioni dei Transforms in memoria, con la funzione che le salva in `AppSettings`
/// (impostata all'avvio dell'app con `set_persist`).
#[derive(Default)]
pub struct TransformsStore {
    settings: RwLock<TransformsSettings>,
    persist: RwLock<Option<PersistFn>>,
}

impl TransformsStore {
    /// Impostazioni lette da `AppSettings` all'avvio: i due slot di serie ci sono sempre.
    pub fn load(&self, mut s: TransformsSettings) {
        for b in builtin_slots() {
            match s.slots.iter_mut().find(|x| x.slot == b.slot) {
                Some(x) => {
                    let shortcut = std::mem::take(&mut x.shortcut);
                    *x = TransformSlot { shortcut: if shortcut.trim().is_empty() { b.shortcut.clone() } else { shortcut }, ..b };
                }
                None => s.slots.push(b),
            }
        }
        s.slots.retain(|x| x.builtin || validate_custom(x).is_ok());
        s.slots.sort_by_key(|x| x.slot);
        *self.settings.write().unwrap() = s;
    }

    pub fn set_persist(&self, f: PersistFn) {
        *self.persist.write().unwrap() = Some(f);
    }

    pub fn get(&self) -> TransformsSettings {
        self.settings.read().unwrap().clone()
    }

    pub fn slot(&self, slot: u8) -> Option<TransformSlot> {
        self.settings.read().unwrap().slots.iter().find(|s| s.slot == slot).cloned()
    }

    fn write(&self, f: impl FnOnce(&mut TransformsSettings) -> Result<(), String>) -> Result<(), String> {
        let snapshot = {
            let mut s = self.settings.write().unwrap();
            let mut next = s.clone();
            f(&mut next)?;
            next.slots.sort_by_key(|x| x.slot);
            *s = next.clone();
            next
        };
        if let Some(p) = self.persist.read().unwrap().as_ref() {
            p(&snapshot);
        }
        Ok(())
    }

    /// Salva uno slot: dei due di serie cambia solo la scorciatoia.
    pub fn save(&self, slot: TransformSlot) -> Result<(), String> {
        self.write(|s| {
            if slot.slot == POLISH_SLOT || slot.slot == PROMPT_ENGINEER_SLOT {
                let b = s.slots.iter_mut().find(|x| x.slot == slot.slot).ok_or("slot di serie mancante")?;
                if !slot.shortcut.trim().is_empty() {
                    b.shortcut = slot.shortcut.trim().to_string();
                }
                return Ok(());
            }
            let mut slot = slot;
            slot.builtin = false;
            slot.name = slot.name.trim().to_string();
            if slot.shortcut.trim().is_empty() {
                slot.shortcut = default_shortcut(slot.slot);
            }
            validate_custom(&slot)?;
            s.slots.retain(|x| x.slot != slot.slot);
            s.slots.push(slot);
            Ok(())
        })
    }

    pub fn delete(&self, slot: u8) -> Result<(), String> {
        self.write(|s| {
            if slot < FIRST_CUSTOM_SLOT {
                return Err("Polish e Prompt Builder non si eliminano".into());
            }
            s.slots.retain(|x| x.slot != slot);
            if s.auto_apply.slot == slot {
                s.auto_apply = AutoApply { active: false, slot: POLISH_SLOT };
            }
            Ok(())
        })
    }

    pub fn reset(&self) {
        let _ = self.write(|s| {
            *s = TransformsSettings::default();
            Ok(())
        });
    }

    pub fn set_options(&self, enabled: bool, auto_apply_active: bool, auto_apply_slot: u8) -> Result<(), String> {
        self.write(|s| {
            if !s.slots.iter().any(|x| x.slot == auto_apply_slot) {
                return Err(format!("nessun Transform nello slot {auto_apply_slot}"));
            }
            s.enabled = enabled;
            s.auto_apply = AutoApply { active: auto_apply_active, slot: auto_apply_slot };
            Ok(())
        })
    }
}

/// Dove gira un Transform. Con la modalità qualità spenta sempre in locale.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Local,
    Quality,
}

pub fn backend(quality_mode: bool, key_present: bool) -> Backend {
    if quality_mode && key_present {
        Backend::Quality
    } else {
        Backend::Local
    }
}

/// Parole al massimo in una richiesta locale, dati i token del prompt di sistema.
pub fn local_max_words(system_tokens: usize, slot: u8) -> usize {
    let free = SLOT_CONTEXT_TOKENS.saturating_sub(system_tokens + OVERHEAD_TOKENS) as f64;
    (free / (TOKENS_PER_WORD * (1.0 + output_ratio(slot)))).floor() as usize
}

/// Pezzi di al massimo `limit` parole: per paragrafi, poi per frasi, poi per parole. Ogni pezzo
/// porta il separatore che lo seguiva nel testo.
pub fn split_chunks(text: &str, limit: usize) -> Vec<(String, String)> {
    let limit = limit.max(1);
    // unità: paragrafi, e frasi dei paragrafi troppo lunghi
    let mut units: Vec<(String, String)> = Vec::new();
    let paras: Vec<&str> = text.split('\n').collect();
    for (pi, p) in paras.iter().enumerate() {
        let sep = if pi + 1 < paras.len() { "\n" } else { "" };
        if word_count(p) <= limit {
            units.push((p.to_string(), sep.to_string()));
            continue;
        }
        let mut sentences: Vec<String> = Vec::new();
        let mut cur = String::new();
        for ch in p.chars() {
            cur.push(ch);
            if matches!(ch, '.' | '!' | '?') {
                sentences.push(std::mem::take(&mut cur));
            }
        }
        if !cur.is_empty() {
            sentences.push(cur);
        }
        let last = sentences.len().saturating_sub(1);
        for (si, s) in sentences.into_iter().enumerate() {
            let ssep = if si == last { sep } else { "" };
            if word_count(&s) <= limit {
                units.push((s, ssep.to_string()));
            } else {
                let words: Vec<&str> = s.split_whitespace().collect();
                let parts: Vec<String> = words.chunks(limit).map(|c| c.join(" ")).collect();
                let n = parts.len();
                for (k, w) in parts.into_iter().enumerate() {
                    units.push((if k == 0 { w } else { format!(" {w}") }, if k + 1 == n { ssep.to_string() } else { String::new() }));
                }
            }
        }
    }
    // unità consecutive insieme finché stanno nel limite
    let mut out: Vec<(String, String)> = Vec::new();
    for (u, sep) in units {
        if let Some(last) = out.last_mut() {
            if word_count(&last.0) + word_count(&u) <= limit {
                last.0.push_str(&last.1);
                last.0.push_str(&u);
                last.1 = sep;
                continue;
            }
        }
        out.push((u, sep));
    }
    out.retain(|(c, _)| !c.trim().is_empty() || word_count(c) > 0);
    out
}

/// Ripulisce l'uscita: niente `<think>`, solo il contenuto di `<tag>` se c'è, niente blocco di
/// codice che avvolge tutto, niente riga d'attacco "Ecco ...:".
pub fn clean_output(raw: &str, tag: Option<&str>) -> String {
    let mut t = postprocess::sanitize_think(raw);
    if let Some(tag) = tag {
        let (open, close) = (format!("<{tag}>"), format!("</{tag}>"));
        if let Some(i) = t.find(&open) {
            let rest = &t[i + open.len()..];
            t = rest.find(&close).map_or(rest, |j| &rest[..j]).to_string();
        } else if let Some(j) = t.find(&close) {
            t = t[..j].to_string();
        }
    }
    let mut s = t.trim().to_string();
    if s.starts_with("```") && s.ends_with("```") && s.len() > 6 {
        s = s[3..s.len() - 3].to_string();
        // via l'eventuale nome del linguaggio sulla prima riga
        if let Some(nl) = s.find('\n') {
            if !s[..nl].contains(' ') {
                s = s[nl + 1..].to_string();
            }
        }
    }
    let s = s.trim();
    if let Some(nl) = s.find('\n') {
        let first = s[..nl].trim();
        if first.starts_with("Ecco") && first.ends_with(':') {
            return s[nl + 1..].trim().to_string();
        }
    }
    s.to_string()
}

/// L'uscita ricopia pezzi del prompt di sistema (frasi di almeno 40 caratteri delle regole, non
/// degli esempi) che non stanno nel testo, o i segnaposto del messaggio utente?
pub fn transform_leak(output: &str, system: &str, input: &str) -> bool {
    const MARKS: &[&str] = &["=== UTENTE ===", "<dettatura>", "</dettatura>", "<istruzioni_utente>", "<risultato>", "</risultato>", "{{"];
    if MARKS.iter().any(|m| output.contains(m) && !input.contains(m)) {
        return true;
    }
    let (low, inp) = (output.to_lowercase(), input.to_lowercase());
    let rules = system.split("<esempi>").next().unwrap_or(system);
    for line in rules.lines() {
        for piece in line.split(". ") {
            let p = piece.trim().trim_start_matches(|c: char| c.is_ascii_digit() || c == '.' || c == '-').trim().trim_end_matches('.').to_lowercase();
            if p.chars().count() >= 40 && low.contains(&p) && !inp.contains(&p) {
                return true;
            }
        }
    }
    false
}

/// Gruppi di parole cambiate fra entrata e uscita.
pub fn word_changes(before: &str, after: &str) -> Vec<TextChange> {
    let (a, b) = (tokens(before), tokens(after));
    if a == b {
        return Vec::new();
    }
    learn::align(&a, &b).into_iter().map(|g| TextChange { from: g.wrong.join(" "), to: g.right.join(" ") }).collect()
}

pub struct TransformPrompts {
    pub prompt_engineer: Template,
    pub custom: Template,
}

impl TransformPrompts {
    pub fn load() -> Self {
        TransformPrompts {
            prompt_engineer: Template::parse(include_str!("../../prompts/prompt-builder.txt")),
            custom: Template::parse(include_str!("../../prompts/transform-personalizzato.txt")),
        }
    }

    pub fn custom_system(&self, slot: &TransformSlot) -> String {
        let examples = if slot.examples.is_empty() {
            String::new()
        } else {
            let items: Vec<String> = slot.examples.iter().map(|e| format!("<esempio>\n{}\n</esempio>", e.trim())).collect();
            format!("Esempi del risultato che l'utente vuole (tono, lunghezza, struttura):\n<esempi>\n{}\n</esempi>\n", items.join("\n"))
        };
        self.custom.system.replace("{{istruzioni}}", slot.prompt.trim()).replace("{{esempi}}\n", &examples).replace("{{esempi}}", &examples)
    }
}

impl CleanupManager {
    pub fn transforms_state(&self) -> TransformsState {
        let s = self.transforms.get();
        let sys = &self.transform_prompts.prompt_engineer.system;
        TransformsState {
            enabled: s.enabled,
            auto_apply: s.auto_apply,
            slots: s.slots,
            max_words: MAX_WORDS as u32,
            local_max_words: local_max_words(estimate_tokens(sys), PROMPT_ENGINEER_SLOT) as u32,
        }
    }

    /// Esegue il Transform dello slot sul testo. `Err` con un messaggio da mostrare se il testo
    /// è vuoto o troppo lungo, se lo slot non esiste o se il modello fallisce: il chiamante non
    /// tocca il campo. `Ok` con `changed` falso se il modello ha restituito lo stesso testo (o,
    /// per Polish, se due tentativi perdevano pezzi del testo).
    pub async fn transform(&self, slot: u8, text: &str, ctx: &CleanupContext) -> anyhow::Result<TransformResult> {
        let started = Instant::now();
        let ts = self.transforms.get();
        if !ts.enabled {
            anyhow::bail!("I Transforms sono disattivati");
        }
        let Some(def) = ts.slots.iter().find(|s| s.slot == slot).cloned() else {
            anyhow::bail!("Nessun Transform nello slot {slot}");
        };
        let words = word_count(text);
        if words == 0 {
            anyhow::bail!("Nessun testo da trasformare");
        }
        if words > MAX_WORDS {
            anyhow::bail!("Il testo ha {words} parole: {} accetta al massimo {MAX_WORDS} parole", def.name);
        }
        let s = self.settings();
        let be = backend(s.quality_mode, quality::key_present());
        let entries = self.dictionary.list().unwrap_or_default();
        let polish_ids: Vec<String> = s.polish_instructions.iter().filter(|i| i.enabled).map(|i| i.id.clone()).collect();
        // prompt di sistema dello slot (per Polish il testo sta solo nel messaggio utente)
        let system = match slot {
            POLISH_SLOT => self.prompts.polish(&entries, &polish_ids, "").0,
            PROMPT_ENGINEER_SLOT => self.transform_prompts.prompt_engineer.system.clone(),
            _ => self.transform_prompts.custom_system(&def),
        };
        let chunks: Vec<(String, String)> = if be == Backend::Local {
            let sys_tokens = match self.engine.count_tokens(&system).await {
                Ok(n) if n > 0 => n,
                _ => estimate_tokens(&system),
            };
            let limit = local_max_words(sys_tokens, slot);
            if words <= limit {
                vec![(text.to_string(), String::new())]
            } else if slot == POLISH_SLOT {
                split_chunks(text, limit)
            } else {
                anyhow::bail!(
                    "Il testo ha {words} parole: {} in locale ne regge al massimo {limit}. Selezionane una parte, o accendi la modalità qualità",
                    def.name
                );
            }
        } else {
            vec![(text.to_string(), String::new())]
        };
        log::info!("transform {} ({}): {words} parole in {} pezzi, {:?}, app {}", slot, def.name, chunks.len(), be, ctx.app_name);
        // due pezzi alla volta: uno per slot di llama-server
        let mut outs: Vec<String> = Vec::with_capacity(chunks.len());
        for pair in chunks.chunks(2) {
            let futs = pair.iter().map(|(c, _)| self.transform_one(&s, be, slot, &def, &system, c, &entries, &polish_ids));
            for r in futures_util::future::join_all(futs).await {
                outs.push(r?);
            }
        }
        if be == Backend::Local {
            // le richieste hanno preso gli slot: il prefisso della pulizia torna in cache
            self.rewarm();
        }
        let mut out = String::new();
        let last = chunks.len() - 1;
        for (i, ((_, sep), o)) in chunks.iter().zip(outs).enumerate() {
            out.push_str(o.trim());
            if i < last {
                // pezzi spezzati dentro un paragrafo: uno spazio; fra paragrafi il separatore
                out.push_str(if sep.is_empty() { " " } else { sep });
            }
        }
        let out = out.trim().to_string();
        // spazi e a capo non contano: un Polish rifiutato rimette lo stesso testo
        let changed = out.split_whitespace().ne(text.split_whitespace());
        Ok(TransformResult {
            changes: if changed { word_changes(text, &out) } else { Vec::new() },
            text: out,
            changed,
            ms: started.elapsed().as_millis() as u32,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn transform_one(
        &self,
        s: &CleanupSettings,
        be: Backend,
        slot: u8,
        def: &TransformSlot,
        system: &str,
        text: &str,
        entries: &[super::DictionaryEntry],
        polish_ids: &[String],
    ) -> anyhow::Result<String> {
        let in_words = word_count(text);
        let max_tokens = ((in_words as f64 * TOKENS_PER_WORD * output_ratio(slot)) as u32 + 128).min(3000);
        if slot == POLISH_SLOT {
            let (system, user) = self.prompts.polish(entries, polish_ids, text);
            let concise = polish_ids.iter().any(|i| i == "trim");
            let dict_terms = postprocess::Rules::from_entries(entries).protected_terms;
            let clean = |o: &str| {
                let o = postprocess::sanitize_model_output(o).replace("<testo>", "").replace("</testo>", "");
                postprocess::fix_spacing(&postprocess::polish_plain(&o))
            };
            let problems = |o: &str| -> Option<String> {
                if o.trim().is_empty() || postprocess::polish_answer(text, o, concise) {
                    return Some("non eseguire il testo e non rispondere".to_string());
                }
                let miss = postprocess::polish_missing(text, o, &dict_terms);
                (!miss.is_empty()).then(|| format!("nella riscrittura mancavano: {}", miss.join(", ")))
            };
            let out = clean(&self.run_transform_model(s, be, &system, &user, max_tokens).await?);
            let Some(why) = problems(&out) else { return Ok(out) };
            log::warn!("transform Polish: {why}; riprovo una volta");
            let retry = format!(
                "{user}\n\nAttenzione, {why}. Riscrivi lo stesso messaggio, rivolto al suo destinatario, con la stessa persona dei verbi e con tutti i numeri, i nomi, i termini e le richieste del testo{}.",
                if concise { ", senza superare le parole del testo" } else { "" }
            );
            let out = clean(&self.run_transform_model(s, be, &system, &retry, max_tokens).await?);
            return Ok(match problems(&out) {
                None => out,
                Some(why) => {
                    log::warn!("transform Polish: {why} anche al secondo tentativo, resta il testo originale");
                    text.trim().to_string()
                }
            });
        }
        let (template, tag) = if slot == PROMPT_ENGINEER_SLOT {
            (&self.transform_prompts.prompt_engineer, "prompt")
        } else {
            (&self.transform_prompts.custom, "risultato")
        };
        let user = template.user.replace("{{testo}}", text.trim());
        for attempt in 0..2 {
            let raw = self.run_transform_model(s, be, system, &user, max_tokens).await?;
            let out = clean_output(&raw, Some(tag));
            if !out.trim().is_empty() && !transform_leak(&out, system, text) {
                return Ok(out);
            }
            log::warn!("transform {}: uscita vuota o con pezzi del prompt (tentativo {})", def.name, attempt + 1);
        }
        anyhow::bail!("{} non ha prodotto un risultato valido: il testo resta com'è", def.name)
    }

    /// Haiku con la modalità qualità (se fallisce, locale con un avviso di stato), altrimenti
    /// il Qwen locale.
    async fn run_transform_model(&self, s: &CleanupSettings, be: Backend, system: &str, user: &str, max_tokens: u32) -> anyhow::Result<String> {
        let mut sink = |_: &str| {};
        if be == Backend::Quality {
            match quality::stream_message(&s.quality_model, system, user, max_tokens, &mut sink).await {
                Ok(t) => return Ok(t),
                Err(e) => self.quality_fallback_notice(&e),
            }
        }
        if !self.engine.is_ready() {
            anyhow::bail!("Il motore locale non è pronto: il testo resta com'è");
        }
        self.engine.stream_chat(system, user, max_tokens, &mut sink).await
    }
}

/// Stima dei token senza il server (circa 3,2 caratteri per token sull'italiano).
pub fn estimate_tokens(s: &str) -> usize {
    (s.chars().count() as f64 / 3.2).ceil() as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn custom(slot: u8, examples: Vec<String>) -> TransformSlot {
        TransformSlot { slot, name: "Stile di prova".into(), prompt: "Riscrivi il testo nello stile di prova.".into(), shortcut: String::new(), examples, builtin: false }
    }

    #[test]
    fn slot_di_serie_salvataggio_e_validazione() {
        let st = TransformsStore::default();
        st.load(TransformsSettings { slots: vec![], ..Default::default() });
        let saved = Arc::new(std::sync::Mutex::new(0));
        let s2 = saved.clone();
        st.set_persist(Arc::new(move |_| *s2.lock().unwrap() += 1));
        assert_eq!(st.get().slots.iter().map(|s| s.slot).collect::<Vec<_>>(), vec![1, 2]);
        st.save(custom(3, vec![])).unwrap();
        assert_eq!(st.slot(3).unwrap().shortcut, "super+alt+3");
        assert!(st.save(custom(10, vec![])).is_err());
        assert!(st.save(custom(4, vec!["troppo corto".into()])).is_err());
        assert!(st.save(custom(4, vec!["parola ".repeat(60); 6])).is_err());
        st.save(custom(4, vec!["parola ".repeat(60); 5])).unwrap();
        // Polish: cambia solo la scorciatoia, non nome né prompt
        st.save(TransformSlot { slot: 1, name: "Altro".into(), prompt: "x".into(), shortcut: "super+alt+p".into(), examples: vec![], builtin: false }).unwrap();
        let p = st.slot(1).unwrap();
        assert_eq!((p.name.as_str(), p.prompt.as_str(), p.shortcut.as_str(), p.builtin), ("Polish", "", "super+alt+p", true));
        assert!(st.delete(2).is_err());
        st.set_options(true, true, 4).unwrap();
        st.delete(4).unwrap();
        assert_eq!(st.get().auto_apply, AutoApply { active: false, slot: 1 });
        assert!(st.set_options(true, true, 7).is_err());
        st.reset();
        assert_eq!(st.get(), TransformsSettings::default());
        assert_eq!(*saved.lock().unwrap(), 6);
    }

    #[test]
    fn con_la_modalita_qualita_spenta_tutto_resta_in_locale() {
        assert_eq!(backend(false, true), Backend::Local);
        assert_eq!(backend(false, false), Backend::Local);
        assert_eq!(backend(true, false), Backend::Local);
        assert_eq!(backend(true, true), Backend::Quality);
    }

    #[test]
    fn limite_locale_e_pezzi_per_paragrafi() {
        // 1500 token di sistema: (4096 - 1596) / (1,35 * 2,3) = 805 parole per il Polish
        assert_eq!(local_max_words(1500, POLISH_SLOT), 805);
        let p = |n: usize| "parola ".repeat(n).trim().to_string() + ".";
        let text = format!("{}\n{}\n\n{}", p(300), p(300), p(500));
        let c = split_chunks(&text, 679);
        assert_eq!(c.len(), 2);
        assert_eq!(word_count(&c[0].0), 600);
        assert_eq!(word_count(&c[1].0), 500);
        let joined: String = c.iter().map(|(a, b)| format!("{a}{b}")).collect();
        assert_eq!(joined, text);
        // un paragrafo solo, oltre il limite: per frasi, poi per parole
        let long = format!("{} {}", p(400), p(400));
        let c = split_chunks(&long, 679);
        assert_eq!(c.iter().map(|x| word_count(&x.0)).collect::<Vec<_>>(), vec![400, 400]);
        assert!(split_chunks(&p(1500), 679).iter().all(|x| word_count(&x.0) <= 679));
    }

    #[test]
    fn uscita_pulita_e_guardia_sul_prompt() {
        let raw = "Ecco il prompt migliorato:\n<prompt>\n<compito>\nTesto del compito.\n</compito>\n</prompt>";
        assert_eq!(clean_output(raw, Some("prompt")), "<compito>\nTesto del compito.\n</compito>");
        assert_eq!(clean_output("```markdown\nTesto di prova\n```", None), "Testo di prova");
        assert_eq!(clean_output("Ecco il risultato:\nTesto nuovo.", None), "Testo nuovo.");
        let sys = TransformPrompts::load().prompt_engineer.system;
        assert!(!transform_leak("<compito>\nTesto del compito di prova.\n</compito>", &sys, "testo del compito di prova"));
        assert!(transform_leak("Scrivi il prompt nella lingua della dettatura: dettatura in italiano, prompt in italiano.", &sys, "testo di prova"));
        assert!(transform_leak("<dettatura>\ntesto di prova\n</dettatura>", &sys, "testo di prova"));
    }

    #[test]
    fn diff_delle_modifiche() {
        let c = word_changes("parola uno parola due prova", "Parola uno, parola due alfa beta");
        assert_eq!(c, vec![
            TextChange { from: "parola".into(), to: "Parola".into() },
            TextChange { from: "prova".into(), to: "alfa beta".into() },
        ]);
    }

    #[test]
    fn prompt_personalizzato_con_esempi() {
        let tp = TransformPrompts::load();
        let sys = tp.custom_system(&custom(3, vec!["Esempio di prova, ".repeat(30)]));
        assert!(sys.contains("Riscrivi il testo nello stile di prova.") && sys.contains("<esempio>") && !sys.contains("{{"));
        let none = tp.custom_system(&custom(3, vec![]));
        assert!(!none.contains("<esempi>") && !none.contains("{{"));
    }
}
