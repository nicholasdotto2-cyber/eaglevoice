//! Post-elaborazione deterministica della pulizia: tutto ciò che non deve dipendere dal modello.
//!
//! Sostituzioni e snippet del dizionario a confini di parola, "premi invio", rete di sicurezza
//! del backtrack, controllo dei termini (un termine presente nel grezzo non può sparire nel
//! pulito), elenchi appiattiti, formati minuscoli, maiuscola dal contesto del campo, spazi.

use once_cell::sync::Lazy;
use regex::Regex;
use std::collections::HashSet;

use super::DictionaryEntry;

/// Tokenizzazione (TOK): parole fatte di lettere e cifre,
/// unite da punto o trattino interni ("claude.md", "follow-up").
static TOK: Lazy<Regex> = Lazy::new(|| Regex::new(r"[^\W_]+(?:[.\-][^\W_]+)*").unwrap());
/// Etichette HTML semplici (TAG).
static TAG: Lazy<Regex> = Lazy::new(|| Regex::new(r"</?[a-zA-Z][a-zA-Z0-9]*\s*/?>").unwrap());

/// Token con la loro posizione in byte.
pub fn token_spans(s: &str) -> Vec<(usize, usize)> {
    TOK.find_iter(s).map(|m| (m.start(), m.end())).collect()
}

pub fn tokens(s: &str) -> Vec<String> {
    TOK.find_iter(s).map(|m| m.as_str().to_string()).collect()
}

pub fn ltokens(s: &str) -> Vec<String> {
    TOK.find_iter(s).map(|m| m.as_str().to_lowercase()).collect()
}

pub fn word_count(s: &str) -> usize {
    TOK.find_iter(s).count()
}

pub fn strip_tags(s: &str) -> String {
    TAG.replace_all(s, " ").into_owned()
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Pattern di una frase a confini di parola, senza distinzione di maiuscole; gli spazi della
/// frase accettano uno o più spazi nel testo.
fn phrase_regex(phrase: &str) -> Option<Regex> {
    let p = phrase.trim();
    if p.is_empty() {
        return None;
    }
    let parts: Vec<String> = p.split_whitespace().map(regex::escape).collect();
    Regex::new(&format!("(?i){}", parts.join(r"\s+"))).ok()
}

/// Trova le occorrenze di `re` che stanno a confini di parola (niente lettera o cifra subito
/// prima o subito dopo).
fn bounded_matches(text: &str, re: &Regex) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    while start <= text.len() {
        let Some(m) = re.find_at(text, start) else { break };
        let before_ok = text[..m.start()].chars().next_back().map_or(true, |c| !is_word_char(c));
        let after_ok = text[m.end()..].chars().next().map_or(true, |c| !is_word_char(c));
        if before_ok && after_ok && m.end() > m.start() {
            out.push((m.start(), m.end()));
            start = m.end();
        } else {
            // avanza di un carattere
            start = m.start() + text[m.start()..].chars().next().map_or(1, |c| c.len_utf8());
        }
    }
    out
}

/// Parole che indicano il mondo AI e codice: servono alle sostituzioni ambigue ("cloud" diventa
/// "Claude" solo se intorno si parla di prompt, skill, agenti).
const AI_CONTEXT: &[&str] = &[
    "prompt", "skill", "skills", "opus", "sonnet", "haiku", "fable", "subagente", "subagenti",
    "subagent", "agente", "agenti", "agent", "agents", "code", "md", "mcp", "api", "token",
    "plugin", "codex", "chatgpt", "anthropic", "hook", "wiki", "repository", "commit", "desktop",
    "chat", "modello", "contesto", "claude", "terminale", "loop", "goal", "memory",
];

fn has_ai_context(text: &str, start: usize, end: usize) -> bool {
    let lo = text[..start].char_indices().rev().nth(80).map_or(0, |(i, _)| i);
    let hi = text[end..].char_indices().nth(80).map_or(text.len(), |(i, _)| end + i);
    let window = format!("{} {}", &text[lo..start], &text[end..hi]);
    ltokens(&window).iter().any(|w| {
        AI_CONTEXT.contains(&w.as_str()) || w.ends_with(".md")
    })
}

fn is_sentence_start(text: &str, pos: usize) -> bool {
    match text[..pos].trim_end_matches([' ', '\t']).chars().next_back() {
        None => true,
        Some(c) => matches!(c, '.' | '!' | '?' | '\n' | '-' | '•'),
    }
}

fn upper_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_lowercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

struct Replacement {
    re: Regex,
    to: String,
    ambiguous: bool,
    entry_id: String,
    /// Sostituzione imparata dalle correzioni (`user_edits`): vale solo la forma sbagliata esatta,
    /// con le stesse maiuscole (o con l'iniziale maiuscola a inizio frase). Senza
    /// maiuscole "termineprova" → TERMINEPROVA cambiava anche gli "Termineprova" che l'utente tiene.
    exact: Option<String>,
}

/// La forma compare nel testo esattamente così, con le stesse maiuscole e a confini di parola
/// (gli spazi interni possono essere più d'uno).
pub fn contains_exact_form(text: &str, form: &str) -> bool {
    let Some(re) = phrase_regex(form) else { return false };
    let want = form.split_whitespace().collect::<Vec<_>>().join(" ");
    bounded_matches(text, &re)
        .into_iter()
        .any(|(s, e)| text[s..e].split_whitespace().collect::<Vec<_>>().join(" ") == want)
}

struct Snippet {
    re: Regex,
    trigger: String,
    expansion: String,
    entry_id: String,
}

/// Regole deterministiche ricavate dal dizionario.
pub struct Rules {
    replacements: Vec<Replacement>,
    snippets: Vec<Snippet>,
    /// Termini da proteggere: frasi del dizionario (o la loro forma sostituita), non snippet.
    pub protected_terms: Vec<String>,
    /// Forme minuscole dei termini, per non toccarne le maiuscole nei formati minuscoli.
    pub keep_case: HashSet<String>,
    /// Se falso, le sostituzioni ambigue non si applicano: decide il modello (variante di prova).
    pub apply_ambiguous: bool,
}

/// Risultato di una passata di sostituzioni.
pub struct Applied {
    pub text: String,
    pub count: u32,
    pub used_ids: Vec<String>,
}

impl Rules {
    pub fn from_entries(entries: &[DictionaryEntry]) -> Self {
        let mut replacements = Vec::new();
        let mut snippets = Vec::new();
        let mut protected = Vec::new();
        let mut keep = HashSet::new();
        for e in entries {
            let Some(re) = phrase_regex(&e.phrase) else { continue };
            match (&e.replacement, e.is_snippet) {
                (Some(exp), true) if !exp.trim().is_empty() => snippets.push(Snippet {
                    re,
                    trigger: e.phrase.trim().to_string(),
                    expansion: exp.clone(),
                    entry_id: e.id.clone(),
                }),
                (Some(to), false) if !to.trim().is_empty() => {
                    let learned = e.source == super::DictionarySource::UserEdits;
                    replacements.push(Replacement {
                        re,
                        to: to.trim().to_string(),
                        ambiguous: e.ambiguous,
                        entry_id: e.id.clone(),
                        exact: learned.then(|| e.phrase.split_whitespace().collect::<Vec<_>>().join(" ")),
                    });
                    protected.push(to.trim().to_string());
                }
                _ => {
                    let p = e.phrase.trim().to_string();
                    // voce di solo vocabolario con maiuscole ("AI STUDIO"): si impone la forma
                    // del dizionario, salvo parole comuni ("Grazie")
                    // solo forme "volute": iniziale maiuscola più un'altra maiuscola (CODEX,
                    // AI STUDIO, GitHub). Le voci imparate storte come "nOMEPROVA" o "fOTO3"
                    // no: imposte, la grafia storta finirebbe in ogni dettatura.
                    let first_upper = p.chars().next().is_some_and(|c| c.is_uppercase());
                    let more_upper = p.chars().skip(1).any(|c| c.is_uppercase());
                    let common = ltokens(&p).len() == 1 && super::dictionary::is_common_word(&p);
                    // le voci imparate solo vocabolario no: sono spesso sostituzioni spente
                    // perché l'utente tiene altre grafie ("DemoLab", "Demo Lab")
                    let learned = e.source == super::DictionarySource::UserEdits;
                    if !e.is_snippet && first_upper && more_upper && !common && !learned {
                        replacements.push(Replacement { re, to: p.clone(), ambiguous: false, entry_id: e.id.clone(), exact: None });
                    }
                    protected.push(p);
                }
            }
        }
        // le frasi più lunghe prima: "cloud code" prima di "cloud"
        replacements.sort_by_key(|r| std::cmp::Reverse(r.re.as_str().len()));
        snippets.sort_by_key(|s| std::cmp::Reverse(s.trigger.len()));
        protected.sort();
        protected.dedup_by(|a, b| a.eq_ignore_ascii_case(b));
        for t in &protected {
            for w in tokens(t) {
                if w.chars().any(|c| c.is_uppercase()) {
                    keep.insert(w.to_lowercase());
                }
            }
        }
        Rules {
            replacements,
            snippets,
            protected_terms: protected,
            keep_case: keep,
            apply_ambiguous: true,
        }
    }

    /// Sostituzioni (non snippet) a confini di parola, senza distinzione di maiuscole.
    pub fn apply_replacements(&self, text: &str) -> Applied {
        let mut out = text.to_string();
        let mut count = 0;
        let mut used = Vec::new();
        for r in &self.replacements {
            if r.ambiguous && !self.apply_ambiguous {
                continue;
            }
            let matches = bounded_matches(&out, &r.re);
            if matches.is_empty() {
                continue;
            }
            let mut buf = String::with_capacity(out.len());
            let mut last = 0;
            let mut n = 0;
            for (s, e) in matches {
                if r.ambiguous && !has_ai_context(&out, s, e) {
                    continue;
                }
                let found = &out[s..e];
                if found == r.to {
                    continue;
                }
                if let Some(form) = &r.exact {
                    let f = found.split_whitespace().collect::<Vec<_>>().join(" ");
                    if f != *form && !(f == upper_first(form) && is_sentence_start(&out, s)) {
                        continue;
                    }
                }
                buf.push_str(&out[last..s]);
                let starts_upper = found.chars().next().is_some_and(|c| c.is_uppercase());
                if starts_upper && is_sentence_start(&out, s) {
                    buf.push_str(&upper_first(&r.to));
                } else {
                    buf.push_str(&r.to);
                }
                last = e;
                n += 1;
            }
            if n > 0 {
                buf.push_str(&out[last..]);
                out = buf;
                count += n;
                used.push(r.entry_id.clone());
            }
        }
        Applied { text: out, count, used_ids: used }
    }

    /// Se l'intera dettatura è il trigger di uno snippet, restituisce l'espansione.
    pub fn whole_snippet(&self, text: &str) -> Option<(String, String)> {
        let norm = |s: &str| ltokens(s).join(" ");
        let t = norm(text);
        if t.is_empty() {
            return None;
        }
        self.snippets
            .iter()
            .find(|s| norm(&s.trigger) == t)
            .map(|s| (s.expansion.clone(), s.entry_id.clone()))
    }

    /// Espande gli snippet nel testo (dopo il modello o quando il modello non parte).
    pub fn apply_snippets(&self, text: &str) -> Applied {
        let mut out = text.to_string();
        let mut count = 0;
        let mut used = Vec::new();
        for s in &self.snippets {
            let m = bounded_matches(&out, &s.re);
            if m.is_empty() {
                continue;
            }
            for (a, b) in m.iter().rev() {
                out.replace_range(*a..*b, &s.expansion);
            }
            count += m.len() as u32;
            used.push(s.entry_id.clone());
        }
        Applied { text: out, count, used_ids: used }
    }

    /// Sostituisce i trigger degli snippet con segnaposto `[[S1]]`, così il modello non riscrive
    /// l'espansione. Restituisce il testo marcato e le espansioni in ordine.
    pub fn mark_snippets(&self, text: &str) -> (String, Vec<(String, String)>) {
        let mut out = text.to_string();
        let mut found = Vec::new();
        for s in &self.snippets {
            for (a, b) in bounded_matches(&out, &s.re).into_iter().rev() {
                found.push((s.expansion.clone(), s.entry_id.clone()));
                let marker = format!("[[S{}]]", found.len());
                out.replace_range(a..b, &marker);
            }
        }
        (out, found)
    }
}

/// Rimette le espansioni al posto dei segnaposto. `None` se il modello ne ha perso uno.
pub fn unmark_snippets(text: &str, snippets: &[(String, String)]) -> Option<String> {
    let mut out = text.to_string();
    for (i, (exp, _)) in snippets.iter().enumerate() {
        let marker = format!("[[S{}]]", i + 1);
        if !out.contains(&marker) {
            return None;
        }
        out = out.replace(&marker, exp);
    }
    Some(out)
}

/// "Premi invio" in fondo alla dettatura: lo toglie e segnala l'Invio.
static PRESS_ENTER: Lazy<Regex> = Lazy::new(|| {
    Regex::new(r"(?i)(?:^|[\s,.;:!?])(premi invio|premi enter|press enter|invia)[\s.!?]*$").unwrap()
});

pub fn strip_press_enter(text: &str) -> (String, bool) {
    let t = text.trim_end();
    if let Some(c) = PRESS_ENTER.captures(t) {
        let m = c.get(1).unwrap();
        let rest = t[..m.start()].trim_end_matches(|c: char| c.is_whitespace() || c == ',' || c == ';' || c == ':');
        return (rest.to_string(), true);
    }
    (text.to_string(), false)
}

/// Rete di sicurezza del backtrack, solo per i casi netti.
///
/// - "scratch that" (o "cancella quello che ho detto"): toglie la frase prima.
/// - "anzi no", "no aspetta", "anzi", "scusa", "no wait": se la parola subito dopo compare fra
///   le 8 parole prima nella stessa frase, toglie da lì fino all'indicatore ("di martedì anzi no
///   di mercoledì" diventa "di mercoledì"). Altrimenti non tocca nulla: "anzi no non serve" resta.
pub fn backtrack(text: &str) -> String {
    let mut out = text.to_string();
    // 1. scratch that: toglie la frase (o il pezzo di frase) prima
    static SCRATCH: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)[,;]?\s*\b(scratch that|cancella quello che ho detto|cancella l'ultima frase)\b[\s,.;:!?]*").unwrap()
    });
    while let Some(m) = SCRATCH.find(&out) {
        let before = &out[..m.start()];
        let cut = before
            .trim_end()
            .rfind(['.', '!', '?', '\n'])
            .map_or(0, |i| i + 1);
        let tail = out[m.end()..].to_string();
        let head = out[..cut].to_string();
        let joined = if head.trim().is_empty() {
            tail
        } else {
            format!("{} {}", head.trim_end(), tail)
        };
        out = joined.trim().to_string();
    }
    // 2. correzioni con frase parallela
    static CUE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\b(anzi no|no aspetta|no wait|anzi|scusa)\b[\s,.;:!?]*").unwrap()
    });
    let mut from = 0;
    loop {
        let Some(m) = CUE.find_at(&out, from) else { break };
        let before = &out[..m.start()];
        let after = &out[m.end()..];
        let first_after = ltokens(after).into_iter().next();
        let sentence_start = before.rfind(['.', '!', '?', '\n']).map_or(0, |i| i + 1);
        let spans = token_spans(&out[sentence_start..m.start()]);
        let window: Vec<(usize, usize)> = spans.iter().rev().take(8).rev().cloned().collect();
        let mut target = None;
        if let Some(f) = &first_after {
            for (s, e) in window.iter().rev() {
                if out[sentence_start + s..sentence_start + e].to_lowercase() == *f {
                    target = Some(sentence_start + s);
                    break;
                }
            }
        }
        match target {
            Some(t) => {
                let head = out[..t].to_string();
                let tail = out[m.end()..].to_string();
                out = format!("{}{}", head, tail);
                from = t;
            }
            None => from = m.end(),
        }
        if from >= out.len() {
            break;
        }
    }
    out
}

/// Toglie blocchi di thinking, etichette e virgolette di contorno dall'uscita del modello,
/// e trasforma gli eventuali elenchi HTML in righe con trattino.
/// Solo via i blocchi `<think>` (uscite che possono contenere etichette e tag, come i Transforms).
pub fn sanitize_think(s: &str) -> String {
    static THINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<think>.*?(</think>|$)").unwrap());
    THINK.replace_all(s, "").into_owned()
}

pub fn sanitize_model_output(s: &str) -> String {
    static THINK: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?s)<think>.*?(</think>|$)").unwrap());
    static LABEL: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)^\s*(testo pulito|testo riscritto|trascrizione pulita|output)\s*:\s*").unwrap()
    });
    let mut t = THINK.replace_all(s, "").into_owned();
    // il modello a volte ripete la richiesta: si tiene solo ciò che segue
    // l'ultimo "Testo pulito:"; senza, l'uscita si scarta (stringa vuota, il chiamante usa il grezzo)
    if t.contains("Trascrizione:") || t.trim_start().starts_with("Stile:") || t.trim_start().starts_with("Formato:") {
        t = match t.rfind("Testo pulito:") {
            Some(i) => t[i + "Testo pulito:".len()..].to_string(),
            None => String::new(),
        };
    }
    t = LABEL.replace(&t, "").into_owned();
    t = html_lists_to_dashes(&t);
    let t = t.trim();
    let pairs = [('"', '"'), ('«', '»'), ('“', '”'), ('\'', '\'')];
    for (a, b) in pairs {
        if t.len() > 2 && t.starts_with(a) && t.ends_with(b) {
            let inner = &t[a.len_utf8()..t.len() - b.len_utf8()];
            if !inner.contains(a) && !inner.contains(b) {
                return inner.trim().to_string();
            }
        }
    }
    t.to_string()
}

fn html_lists_to_dashes(s: &str) -> String {
    if !s.contains("<li") {
        return s.to_string();
    }
    static LI: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?is)\s*<li[^>]*>\s*(.*?)\s*</li>\s*").unwrap());
    static LIST: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)\s*</?(ul|ol)[^>]*>\s*").unwrap());
    let t = LI.replace_all(s, "\n- $1\n");
    let t = LIST.replace_all(&t, "\n");
    static GAP: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n\s*\n(- )").unwrap());
    GAP.replace_all(&t, "\n$1").into_owned()
}

/// Toglie "appunto" usato come intercalare (il modello locale lo lascia spesso). Resta "un appunto", "l'appunto", "gli appunti".
pub fn drop_fillers(text: &str) -> String {
    static APPUNTO: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?i)appunto").unwrap());
    let mut out = text.to_string();
    for (s, e) in bounded_matches(text, &APPUNTO).into_iter().rev() {
        let before = &out[..s];
        if before.ends_with('\'') || before.ends_with('’') {
            continue;
        }
        let prev = ltokens(before).pop().unwrap_or_default();
        if matches!(prev.as_str(), "un" | "questo" | "quello" | "quest" | "quell" | "mio" | "tuo" | "suo" | "primo" | "ultimo") {
            continue;
        }
        // una virgola sola resta: ", appunto," diventa ","
        let after = &out[e..];
        let trail = after.len() - after.trim_start_matches([' ', ',']).len();
        let lead = before.len() - before.trim_end_matches([' ', ',']).len();
        let had_comma = out[s - lead..s].contains(',') || after[..trail].contains(',');
        let at_end = after[trail..].is_empty() || after[trail..].starts_with(['.', '?', '!', ':', ';', '\n']);
        let starts = before.trim_end_matches([' ', ',']).is_empty() || before.trim_end_matches([' ', ',']).ends_with(['.', '?', '!', '\n']);
        let joint = if at_end || starts { "" } else if had_comma { ", " } else { " " };
        let joint = if starts && !before.trim().is_empty() { " " } else { joint };
        out.replace_range(s - lead..e + trail, joint);
    }
    // maiuscola se "appunto" apriva la frase
    fix_sentence_caps(&upper_first(&out))
}

/// Da quante parole una dettatura si pulisce in due metà in parallelo (due slot di
/// llama-server): sotto, una richiesta sola sta già nel budget.
pub const PARALLEL_MIN_WORDS: usize = 50;

/// Taglia il testo in due metà vicino al centro (fra il 30 e il 70% delle parole): prima a fine
/// frase, poi dopo una virgola, poi fra due parole qualsiasi. Restituisce (prima, seconda,
/// taglio a fine frase). Non taglia dentro un segnaposto `[[S1]]`.
pub fn split_halves(text: &str) -> Option<(String, String, bool)> {
    let spans = token_spans(text);
    let n = spans.len();
    if n < PARALLEL_MIN_WORDS {
        return None;
    }
    let (lo, hi) = (n * 3 / 10, n * 7 / 10);
    let mut best: Option<(u8, usize, usize)> = None; // (priorità, distanza dal centro, indice)
    for i in lo.max(1)..=hi.min(n - 1) {
        let gap = &text[spans[i - 1].1..spans[i].0];
        if gap.contains('[') || gap.contains(']') || gap.contains('\'') || gap.contains('’') || !gap.contains(char::is_whitespace) {
            continue;
        }
        // niente taglio dentro una ripetizione o un'autocorrezione: le metà pulite da sole
        // la sbagliano ("dei variazioni delle variazioni")
        if !gap.contains(['.', '?', '!']) && straddles_repair(text, &spans, i) {
            continue;
        }
        let prio = if gap.contains(['.', '?', '!']) {
            0
        } else if gap.contains([',', ';', ':']) {
            1
        } else {
            2
        };
        let dist = i.abs_diff(n / 2);
        if best.is_none_or(|(p, d, _)| (prio, dist) < (p, d)) {
            best = Some((prio, dist, i));
        }
    }
    let (prio, _, i) = best?;
    let cut = spans[i].0;
    Some((text[..cut].trim_end().to_string(), text[cut..].trim_start().to_string(), prio == 0))
}

/// Il taglio prima del token `i` cade in mezzo a una ripetizione o a un'autocorrezione? Vero se
/// una parola di almeno 4 lettere compare sia nei 6 token prima sia nei 6 dopo, o se c'è un
/// indicatore di correzione ("scusa", "anzi", "cioè", "volevo dire", "aspetta") vicino.
fn straddles_repair(text: &str, spans: &[(usize, usize)], i: usize) -> bool {
    let tok = |k: usize| text[spans[k].0..spans[k].1].to_lowercase();
    let before: Vec<String> = (i.saturating_sub(6)..i).map(tok).collect();
    let after: Vec<String> = (i..(i + 6).min(spans.len())).map(tok).collect();
    let cue = |w: &String| matches!(w.as_str(), "scusa" | "anzi" | "cioè" | "aspetta" | "dire");
    if before.iter().chain(after.iter()).any(cue) {
        return true;
    }
    before.iter().any(|w| w.chars().count() >= 4 && after.contains(w))
}

/// Riunisce le due metà pulite. Se il taglio non era a fine frase, la prima metà perde il
/// punto finale aggiunto dal modello e la seconda riparte in minuscolo se nel grezzo la sua
/// prima parola era minuscola (un nome proprio resta maiuscolo).
pub fn join_halves(a: &str, b: &str, at_sentence_end: bool, raw_b: &str) -> String {
    let a = a.trim_end();
    let b = b.trim_start();
    if at_sentence_end {
        return format!("{a} {b}");
    }
    let a = a.strip_suffix('.').filter(|x| !x.ends_with('.')).unwrap_or(a);
    let raw_lower = raw_b.trim_start().chars().next().is_some_and(|c| c.is_lowercase());
    let b = if raw_lower { lower_first(b) } else { b.to_string() };
    format!("{a} {b}")
}

/// L'uscita contiene pezzi del prompt? Capita che il modello ricopi righe del
/// messaggio utente ("Dizionario:", "- by the way → by the way", "Stile: ...", "App: ...") e
/// sarebbero finite nel campo dell'utente. Controlla: tag del prompt, la freccia delle righe
/// del dizionario, righe che iniziano con un'etichetta del prompt, righe di almeno 15
/// caratteri del messaggio utente (fuori dal testo da pulire) o del sistema ricopiate, due o
/// più righe uguali a voci del dizionario.
pub fn prompt_leak(output: &str, user_msg: &str, system: &str) -> bool {
    const TAGS: &[&str] = &["<contesto>", "</contesto>", "<trascrizione>", "</trascrizione>", "<testo>", "</testo>"];
    const LABELS: &[&str] = &[
        "dizionario:", "dizionario personale", "stile:", "formato:", "app:", "elenchi:", "nomi sullo schermo",
        "trascrizione:", "testo pulito:", "esempi:",
    ];
    let low = output.to_lowercase();
    if TAGS.iter().any(|t| low.contains(t)) || output.contains('→') {
        return true;
    }
    for line in output.lines() {
        let l = line.trim().trim_start_matches(['-', '•', '*']).trim().to_lowercase();
        if LABELS.iter().any(|m| l.starts_with(m)) {
            return true;
        }
    }
    // etichette con la maiuscola anche a metà riga ("... chiudi. Stile: naturale")
    const MID: &[&str] = &["Dizionario:", "Stile: ", "Formato: ", "Elenchi: ", "App: ", "Trascrizione:", "Testo pulito:"];
    if MID.iter().any(|m| output.contains(m)) {
        return true;
    }
    // righe del messaggio utente fuori dal blocco del testo da pulire
    let mut prompt_lines: Vec<&str> = Vec::new();
    let mut inside = false;
    for l in user_msg.lines() {
        let t = l.trim();
        if t == "<trascrizione>" {
            inside = true;
            continue;
        }
        if t == "</trascrizione>" {
            inside = false;
            continue;
        }
        if !inside {
            prompt_lines.push(t);
        }
    }
    let mut dict_lines = HashSet::new();
    for l in system.lines() {
        let t = l.trim();
        // gli esempi sono frasi di dettatura: una frase uguale non è un'eco
        if t == "Esempi:" {
            break;
        }
        if t.starts_with("- ") {
            dict_lines.insert(t.to_lowercase());
        } else {
            prompt_lines.push(t);
        }
    }
    // righe e frasi (almeno 15 caratteri) del prompt ricopiate, anche a pezzi
    for p in &prompt_lines {
        let p = p.to_lowercase();
        let pieces = std::iter::once(p.as_str()).chain(p.split(". ").map(str::trim));
        for piece in pieces {
            let piece = piece.trim_end_matches('.');
            if piece.chars().count() >= 15 && low.contains(piece) {
                return true;
            }
        }
    }
    // anche solo l'inizio (25 caratteri) di una riga lunga del messaggio utente
    let user_starts = user_msg.lines().map(str::trim).filter(|t| !t.starts_with('<') && t.chars().count() >= 25);
    for t in user_starts {
        let head: String = t.chars().take(25).collect();
        if low.contains(&head.to_lowercase()) && !user_msg_text(user_msg).to_lowercase().contains(&head.to_lowercase()) {
            return true;
        }
    }
    output.lines().filter(|l| dict_lines.contains(&l.trim().to_lowercase())).count() >= 2
}

/// Il testo da pulire dentro il messaggio utente (fra `<trascrizione>` e `</trascrizione>`).
fn user_msg_text(user_msg: &str) -> &str {
    let start = user_msg.find("<trascrizione>").map_or(0, |i| i + "<trascrizione>".len());
    let end = user_msg[start..].find("</trascrizione>").map_or(user_msg.len(), |i| start + i);
    &user_msg[start..end]
}

/// Il Polish ha risposto o eseguito il testo invece di riscriverlo? Capita che il modello locale,
/// con una richiesta da riscrivere, risponda "Ecco il messaggio..." e scriva lui il messaggio.
/// Segnali: attacco da risposta ("Ecco", "Certo", "Di seguito"...), segnaposto fra
/// quadre, oggetto o firma di mail che il testo non aveva; con "Togli ripetizioni e giri di parole" attivo,
/// un'uscita più lunga del testo.
pub fn polish_answer(input: &str, output: &str, concise: bool) -> bool {
    let out = output.trim();
    if out.is_empty() {
        return true;
    }
    let low_out = out.to_lowercase();
    let low_in = input.to_lowercase();
    const OPENERS: &[&str] = &["ecco", "certo", "ok, ecco", "ok ecco", "di seguito", "sure", "here is", "here's", "ecco a te", "perfetto, ecco"];
    let first: String = low_out.split(['\n', ':']).next().unwrap_or("").trim().to_string();
    if OPENERS.iter().any(|o| first.starts_with(o)) && !OPENERS.iter().any(|o| low_in.trim_start().starts_with(o)) {
        return true;
    }
    static PLACEHOLDER: Lazy<Regex> = Lazy::new(|| Regex::new(r"\[[^\]]{2,40}\]").unwrap());
    if PLACEHOLDER.is_match(out) && !PLACEHOLDER.is_match(input) && !out.contains("[[S") {
        return true;
    }
    for mark in ["oggetto:", "cordiali saluti", "gentile cliente", "distinti saluti", "un saluto,"] {
        if low_out.contains(mark) && !low_in.contains(mark) {
            return true;
        }
    }
    // contenuto perso: meno del 50% delle parole (i Polish buoni ne tengono di più) su un testo
    // di almeno 15 (una richiesta lunga ridotta a una riga perde i dettagli)
    let (wi, wo) = (word_count(input), word_count(out));
    if wi >= 15 && (wo as f64) < 0.5 * wi as f64 {
        return true;
    }
    // traduzione o traslitterazione: testo in un altro alfabeto, uscita in lettere latine
    let latin = |s: &str| {
        let letters: Vec<char> = s.chars().filter(|c| c.is_alphabetic()).collect();
        if letters.is_empty() {
            return 1.0;
        }
        letters.iter().filter(|&&c| c.is_ascii_alphabetic() || ('\u{00C0}'..='\u{024F}').contains(&c)).count() as f64
            / letters.len() as f64
    };
    if latin(input) < 0.5 && latin(out) > 0.8 {
        return true;
    }
    concise && wo > wi
}

const NUMBER_WORDS: &[(&str, &str)] = &[
    ("uno", "1"), ("una", "1"), ("due", "2"), ("tre", "3"), ("quattro", "4"), ("cinque", "5"), ("sei", "6"),
    ("sette", "7"), ("otto", "8"), ("nove", "9"), ("dieci", "10"), ("undici", "11"), ("dodici", "12"),
    ("quindici", "15"), ("venti", "20"), ("trenta", "30"), ("quaranta", "40"), ("cinquanta", "50"),
    ("cento", "100"), ("mille", "1000"),
];

/// Copertura del Polish: cosa del testo manca nell'uscita. Controlla numeri (anche detti a
/// parole, accettati in cifre o in lettere), parole approssimative ("quasi", "circa"), termini
/// del dizionario, parole con la maiuscola dentro la frase o inglesi (finale consonantica, fuori
/// dalle parole comuni), pronomi di prima persona singolare. Casi tipici: "mi costa venti
/// minuti" diventato "impiega circa 20 minuti", "quasi tre ore" diventato "3 ore", "AI STUDIO" perso.
pub fn polish_missing(input: &str, output: &str, dict_terms: &[String]) -> Vec<String> {
    let mut missing = Vec::new();
    let in_toks = ltokens(input);
    let out_toks = ltokens(output);
    let out_set: HashSet<&str> = out_toks.iter().map(|s| s.as_str()).collect();
    let has = |w: &str| out_set.contains(w) || out_set.contains(w.replace('.', "").as_str());
    // numeri in cifre e a parole
    for w in &in_toks {
        if w.chars().all(|c| c.is_ascii_digit() || c == '.' || c == ',') && w.chars().any(|c| c.is_ascii_digit()) {
            let plain = w.replace(['.', ','], "");
            if !out_toks.iter().any(|o| o.replace(['.', ','], "") == plain) {
                missing.push(w.clone());
            }
        } else if let Some((word, digit)) = NUMBER_WORDS.iter().find(|(n, _)| n == w) {
            // "una" e "uno" sono spesso articoli: si controllano solo se seguiti da un numero d'unità
            if matches!(*word, "uno" | "una") {
                continue;
            }
            if !has(word) && !has(digit) {
                missing.push(w.clone());
            }
        }
    }
    // parole approssimative
    // solo "quasi" e "circa": "magari" e "forse" sono spesso intercalari che "conciso" toglie
    for a in ["quasi", "circa"] {
        if in_toks.iter().any(|w| w == a) && !has(a) {
            missing.push(a.to_string());
        }
    }
    // termini del dizionario presenti nel testo
    for t in dict_terms {
        let key = ltokens(t);
        if key.is_empty() {
            continue;
        }
        if !count_seq(&in_toks, &key).is_empty() && count_seq(&out_toks, &key).is_empty() {
            missing.push(t.clone());
        }
    }
    // maiuscole dentro la frase e parole inglesi
    let spans = token_spans(input);
    for (i, (s, e)) in spans.iter().enumerate() {
        let w = &input[*s..*e];
        let lw = w.to_lowercase();
        let mid_sentence = i > 0 && !is_sentence_start(input, *s);
        let capital = mid_sentence && w.chars().next().is_some_and(|c| c.is_uppercase());
        let last = lw.chars().next_back().unwrap_or('a');
        let english = lw.chars().count() >= 4
            && lw.chars().all(|c| c.is_ascii_alphabetic())
            && !"aeiouàèéìòù".contains(last)
            && !super::dictionary::is_common_word(&lw);
        if (capital || english) && !has(&lw) {
            missing.push(w.to_string());
        }
    }
    // prima persona singolare
    let first = ["io", "mi", "me", "mio", "mia", "miei", "mie"];
    if in_toks.iter().any(|w| first.contains(&w.as_str())) && !out_toks.iter().any(|w| first.contains(&w.as_str())) {
        missing.push("prima persona".to_string());
    }
    // registro del Lei: le formule del testo restano, e non compare il tu
    let low_in = format!(" {} ", in_toks.join(" "));
    let low_out = format!(" {} ", out_toks.join(" "));
    let lei: Vec<&str> = LEI_MARKERS.iter().copied().filter(|m| low_in.contains(&format!(" {m} "))).collect();
    if !lei.is_empty() {
        for m in &lei {
            if !low_out.contains(&format!(" {m} ")) {
                missing.push(format!("registro Lei: {m}"));
            }
        }
        for t in TU_MARKERS {
            if low_out.contains(&format!(" {t} ")) && !low_in.contains(&format!(" {t} ")) {
                missing.push(format!("registro tu: {t}"));
            }
        }
    }
    // parole di contenuto (verbi, aggettivi, avverbi che portano informazione): se ne manca
    // più di una (o più del 10% sui testi lunghi), il Polish ha riassunto o sostituito verbi
    let gone = content_missing(&in_toks, &out_toks);
    let n_content = in_toks.iter().filter(|w| is_content_word(w)).count();
    if gone.len() > 1usize.max(n_content / 10) {
        missing.push(format!("parole: {}", gone.join(", ")));
    }
    missing.sort();
    missing.dedup();
    missing
}

/// Formule del Lei che, se il testo le usa, devono restare (per esempio "le confermo"
/// diventato "confermo", "mi scriva pure" diventato "mi scrivi pure").
const LEI_MARKERS: &[&str] = &[
    "le confermo", "le scrivo", "le chiedo", "le invio", "le mando", "le ricordo", "le comunico", "la ringrazio",
    "la informo", "mi scriva", "mi faccia", "mi dica", "mi mandi", "mi chiami", "sua disposizione",
];
const TU_MARKERS: &[&str] = &["ti", "tuo", "tua", "tuoi", "scrivi", "dimmi", "fammi", "mandami", "chiamami"];

/// Parole funzionali e intercalari: non contano nella copertura delle parole di contenuto.
const FUNCTION_WORDS: &[&str] = &[
    "allora", "quindi", "dunque", "appunto", "comunque", "praticamente", "cioè", "tipo", "anche", "però", "perché",
    "poiché", "mentre", "quando", "dove", "come", "cosa", "cose", "questo", "questa", "questi", "queste", "quello",
    "quella", "quelli", "quelle", "qui", "qua", "così", "molto", "tutto", "tutti", "tutta", "tutte", "della", "delle",
    "dello", "degli", "dalla", "dalle", "dallo", "dagli", "nella", "nelle", "nello", "negli", "sulla", "sulle", "sullo",
    "sugli", "alla", "alle", "allo", "agli", "col", "coi", "sono", "essere", "stato", "stata", "hanno", "abbiamo",
    "avere", "fare", "fatto", "loro", "nostro", "nostra", "vostro", "vostra", "suoi", "sue", "anzi",
    "sarebbe", "magari", "forse", "proprio", "solo", "ecco", "okay", "insomma", "diciamo", "senza",
    "verso",
    // connettivi di elenco e modi di dire che una riscrittura toglie senza perdere informazione
    // "volevo dire", "più o meno" (diventa "circa"), "infine" degli elenchi
    "volevo", "dire", "dentro", "infine", "meno", "tanto", "bisogna",
];
// "primo", "ancora", "ogni volta", "oggi", "sempre", "prima", "dopo" portano informazione: restano
// fuori da questa lista e contano nella copertura (altrimenti "il primo acconto" si perde).

pub fn is_content_word(w: &str) -> bool {
    // i numeri a parole li controlla già la parte sui numeri (accettati anche in cifre)
    w.chars().count() >= 4
        && w.chars().all(|c| c.is_alphabetic())
        && !FUNCTION_WORDS.contains(&w)
        && !NUMBER_WORDS.iter().any(|(n, _)| *n == w)
}

/// Parole di contenuto del testo assenti nell'uscita. Due parole si considerano la stessa se
/// hanno le stesse prime 5 lettere ("ricerca" e "ricerche", "servizi" e "servizio").
pub fn content_missing(in_toks: &[String], out_toks: &[String]) -> Vec<String> {
    let stem = |w: &str| w.chars().take(5).collect::<String>();
    let out_stems: HashSet<String> = out_toks.iter().map(|w| stem(w)).collect();
    let mut gone: Vec<String> = in_toks
        .iter()
        .filter(|w| is_content_word(w))
        .filter(|w| !out_stems.contains(&stem(w)))
        .cloned()
        .collect();
    gone.sort();
    gone.dedup();
    gone
}

/// Ritocchi deterministici dell'uscita del Polish: niente apici inversi di markdown
/// ("`cloud.md`" diventa "cloud.md"), il testo resta testo semplice.
pub fn polish_plain(text: &str) -> String {
    text.replace('`', "")
}

/// Il modello ha risposto invece di pulire? Uscita molto più corta o più lunga dell'ingresso.
pub fn looks_like_answer(input: &str, output: &str) -> bool {
    let i = word_count(input) as f64;
    let o = word_count(output) as f64;
    if o == 0.0 {
        return true;
    }
    i >= 8.0 && (o < 0.4 * i || o > 2.0 * i + 10.0)
}

/// Il grezzo sembra già pulito: una sola frase, maiuscola iniziale, punto finale.
pub fn already_clean(text: &str) -> bool {
    let t = text.trim();
    let starts_upper = t.chars().next().is_some_and(|c| c.is_uppercase());
    let ends = t.ends_with(['.', '?', '!']);
    let inner = t.char_indices().next_back().map_or(t, |(i, _)| &t[..i]);
    let one_sentence = !inner.contains(". ") && !inner.contains("? ") && !inner.contains("! ");
    let has_filler = ltokens(t).iter().any(|w| matches!(w.as_str(), "ehm" | "uhm" | "mmh" | "appunto"));
    let toks = ltokens(t);
    let repeated = toks.windows(2).any(|w| w[0] == w[1]);
    starts_upper && ends && one_sentence && !has_filler && !repeated
}

/// Numero massimo di parole per saltare il modello su un grezzo già pulito (un testo breve che
/// arriva già pulito di solito resta con le stesse parole).
pub const ALREADY_CLEAN_MAX_WORDS: usize = 8;

/// Decide se saltare il modello.
pub fn should_skip(text: &str, skip_under_words: u32) -> bool {
    let n = word_count(text);
    n < skip_under_words as usize || (n <= ALREADY_CLEAN_MAX_WORDS && already_clean(text))
}

/// Punteggiatura di base per il formato "No AI formatting" e per il salto: maiuscola iniziale,
/// punto finale se `final_period` e il testo finisce con una lettera o una cifra.
pub fn basic_punctuation(text: &str, final_period: bool) -> String {
    let t = text.trim();
    let mut out = upper_first(t);
    if final_period && out.chars().next_back().is_some_and(|c| c.is_alphanumeric()) {
        out.push('.');
    }
    out
}

/// Allinea due sequenze di token (minuscole) e restituisce le coppie (i, j) uguali della
/// sottosequenza comune più lunga.
fn lcs_pairs(a: &[String], b: &[String]) -> Vec<(usize, usize)> {
    let (n, m) = (a.len(), b.len());
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[idx(i, j)] = if a[i] == b[j] {
                dp[idx(i + 1, j + 1)] + 1
            } else {
                dp[idx(i + 1, j)].max(dp[idx(i, j + 1)])
            };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    while i < n && j < m {
        if a[i] == b[j] {
            out.push((i, j));
            i += 1;
            j += 1;
        } else if dp[idx(i + 1, j)] >= dp[idx(i, j + 1)] {
            i += 1;
        } else {
            j += 1;
        }
    }
    out
}

fn count_seq(toks: &[String], key: &[String]) -> Vec<usize> {
    let mut out = Vec::new();
    let k = key.len();
    if k == 0 || toks.len() < k {
        return out;
    }
    let mut i = 0;
    while i + k <= toks.len() {
        if toks[i..i + k] == *key {
            out.push(i);
            i += k;
        } else {
            i += 1;
        }
    }
    out
}

/// Scrive i termini protetti con la loro grafia dentro un pezzo di grezzo, a confini di parola e
/// senza badare alle maiuscole. Con `from_dictionary` le grafie storte del dizionario ("nOMEPROVA":
/// minuscola e poi maiuscole) non si impongono; la lista `grafie_correzioni.txt` invece è già
/// scelta a mano e vale tutta ("file-provaV2").
fn recase_terms(piece: &str, terms: &[String], from_dictionary: bool) -> String {
    let mut out = piece.to_string();
    for t in terms {
        let odd = from_dictionary
            && t.chars().next().is_some_and(|c| c.is_lowercase())
            && t.chars().skip(1).any(|c| c.is_uppercase());
        if odd || !t.chars().any(|c| c.is_uppercase()) {
            continue;
        }
        let Some(re) = phrase_regex(t) else { continue };
        for (a, b) in bounded_matches(&out.clone(), &re).into_iter().rev() {
            out.replace_range(a..b, t);
        }
    }
    out
}

/// Controllo dei termini: ogni termine presente nel grezzo deve esserci anche nel pulito.
/// Se il modello lo ha tolto o cambiato, rimette il pezzo di grezzo fra le due parole uguali
/// più vicine. Restituisce il testo e il numero di pezzi ripristinati.
pub fn restore_terms(raw: &str, cleaned: &str, terms: &[String], spellings: &[String]) -> (String, u32) {
    let r_spans = token_spans(raw);
    let r_toks: Vec<String> = r_spans.iter().map(|(s, e)| raw[*s..*e].to_lowercase()).collect();
    let c_spans = token_spans(cleaned);
    let c_toks: Vec<String> = c_spans.iter().map(|(s, e)| cleaned[*s..*e].to_lowercase()).collect();
    let mut edits: Vec<(usize, usize, String)> = Vec::new(); // byte del pulito, testo nuovo
    let mut pairs: Option<Vec<(usize, usize)>> = None;
    for term in terms {
        let key = ltokens(term);
        if key.is_empty() {
            continue;
        }
        let in_raw = count_seq(&r_toks, &key);
        if in_raw.is_empty() {
            continue;
        }
        let in_clean = count_seq(&c_toks, &key);
        if in_clean.len() >= in_raw.len() {
            continue;
        }
        let pairs = pairs.get_or_insert_with(|| lcs_pairs(&r_toks, &c_toks));
        for i in in_raw {
            let k = key.len();
            // presente se tutti i token del termine sono allineati a token consecutivi del pulito
            let mapped: Vec<usize> = (i..i + k)
                .filter_map(|ri| pairs.iter().find(|(a, _)| *a == ri).map(|(_, b)| *b))
                .collect();
            if mapped.len() == k && mapped.windows(2).all(|w| w[1] == w[0] + 1) {
                continue;
            }
            let before = pairs.iter().filter(|(a, _)| *a < i).last().cloned();
            let after = pairs.iter().find(|(a, _)| *a >= i + k).cloned();
            let raw_from = before.map_or(0, |(a, _)| r_spans[a].1);
            let raw_to = after.map_or(raw.len(), |(a, _)| r_spans[a].0);
            let c_from = before.map_or(0, |(_, b)| c_spans[b].1);
            let c_to = after.map_or(cleaned.len(), |(_, b)| c_spans[b].0);
            if c_from > c_to {
                continue;
            }
            let piece = raw[raw_from..raw_to].trim();
            let piece = piece.trim_matches(|c: char| c == ',' || c == ';');
            // il pezzo rimesso prende solo le grafie distintive della lista ("nomeprova" diventa
            // "Nomeprova", "chatgpt" diventa "ChatGPT"); le parole comuni restano come nel grezzo
            let piece = recase_terms(piece, spellings, false);
            let text = if c_from == 0 {
                format!("{} ", piece)
            } else if c_to == cleaned.len() {
                format!(" {}", piece)
            } else {
                format!(" {} ", piece)
            };
            if !edits.iter().any(|(a, b, _)| c_from < *b && *a < c_to || (*a == c_from && *b == c_to)) {
                edits.push((c_from, c_to, text));
            }
        }
    }
    if edits.is_empty() {
        return (cleaned.to_string(), 0);
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut out = cleaned.to_string();
    let n = edits.len() as u32;
    for (a, b, t) in edits {
        // tiene la punteggiatura del pulito ai bordi del pezzo sostituito
        let seg = &out[a..b];
        let lead: String = seg.chars().take_while(|c| c.is_ascii_punctuation() && *c != '-').collect();
        let trail: String = {
            let mut v: Vec<char> = seg.chars().rev().take_while(|c| matches!(c, '.' | ',' | '?' | '!' | ':' | ';')).collect();
            v.reverse();
            v.into_iter().collect()
        };
        let mut rep = String::new();
        rep.push_str(&lead);
        rep.push_str(&t);
        if !trail.is_empty() && !t.trim_end().ends_with(['.', ',', '?', '!', ':', ';']) {
            rep = format!("{}{} ", rep.trim_end(), trail);
        }
        out.replace_range(a..b, &rep);
    }
    (fix_spacing(&out), n)
}

/// Distanza massima fra i caratteri (`normalized_distance`, senza spazi e simboli detti) perché
/// un termine del dizionario scritto dal modello "suoni come" una parola o un gruppo di parole
/// del grezzo. Soglia scelta dopo le prove. Esempi: le storpiature stanno
/// sotto ("tubbi" per TUBI 0,20; "cloud" per Claude 0,33), i cambi
/// sbagliati sopra ("clod" per Claude 0,50, "temi" per TUBI 0,50).
pub const TERM_NEAR_MAX: f64 = 0.40;

/// Simboli detti ("slash help", "punto env", "file punto py"): il modello li
/// scrive come simboli, quindi non contano nel confronto e non si rimettono.
const SPOKEN_SYMBOLS: &[&str] = &[
    "slash", "punto", "trattino", "chiocciola", "underscore", "dot", "barra", "virgola", "hashtag", "cancelletto", "tilde",
];

/// Forma compatta per il confronto dei suoni: minuscole, solo lettere e cifre, senza simboli detti.
fn compact_words<S: AsRef<str>>(words: &[S]) -> String {
    words
        .iter()
        .map(|w| w.as_ref().to_lowercase())
        .filter(|w| !SPOKEN_SYMBOLS.contains(&w.as_str()))
        .collect::<String>()
        .chars()
        .filter(|c| c.is_alphanumeric())
        .collect()
}

/// Il termine somiglia a una parola o a un gruppo di parole (fino a quelle del termine più 2)
/// della finestra del grezzo? Vale anche dentro una parola incollata ("filecloud.md").
fn sounds_like(window: &[String], term: &str) -> bool {
    let tc = compact_words(&ltokens(term));
    if tc.is_empty() {
        return true;
    }
    let k = ltokens(term).len();
    let near = |c: &str| !c.is_empty() && super::dictionary::normalized_distance(c, &tc) <= TERM_NEAR_MAX;
    for a in 0..window.len() {
        for e in a + 1..=(a + k + 2).min(window.len()) {
            if near(&compact_words(&window[a..e])) {
                return true;
            }
        }
        let w: Vec<char> = compact_words(&window[a..a + 1]).chars().collect();
        let l = tc.chars().count();
        for len in [l.saturating_sub(1), l, l + 1] {
            if len >= 3 && len < w.len() {
                let head: String = w[..len].iter().collect();
                let tail: String = w[w.len() - len..].iter().collect();
                if near(&head) || near(&tail) {
                    return true;
                }
            }
        }
    }
    false
}

/// Regola 18 ristretta, lato deterministico: un termine del dizionario che il modello ha scritto
/// al posto di parole del grezzo che non gli somigliano (né per suono né per lettere) torna alla
/// forma del grezzo; se il grezzo lì non aveva nulla, il termine si toglie. I termini già presenti
/// nel grezzo (anche grazie alle sostituzioni del dizionario) non si toccano. Restituisce il testo
/// e il numero di termini tolti.
pub fn revert_far_terms(raw: &str, out: &str, terms: &[String]) -> (String, u32) {
    let r_spans = token_spans(raw);
    let rt: Vec<String> = r_spans.iter().map(|(s, e)| raw[*s..*e].to_lowercase()).collect();
    let c_spans = token_spans(out);
    let ct: Vec<String> = c_spans.iter().map(|(s, e)| out[*s..*e].to_lowercase()).collect();
    let mut pairs: Option<Vec<(usize, usize)>> = None;
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for term in terms {
        let key = ltokens(term);
        if key.is_empty() {
            continue;
        }
        let found = count_seq(&ct, &key);
        if found.is_empty() {
            continue;
        }
        let pairs = pairs.get_or_insert_with(|| lcs_pairs(&rt, &ct));
        let k = key.len();
        for j in found {
            if (j..j + k).all(|cj| pairs.iter().any(|&(_, b)| b == cj)) {
                continue; // c'era già nel grezzo
            }
            let before = pairs.iter().filter(|&&(_, b)| b < j).last().cloned();
            let after = pairs.iter().find(|&&(_, b)| b >= j + k).cloned();
            let ri1 = before.map_or(0, |(a, _)| a + 1);
            let ri2 = after.map_or(rt.len(), |(a, _)| a);
            let window = &rt[ri1.saturating_sub(1)..(ri2 + 1).min(rt.len()).max(ri1.saturating_sub(1))];
            if sounds_like(window, term) {
                continue;
            }
            let c_from = before.map_or(0, |(_, b)| c_spans[b].1);
            let c_to = after.map_or(out.len(), |(_, b)| c_spans[b].0);
            let raw_from = before.map_or(0, |(a, _)| r_spans[a].1);
            let raw_to = after.map_or(raw.len(), |(a, _)| r_spans[a].0);
            if c_from > c_to || raw_from > raw_to {
                continue;
            }
            let piece = raw[raw_from..raw_to].trim().trim_matches(|c: char| c == ',' || c == ';');
            let text = if piece.is_empty() {
                " ".to_string()
            } else if c_from == 0 {
                format!("{piece} ")
            } else if c_to == out.len() {
                format!(" {piece}")
            } else {
                format!(" {piece} ")
            };
            if !edits.iter().any(|(a, b, _)| c_from < *b && *a < c_to || (*a == c_from && *b == c_to)) {
                edits.push((c_from, c_to, text));
            }
        }
    }
    if edits.is_empty() {
        return (out.to_string(), 0);
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let n = edits.len() as u32;
    let mut res = out.to_string();
    for (a, b, t) in edits {
        // la punteggiatura del pulito in fondo al tratto resta ("i TUBI," diventa "i temi,")
        let seg = &res[a..b];
        let trail: String = {
            let mut v: Vec<char> = seg.trim_end().chars().rev().take_while(|c| matches!(c, '.' | ',' | '?' | '!' | ':' | ';')).collect();
            v.reverse();
            v.into_iter().collect()
        };
        let rep = if trail.is_empty() || t.trim_end().ends_with(['.', ',', '?', '!', ':', ';']) {
            t
        } else {
            format!("{}{} ", t.trim_end(), trail)
        };
        res.replace_range(a..b, &rep);
    }
    (fix_spacing(res.trim_start()), n)
}

/// Parole che il modello può togliere senza perdere contenuto (regola 3 del prompt: esitazioni,
/// intercalari, attacchi del parlato): non contano nei tratti tolti e non si rimettono ai bordi.
const DROPPABLE: &[&str] = &[
    "ehm", "uhm", "mmh", "eh", "ehh", "uh", "appunto", "cioè", "tipo", "diciamo", "praticamente", "insomma", "boh",
    "ok", "okay", "allora", "dunque", "comunque", "quindi", "beh", "vabbè", "ecco",
];
/// Indicatori di autocorrezione (regola 4): il tratto tolto vicino a questi è una correzione.
const SELF_FIX: &[&str] = &["scusa", "scusami", "anzi", "aspetta", "volevo", "correggo", "cancella", "scratch", "sbagliato"];
/// Ordinali e connettivi degli elenchi: spariscono quando il modello fa un elenco ("la prima").
const LIST_WORDS: &[&str] = &[
    "primo", "prima", "secondo", "seconda", "terzo", "terza", "quarto", "quarta", "quinto", "quinta", "ultimo", "ultima",
    "poi", "infine",
];
const ARTICLES: &[&str] = &["la", "il", "lo", "l"];

/// Numero in cifre o a parole (o unità): il modello li riscrive in cifre, il tratto non si rimette.
fn numberish(w: &str) -> bool {
    const MORE: &[&str] = &[
        "zero", "tredici", "quattordici", "sedici", "diciassette", "diciotto", "diciannove", "sessanta", "settanta",
        "ottanta", "novanta", "mila", "milioni", "milione", "miliardi", "euro", "percento", "gradi", "mezza", "mezzo",
    ];
    w.chars().any(|c| c.is_ascii_digit())
        || NUMBER_WORDS.iter().any(|(n, _)| *n == w)
        || MORE.contains(&w)
        || w.ends_with("mila")
}

/// Parole del grezzo tolte dal modello senza motivo (con la voce bassa il
/// modello locale toglie pezzi di frase, accorcia "quelle che sono", lascia cadere la coda della
/// dettatura). Ogni tratto del grezzo che nell'uscita manca, o che il modello ha sostituito con
/// meno parole, torna al suo posto, salvo:
/// - riempitivi e intercalari (`DROPPABLE`) e ripetizioni delle 6 parole vicine;
/// - autocorrezioni ("scusa", "volevo dire", "anzi") nel tratto o subito prima;
/// - false partenze: una parola del tratto ritorna nelle 4 parole dopo ("i vari... le varie");
/// - numeri, simboli detti, parole incollate o riscritte con le stesse lettere ("demo lab"
///   diventato DEMOLAB, "file punto py" diventato "file.py");
/// - ordinali di un elenco e parole corte prima della punteggiatura (code di frase come "e", "ma");
/// - tratti dove il modello ha scritto un termine del dizionario (`terms`).
///
/// Vale per i livelli Exact e Tidy, quelli che non devono accorciare.
pub fn restore_dropped(raw: &str, out: &str, terms: &[String]) -> (String, u32) {
    let term_words: HashSet<String> = terms.iter().flat_map(|t| ltokens(t)).collect();
    let r_spans = token_spans(raw);
    let rt: Vec<String> = r_spans.iter().map(|(s, e)| raw[*s..*e].to_lowercase()).collect();
    let c_spans = token_spans(out);
    let ct: Vec<String> = c_spans.iter().map(|(s, e)| out[*s..*e].to_lowercase()).collect();
    if rt.is_empty() || ct.is_empty() {
        return (out.to_string(), 0);
    }
    let pairs = lcs_pairs(&rt, &ct);
    let mut anchors: Vec<(isize, isize)> = vec![(-1, -1)];
    anchors.extend(pairs.iter().map(|&(a, b)| (a as isize, b as isize)));
    anchors.push((rt.len() as isize, ct.len() as isize));
    let is = |w: &str, list: &[&str]| list.contains(&w);
    let prefix4 = |w: &str| w.chars().take(4).collect::<String>();
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    for w in anchors.windows(2) {
        let ((a0, b0), (a1, b1)) = (w[0], w[1]);
        let (i1, i2, j1, j2) = ((a0 + 1) as usize, a1 as usize, (b0 + 1) as usize, b1 as usize);
        if i2 <= i1 {
            continue;
        }
        let gap = &rt[i1..i2];
        let og = &ct[j1..j2];
        // sostituzione: solo se il modello ha messo meno parole di quelle che ha tolto
        if j2 > j1 && i2 - i1 < j2 - j1 + 1 {
            continue;
        }
        let keep: Vec<&String> = gap.iter().filter(|w| !is(w, DROPPABLE)).collect();
        if keep.is_empty() {
            continue;
        }
        if gap.iter().chain(rt[i1.saturating_sub(2)..i1].iter()).any(|w| is(w, SELF_FIX)) {
            continue;
        }
        let near_before = &rt[i1.saturating_sub(6)..i1];
        let near_after = &rt[i2..(i2 + 6).min(rt.len())];
        if keep.iter().all(|w| near_before.contains(w) || near_after.contains(w)) {
            continue; // ripetizione
        }
        let next: HashSet<String> = rt[i2..(i2 + 4).min(rt.len())]
            .iter()
            .filter(|w| w.chars().count() >= 4)
            .map(|w| prefix4(w))
            .collect();
        if keep.iter().any(|w| w.chars().count() >= 4 && next.contains(&prefix4(w))) {
            continue; // falsa partenza
        }
        if gap.iter().chain(og.iter()).any(|w| numberish(w)) || gap.iter().any(|w| is(w, SPOKEN_SYMBOLS)) {
            continue;
        }
        if !og.is_empty() && super::dictionary::normalized_distance(&compact_words(gap), &compact_words(og)) <= 0.34 {
            continue; // stesse lettere, solo incollate o riscritte
        }
        // il modello ha scritto lì un termine del dizionario ("parola nomeuno" diventato NOMEUNO):
        // il termine resta, il tratto non si rimette
        if og.iter().any(|o| term_words.contains(o)) {
            continue;
        }
        if keep.iter().all(|w| is(w, LIST_WORDS) || is(w, ARTICLES)) && keep.iter().any(|w| !is(w, ARTICLES)) {
            continue;
        }
        if r_spans[i1..i2].iter().any(|(s, _)| raw[..*s].ends_with("[[")) {
            continue; // segnaposto di uno snippet
        }
        if keep.iter().all(|w| w.chars().count() <= 3) {
            let pos = if j2 > j1 { c_spans[j2 - 1].1 } else if b0 >= 0 { c_spans[b0 as usize].1 } else { 0 };
            let next = out[pos..].trim_start_matches(' ');
            if next.is_empty() || next.starts_with(['.', ',', ';', ':', '?', '!', '\n']) {
                continue;
            }
        }
        // riempitivi ai bordi del tratto restano fuori
        let (mut s, mut e) = (i1, i2);
        while s < e && is(&rt[s], DROPPABLE) {
            s += 1;
        }
        while e > s && is(&rt[e - 1], DROPPABLE) {
            e -= 1;
        }
        let piece = &raw[r_spans[s].0..r_spans[e - 1].1];
        if j2 > j1 {
            edits.push((c_spans[j1].0, c_spans[j2 - 1].1, piece.to_string()));
        } else if b0 >= 0 {
            let p = c_spans[b0 as usize].1;
            edits.push((p, p, format!(" {piece}")));
        } else {
            edits.push((0, 0, format!("{piece} ")));
        }
    }
    if edits.is_empty() {
        return (out.to_string(), 0);
    }
    let n = edits.len() as u32;
    let mut res = out.to_string();
    for (a, b, t) in edits.into_iter().rev() {
        let t = if a == 0 {
            // tratto rimesso in testa: la maiuscola passa al tratto
            let rest = &res[b..];
            let first = ltokens(rest).into_iter().next().unwrap_or_default();
            let lower_rest = super::dictionary::is_common_word(&first) && !rest.trim_start().starts_with(|c: char| c.is_uppercase() && rest.trim_start().chars().nth(1).is_some_and(|d| d.is_uppercase()));
            let head = upper_first(&t);
            if lower_rest {
                let tail = lower_first(rest);
                res = format!("{head}{tail}");
                continue;
            }
            head
        } else {
            t
        };
        res.replace_range(a..b, &t);
    }
    (fix_spacing(&res), n)
}

/// Parole dei comandi detti ("a capo", "nuova riga"): il tratto che le contiene non si tocca.
const COMMAND_WORDS: &[&str] = &["capo", "riga", "paragrafo", "invio", "enter"];

/// Minuscole senza accenti, per i confronti delle grafie ("perchè" e "perché" sono la stessa parola).
fn fold(s: &str) -> String {
    s.to_lowercase()
        .chars()
        .map(|c| match c {
            'à' | 'á' | 'â' | 'ä' => 'a',
            'è' | 'é' | 'ê' | 'ë' => 'e',
            'ì' | 'í' | 'î' | 'ï' => 'i',
            'ò' | 'ó' | 'ô' | 'ö' => 'o',
            'ù' | 'ú' | 'û' | 'ü' => 'u',
            c => c,
        })
        .collect()
}

/// Cambio della sola desinenza: le due parole hanno la stessa radice e differiscono solo nelle
/// ultime 3 lettere al massimo ("dettagliato" e "dettagliata", "fanno" e "fa", "ho" e "ha",
/// "vengono" e "vengano"). È un cambio di genere, numero, persona o modo: cambia il senso.
fn ending_change(a: &str, b: &str) -> bool {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let p = a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count();
    p >= 1 && a.len() - p <= 3 && b.len() - p <= 3
}

/// Una parola del grezzo e quella che il modello ha scritto al suo posto sono la stessa parola
/// detta, scritta meglio? Sì per accenti ("perchè"), h ("o fatto" diventa "ho fatto"), elisioni
/// ("la" davanti all'apostrofo diventa "l'"), d eufonica ("a" e "ad") e storpiature dell'ASR
/// corrette nel mezzo della parola ("formatazione", "trascrizioni" per "transcrizioni"). No per
/// i cambi di desinenza, le parole diverse e i sinonimi.
fn same_spoken_word(g: &str, o: &str, o_elided: bool, g_elided: bool) -> bool {
    let (g, o) = (fold(g), fold(o));
    if g == o || g.replace('h', "") == o.replace('h', "") {
        return true;
    }
    if (o_elided && g.starts_with(&o) && g.len() - o.len() <= 2) || (g_elided && o.starts_with(&g) && o.len() - g.len() <= 2) {
        return true;
    }
    if matches!((g.as_str(), o.as_str()), ("a", "ad") | ("ad", "a") | ("e", "ed") | ("ed", "e") | ("o", "od") | ("od", "o")) {
        return true;
    }
    g.chars().count() >= 4
        && o.chars().count() >= 4
        && super::dictionary::normalized_distance(&g, &o) <= 0.34
        && !ending_change(&g, &o)
}

/// Il token che finisce in `end` è seguito da un apostrofo (forma elisa: "l'", "dell'", "un'").
fn elided_at(text: &str, end: usize) -> bool {
    text[end..].starts_with(['\'', '’'])
}

/// Prima lettera maiuscola come la parola che sostituisce (inizio frase): "Egli" torna "E gli".
fn case_like(piece: &str, model_word: &str) -> String {
    if model_word.chars().next().is_some_and(|c| c.is_uppercase()) {
        upper_first(piece)
    } else {
        piece.to_string()
    }
}

/// "Niente parole cambiate" (per esempio "e gli ho portato" diventato "Egli mi ha portato"):
/// ogni parola che il modello ha scritto al posto di parole dette, o in
/// più, torna come nel grezzo. Il grezzo è il testo dato al modello (dopo backtrack e sostituzioni
/// del dizionario). Restano le modifiche ammesse:
/// - punteggiatura e maiuscole (non contano nell'allineamento);
/// - stesse lettere incollate o staccate, stessa parola scritta meglio (`same_spoken_word`);
/// - parole nello stesso tratto solo in ordine diverso, parole spostate da un altro punto;
/// - numeri in cifre, simboli e comandi detti ("slash", "punto", "a capo");
/// - termini del dizionario e delle correzioni (`terms`: quelli lontani li toglie già
///   `revert_far_terms`), segnaposto degli snippet;
/// - autocorrezioni ("scusa", "anzi", "volevo dire") nel tratto o subito prima;
/// - parole tolte: se ne occupa `restore_dropped`, che sa quali tagli sono giusti (riempitivi,
///   ripetizioni, false partenze). Qui contano solo i tratti dove il modello ha scritto almeno
///   tante parole quante ne ha prese dal grezzo, senza contare i riempitivi.
///
/// Nelle sostituzioni una a una torna solo la parola cambiata; negli altri tratti torna il tratto
/// intero del grezzo; le parole aggiunte si tolgono. Restituisce il testo e il numero di tratti
/// riportati al grezzo.
pub fn revert_changed_words(raw: &str, out: &str, terms: &[String]) -> (String, u32) {
    let term_words: HashSet<String> = terms.iter().flat_map(|t| ltokens(t)).collect();
    let r_spans = token_spans(raw);
    let rt: Vec<String> = r_spans.iter().map(|(s, e)| raw[*s..*e].to_lowercase()).collect();
    let c_spans = token_spans(out);
    let ct: Vec<String> = c_spans.iter().map(|(s, e)| out[*s..*e].to_lowercase()).collect();
    if rt.is_empty() || ct.is_empty() {
        return (out.to_string(), 0);
    }
    let pairs = lcs_pairs(&rt, &ct);
    let aligned: HashSet<usize> = pairs.iter().map(|&(a, _)| a).collect();
    // parole del grezzo che il modello non ha lasciato al loro posto: se compaiono altrove
    // nell'uscita sono state spostate, non inventate
    let mut unaligned: Vec<String> = (0..rt.len()).filter(|i| !aligned.contains(i)).map(|i| fold(&rt[i])).collect();
    let mut anchors: Vec<(isize, isize)> = vec![(-1, -1)];
    anchors.extend(pairs.iter().map(|&(a, b)| (a as isize, b as isize)));
    anchors.push((rt.len() as isize, ct.len() as isize));
    let is = |w: &str, list: &[&str]| list.contains(&w);
    let digit = |w: &String| w.chars().any(|c| c.is_ascii_digit());
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut gaps = 0u32;
    for w in anchors.windows(2) {
        let ((a0, b0), (a1, b1)) = (w[0], w[1]);
        let (i1, i2, j1, j2) = ((a0 + 1) as usize, a1 as usize, (b0 + 1) as usize, b1 as usize);
        if j2 <= j1 {
            continue; // niente scritto dal modello: parole tolte, vedi restore_dropped
        }
        let g: Vec<usize> = (i1..i2).filter(|&i| !is(&rt[i], DROPPABLE)).collect();
        let o = &ct[j1..j2];
        if g.len() > o.len() {
            continue; // il modello ha accorciato: lo decide restore_dropped
        }
        let gw: Vec<&String> = g.iter().map(|&i| &rt[i]).collect();
        // modifiche ammesse sull'intero tratto
        if o.iter().any(digit) || gw.iter().any(|w| digit(w)) {
            continue;
        }
        if gw.iter().any(|w| is(w, SPOKEN_SYMBOLS) || is(w, COMMAND_WORDS)) {
            continue;
        }
        if o.iter().any(|w| term_words.contains(w)) {
            continue;
        }
        if rt[i1..i2].iter().chain(rt[i1.saturating_sub(2)..i1].iter()).any(|w| is(w, SELF_FIX)) {
            continue;
        }
        let snippet = |text: &str, spans: &[(usize, usize)]| spans.iter().any(|(s, e)| text[..*s].ends_with("[[") || text[*e..].starts_with("]]"));
        if snippet(raw, &r_spans[i1..i2]) || snippet(out, &c_spans[j1..j2]) {
            continue;
        }
        let gc = fold(&compact_words(&gw));
        let oc = fold(&compact_words(o));
        if gc == oc || gc.replace('h', "") == oc.replace('h', "") {
            continue; // stesse lettere: "su la" e "sulla", "demo lab" e "demolab"
        }
        let mut gs: Vec<String> = gw.iter().map(|w| fold(w)).collect();
        let mut os: Vec<String> = o.iter().map(|w| fold(w)).collect();
        gs.sort();
        os.sort();
        if gs == os {
            continue; // stesse parole in un altro ordine
        }
        let mut changed = false;
        if g.is_empty() {
            // parole aggiunte: via, salvo quelle spostate da un altro punto del grezzo
            for j in j1..j2 {
                if let Some(k) = unaligned.iter().position(|u| *u == fold(&ct[j])) {
                    unaligned.swap_remove(k);
                    continue;
                }
                let (s, e) = c_spans[j];
                let e = if elided_at(out, e) { e + out[e..].chars().next().map_or(0, |c| c.len_utf8()) } else { e };
                edits.push((s, e, String::new()));
                changed = true;
            }
        } else if g.len() == o.len() {
            // una a una: torna solo la parola cambiata
            for (k, &i) in g.iter().enumerate() {
                let j = j1 + k;
                if same_spoken_word(&rt[i], &ct[j], elided_at(out, c_spans[j].1), elided_at(raw, r_spans[i].1)) {
                    continue;
                }
                let (s, e) = c_spans[j];
                let piece = &raw[r_spans[i].0..r_spans[i].1];
                // la forma elisa dell'uscita ("l'") torna intera: l'apostrofo resta solo se c'era nel grezzo
                let e = if elided_at(out, e) && !elided_at(raw, r_spans[i].1) {
                    e + out[e..].chars().next().map_or(0, |c| c.len_utf8())
                } else {
                    e
                };
                let piece = if out[e..].starts_with(|c: char| c.is_alphanumeric()) { format!("{piece} ") } else { piece.to_string() };
                edits.push((s, e, case_like(&piece, &out[c_spans[j].0..c_spans[j].1])));
                changed = true;
            }
        } else {
            // più parole di quelle dette: torna il tratto intero del grezzo, senza i riempitivi ai bordi
            let (s, e) = (g[0], *g.last().unwrap());
            let piece = &raw[r_spans[s].0..r_spans[e].1];
            let (cs, ce) = (c_spans[j1].0, c_spans[j2 - 1].1);
            edits.push((cs, ce, case_like(piece, &out[c_spans[j1].0..c_spans[j1].1])));
            changed = true;
        }
        if changed {
            gaps += 1;
        }
    }
    if edits.is_empty() {
        return (out.to_string(), 0);
    }
    edits.sort_by_key(|e| std::cmp::Reverse(e.0));
    let mut res = out.to_string();
    for (a, b, t) in edits {
        res.replace_range(a..b, &t);
    }
    // una parola tolta in testa o dopo un punto non lascia la virgola che la seguiva
    static ORPHAN_START: Lazy<Regex> = Lazy::new(|| Regex::new(r"(?m)^[ \t]*[,;:][ \t]*").unwrap());
    static ORPHAN: Lazy<Regex> = Lazy::new(|| Regex::new(r"([.!?])[ \t]*[,;:][ \t]*").unwrap());
    let res = ORPHAN_START.replace_all(&res, "").into_owned();
    let res = ORPHAN.replace_all(&res, "$1 ").into_owned();
    let mut res = fix_sentence_caps(&fix_spacing(&res));
    if out.trim_start().starts_with(|c: char| c.is_uppercase()) {
        res = upper_first(&res);
    }
    (res, gaps)
}

/// Appiattisce gli elenchi (righe con trattino o numerate) in una frase, per le app che non li
/// reggono (terminale, chat personali).
pub fn flatten_lists(text: &str) -> String {
    static ITEM: Lazy<Regex> = Lazy::new(|| Regex::new(r"^\s*(?:[-•*]|\d+[.)])\s+(.*)$").unwrap());
    let lines: Vec<&str> = text.lines().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if ITEM.is_match(lines[i]) {
            let mut items = Vec::new();
            while i < lines.len() && (ITEM.is_match(lines[i]) || lines[i].trim().is_empty() && i + 1 < lines.len() && ITEM.is_match(lines[i + 1])) {
                if let Some(c) = ITEM.captures(lines[i]) {
                    items.push(c[1].trim().trim_end_matches(['.', ';', ',']).to_string());
                }
                i += 1;
            }
            let joined = items.join(", ");
            // attacca alla riga di introduzione (anche oltre una riga vuota)
            while out.last().is_some_and(|l| l.trim().is_empty()) {
                out.pop();
            }
            match out.last_mut() {
                Some(prev) if !prev.trim().is_empty() => {
                    let p = prev.trim_end().to_string();
                    *prev = format!("{} {}.", p, joined);
                }
                _ => out.push(format!("{}.", upper_first(&joined))),
            }
            // salta le righe vuote dopo l'elenco e continua sulla stessa riga
            while i < lines.len() && lines[i].trim().is_empty() {
                i += 1;
            }
            if i < lines.len() && !ITEM.is_match(lines[i]) {
                let next = lines[i].trim().to_string();
                if let Some(prev) = out.last_mut() {
                    *prev = format!("{} {}", prev, next);
                }
                i += 1;
            }
            continue;
        }
        out.push(lines[i].to_string());
        i += 1;
    }
    out.join("\n").replace("..", ".")
}

/// Formato "Casual messaging" (LOWERCASE_SENTENCES): frasi con l'iniziale minuscola, salvo
/// termini del dizionario, sigle e nomi da tenere.
pub fn lowercase_sentences(text: &str, keep: &HashSet<String>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at_start = true;
    let mut chars = text.char_indices().peekable();
    while let Some((i, c)) = chars.next() {
        if at_start && c.is_alphabetic() {
            let word: String = text[i..].chars().take_while(|ch| is_word_char(*ch)).collect();
            let all_caps = word.chars().count() >= 2 && word.chars().all(|ch| !ch.is_lowercase());
            if all_caps || keep.contains(&word.to_lowercase()) || word == "I" {
                out.push(c);
            } else {
                out.extend(c.to_lowercase());
            }
            at_start = false;
            continue;
        }
        if matches!(c, '.' | '!' | '?' | '\n') {
            at_start = true;
        } else if !c.is_whitespace() && !matches!(c, '-' | '"' | '«' | '(' | '“') {
            at_start = false;
        }
        out.push(c);
    }
    out
}

/// Maiuscola e spazio iniziali dal testo già presente nel campo prima del cursore: se continua
/// una frase, la prima parola va in minuscolo (il difetto più corretto a mano dall'utente).
pub fn apply_field_context(text: &str, field_before: Option<&str>, keep: &HashSet<String>) -> String {
    // campo vuoto o sconosciuto: maiuscola iniziale
    let before = field_before.unwrap_or("");
    if before.trim().is_empty() {
        return upper_first(text);
    }
    let last = before.trim_end().chars().next_back().unwrap_or('.');
    let mut out = text.to_string();
    let first_word: String = out.chars().take_while(|c| is_word_char(*c)).collect();
    if !matches!(last, '.' | '!' | '?') && !before.ends_with('\n') {
        let all_caps = first_word.chars().count() >= 2 && first_word.chars().all(|c| !c.is_lowercase());
        if !all_caps && !keep.contains(&first_word.to_lowercase()) {
            out = lower_first(&out);
        }
    } else {
        out = upper_first(&out);
    }
    let needs_space = !before.ends_with(char::is_whitespace)
        && !out.starts_with([',', '.', ';', ':', '!', '?', ')', ' ']);
    if needs_space {
        out.insert(0, ' ');
    }
    out
}

/// Spazi doppi, spazio prima della punteggiatura, righe vuote multiple.
pub fn fix_spacing(text: &str) -> String {
    static MULTI: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]{2,}").unwrap());
    static BEFORE_P: Lazy<Regex> = Lazy::new(|| Regex::new(r"[ \t]+([,.;:!?)])").unwrap());
    static NL: Lazy<Regex> = Lazy::new(|| Regex::new(r"\n{3,}").unwrap());
    static DUP_P: Lazy<Regex> = Lazy::new(|| Regex::new(r"([,;])\s*([,.;])").unwrap());
    let t = MULTI.replace_all(text, " ");
    let t = BEFORE_P.replace_all(&t, "$1");
    let t = DUP_P.replace_all(&t, "$2");
    let t: Vec<String> = t.lines().map(|l| l.trim_end().to_string()).collect();
    let t = t.join("\n");
    let t = NL.replace_all(&t, "\n\n");
    t.trim().to_string()
}

/// Maiuscola dopo punto, punto interrogativo o esclamativo seguito da uno spazio (dopo le
/// sostituzioni). Un punto dentro una parola ("claude.md", "5.000") non apre una frase.
pub fn fix_sentence_caps(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut punct = false;
    let mut armed = false;
    for c in text.chars() {
        if armed && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            armed = false;
            punct = false;
            continue;
        }
        if matches!(c, '.' | '!' | '?') {
            punct = true;
            armed = false;
        } else if c.is_whitespace() {
            if punct {
                armed = true;
            }
        } else {
            punct = false;
            armed = false;
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::DictionarySource;

    fn entry(phrase: &str, replacement: Option<&str>, snippet: bool, ambiguous: bool) -> DictionaryEntry {
        DictionaryEntry {
            id: phrase.to_string(),
            phrase: phrase.to_string(),
            replacement: replacement.map(|s| s.to_string()),
            is_snippet: snippet,
            starred: false,
            frequency: 0,
            last_used: None,
            source: DictionarySource::Manual,
            ambiguous,
        }
    }

    #[test]
    fn backtrack_frase_parallela() {
        assert_eq!(
            backtrack("parola uno di prova alfa anzi no di prova beta"),
            "parola uno di prova beta"
        );
        assert_eq!(
            backtrack("parola uno valore alto scusa valore basso"),
            "parola uno valore basso"
        );
    }

    #[test]
    fn backtrack_non_tocca_anzi_no_che_annulla() {
        let t = "parola uno parola due parola tre anzi no niente";
        assert_eq!(backtrack(t), t);
    }

    #[test]
    fn backtrack_scratch_that() {
        assert_eq!(
            backtrack("Frase uno. Frase due scratch that. Frase tre."),
            "Frase uno. Frase tre."
        );
    }

    #[test]
    fn premi_invio_nelle_varianti() {
        for t in ["testo di prova premi invio", "testo di prova, press enter.", "testo di prova invia", "testo di prova Premi Invio!"] {
            let (out, enter) = strip_press_enter(t);
            assert!(enter, "{t}");
            assert_eq!(out, "testo di prova", "{t}");
        }
        let (out, enter) = strip_press_enter("parola l'invia parola due");
        assert!(!enter);
        assert_eq!(out, "parola l'invia parola due");
    }

    #[test]
    fn sostituzioni_a_confini_di_parola() {
        let rules = Rules::from_entries(&[entry("parola alfa", Some("ParolaAlfa"), false, false), entry("beta", Some("BETA"), false, false)]);
        let a = rules.apply_replacements("testo Parola  Alfa e non betamax, ma beta sì");
        assert_eq!(a.text, "testo ParolaAlfa e non betamax, ma BETA sì");
        assert_eq!(a.count, 2);
        // forma del dizionario imposta solo per le maiuscole volute
        let rules = Rules::from_entries(&[entry("tERMINEPROVA", None, false, false), entry("NOME ESEMPIO", None, false, false)]);
        assert_eq!(rules.apply_replacements("testo termineprova e nome esempio").text, "testo termineprova e NOME ESEMPIO");
    }

    #[test]
    fn sostituzione_ambigua_solo_con_contesto() {
        let rules = Rules::from_entries(&[entry("cloud", Some("Claude"), false, true)]);
        assert_eq!(rules.apply_replacements("testo di prova con prompt e cloud").text, "testo di prova con prompt e Claude");
        assert_eq!(rules.apply_replacements("testo di prova con cloud e altro").text, "testo di prova con cloud e altro");
    }

    #[test]
    fn snippet_su_dettatura_intera_e_dentro_il_testo() {
        let rules = Rules::from_entries(&[entry("indirizzo di prova", Some("nome@example.com"), true, false)]);
        assert_eq!(rules.whole_snippet("Indirizzo di prova.").map(|x| x.0), Some("nome@example.com".to_string()));
        let (marked, s) = rules.mark_snippets("parola uno indirizzo di prova parola due");
        assert_eq!(marked, "parola uno [[S1]] parola due");
        assert_eq!(unmark_snippets("Parola uno [[S1]], parola due.", &s).unwrap(), "Parola uno nome@example.com, parola due.");
        assert!(unmark_snippets("Parola uno, parola due.", &s).is_none());
    }

    #[test]
    fn salto_della_pulizia() {
        assert!(should_skip("sull'altra parola", 4));
        assert!(should_skip("Parola uno parola due parola tre.", 4));
        assert!(!should_skip("parola uno parola due parola tre", 4));
        assert!(!should_skip("parola uno parola due città", 4)); // ultimo carattere di 2 byte
        assert!(!should_skip("Frase uno di prova. Frase due di prova con A e B.", 4));
    }

    #[test]
    fn termine_perso_viene_ripristinato() {
        let raw = "testo di prova con TERMINEPROVA e altro";
        let cleaned = "Testo di prova con Terminiprova e altro.";
        let (out, n) = restore_terms(raw, cleaned, &["TERMINEPROVA".to_string()], &[]);
        assert_eq!(n, 1);
        assert_eq!(out, "Testo di prova con TERMINEPROVA e altro.");
        let (same, n0) = restore_terms(raw, "Testo di prova con TERMINEPROVA e altro.", &["TERMINEPROVA".to_string()], &[]);
        assert_eq!((same.as_str(), n0), ("Testo di prova con TERMINEPROVA e altro.", 0));
    }

    #[test]
    fn appunto_intercalare_tolto() {
        assert_eq!(drop_fillers("Parola uno appunto parola due."), "Parola uno parola due.");
        assert_eq!(drop_fillers("Appunto, parola uno parola due."), "Parola uno parola due.");
        assert_eq!(drop_fillers("Testo con un appunto e l'appunto di prova."), "Testo con un appunto e l'appunto di prova.");
    }

    #[test]
    fn dettatura_lunga_divisa_in_due_metà() {
        let corto = "una frase di poche parole";
        assert!(split_halves(corto).is_none());
        let a: String = (1..=28).map(|k| format!("alfa{k} ")).collect(); // 28 parole tutte diverse
        let b: String = (1..=28).map(|k| format!("beta{k} ")).collect();
        let (x, y, fine) = split_halves(&format!("{}. {}", a.trim(), b.trim())).unwrap();
        assert!(fine && x.ends_with("alfa28.") && y.starts_with("beta1"), "{x} | {y}");
        let (x, y, fine) = split_halves(&format!("{} [[S1]] {}", a.trim(), b.trim())).unwrap();
        assert!(!fine && !x.ends_with("[[") && !y.starts_with("S1]]"), "{x} | {y}");
        // ripetizione a cavallo del centro e nessuna fine frase: niente taglio lì
        let rip = format!("{} prova delle ripetute dei ripetute delle ripetute {}", a.trim(), b.trim());
        match split_halves(&rip) {
            None => {}
            Some((x, y, _)) => {
                let tail: Vec<String> = ltokens(&x).into_iter().rev().take(6).collect();
                let head: Vec<String> = ltokens(&y).into_iter().take(6).collect();
                assert!(!tail.contains(&"ripetute".to_string()) || !head.contains(&"ripetute".to_string()), "{x} | {y}");
            }
        }
        assert_eq!(join_halves("Frase uno.", "Poi frase due.", false, "poi frase due"), "Frase uno poi frase due.");
        assert_eq!(join_halves("Frase uno.", "Nome Esempio due.", false, "Nome Esempio due"), "Frase uno Nome Esempio due.");
        assert_eq!(join_halves("Frase uno.", "Poi frase due.", true, "poi frase due"), "Frase uno. Poi frase due.");
    }

    #[test]
    fn pezzi_del_prompt_nell_uscita() {
        let user = "<contesto>\nStile: naturale e diretto, come parla l'utente. Non renderlo più formale.\nFormato: testo normale.\nApp: SnippingTool (categoria other).\n</contesto>\n<trascrizione>\nfrase di prova uno e due\n</trascrizione>\nTesto pulito:";
        let system = "Regole lunghe che il modello non deve ricopiare mai.\n- TERMINEUNO\n- TERMINEDUE\n- btw → by the way";
        assert!(!prompt_leak("Frase di prova uno e due.", user, system));
        // echi del prompt da riconoscere
        assert!(prompt_leak("Frase di prova.\nDizionario:\n- TERMINEUNO", user, system));
        assert!(prompt_leak("- by the way → by the way\nFrase di prova.", user, system));
        assert!(prompt_leak("Frase di prova. Stile: naturale e diretto", user, system));
        assert!(prompt_leak("frase di prova app: SnippingTool (categoria other).", user, system));
        assert!(prompt_leak("Frase di prova. Non renderlo più formale.", user, system));
        assert!(prompt_leak("<trascrizione>Frase di prova.</trascrizione>", user, system));
        assert!(prompt_leak("Frase di prova.\n- TERMINEUNO\n- TERMINEDUE", user, system));
        // una voce del dizionario dentro il testo non è un'eco
        assert!(!prompt_leak("Frase di prova con TERMINEUNO dentro.", user, system));
    }

    /// Termini delle correzioni fusi, storpiati o tolti vengono rimessi; la grafia cambia solo
    /// per i termini distintivi, mai per le parole comuni. Liste di prova scritte qui, non
    /// quelle locali.
    #[test]
    fn termini_delle_correzioni_fusi_storpiati_o_tolti() {
        let list = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<String>>();
        let terms = list(&["Nomeprova", "CHATGPT", "file-provaV2", "GitHub", "PDF", "TAVOLI", "Desktop", "CHAT"]);
        let spell = list(&["Nomeprova", "ChatGPT", "file-provaV2", "GitHub", "PDF"]);
        let count = |t: &str, w: &str| ltokens(t).iter().filter(|x| *x == w).count();
        // fuso: "studio nomeprova" diventato "StudioNomeprova", tre volte; rimesso con la grafia "Nomeprova"
        let raw = "parola uno studio nomeprova parola due, poi studio nomeprova parola tre e studio nomeprova parola quattro";
        let cleaned = "Parola uno StudioNomeprova parola due, poi StudioNomeprova parola tre e StudioNomeprova parola quattro.";
        let (out, n) = restore_terms(raw, cleaned, &terms, &spell);
        assert_eq!((count(&out, "nomeprova"), n), (3, 3), "{out}");
        assert_eq!(out.matches("studio Nomeprova").count(), 3, "{out}");
        // storpiato: "studio nomeprova x" diventato "StudioNox"
        let (out, _) = restore_terms("parola uno studio nomeprova x parola due", "Parola uno StudioNox parola due.", &terms, &spell);
        assert_eq!(count(&out, "nomeprova"), 1, "{out}");
        // tolto: "sul desktop" sparito, rimesso in minuscolo (parola comune: niente grafia)
        let (out, n) = restore_terms("parola uno sul desktop parola due", "Parola uno parola due.", &terms, &spell);
        assert_eq!(n, 1, "{out}");
        assert!(out.contains("sul desktop") && !out.contains("Desktop"), "{out}");
        // "chatgpt" rimesso prende la grafia della lista, "ChatGPT"
        let (out, _) = restore_terms("parola uno a chatgpt parola due", "Parola uno parola due.", &terms, &spell);
        assert!(out.contains("ChatGPT"), "{out}");
        // parola comune protetta: rimessa com'era nel grezzo
        let (out, _) = restore_terms("parola uno la chat parola due", "Parola uno parola due.", &terms, &spell);
        assert!(out.contains("la chat") && !out.contains("CHAT"), "{out}");
        // grafia della lista con minuscola iniziale e maiuscola dopo: si impone
        let (out, n) = restore_terms("parola uno il file-provav2 parola due", "Parola uno parola due.", &terms, &spell);
        assert!(n == 1 && out.contains("file-provaV2"), "{out}");
        // TAVOLI è una parola comune: protetta ma senza grafia imposta
        let (out, _) = restore_terms("parola uno i tavoli parola due", "Parola uno parola due.", &terms, &spell);
        assert!(out.contains("i tavoli") && !out.contains("TAVOLI"), "{out}");
        // le grafie storte del dizionario restano saltate
        assert_eq!(recase_terms("apri nomeprova", &["nOMEPROVA".to_string()], true), "apri nomeprova");
        assert_eq!(recase_terms("apri nomeprova", &["nOMEPROVA".to_string()], false), "apri nOMEPROVA");
    }

    /// Uscite sintetiche del Polish che rispondono invece di riscrivere.
    #[test]
    fn polish_che_risponde_riconosciuto() {
        let richiesta = "testo di prova da riscrivere con parola uno parola due parola tre parola quattro e parola cinque";
        let risposta = "Certo, ecco il testo di prova:\n\nOggetto: Prova\n\nTesto di prova con parola uno.\n\nUn saluto,\n[Nome]";
        assert!(polish_answer(richiesta, risposta, true));
        let elenco = "elenco di prova con voce uno voce due voce tre e voce quattro da ordinare";
        assert!(polish_answer(elenco, "- Voci di prova: [elenco voci]\n- Voci da ordinare: [da definire]", true));
        // riscritture vere: niente allarme
        assert!(!polish_answer(elenco, "Elenco di prova con voce uno, voce due, voce tre e voce quattro da ordinare.", true));
        assert!(!polish_answer(richiesta, "Testo di prova con parola uno, parola due, parola tre, parola quattro e parola cinque.", true));
        // con "Togli ripetizioni e giri di parole" attivo un'uscita più lunga non passa; senza, sì
        assert!(polish_answer("testo di prova", "Testo di prova con parole in più.", true));
        assert!(!polish_answer("testo di prova", "Testo di prova con parole in più.", false));
        // contenuto perso e traslitterazione
        let lunga = "Testo di prova lungo con parola uno, parola due, parola tre, parola quattro, parola cinque e parola sei per contare.";
        assert!(polish_answer(lunga, "Testo di prova breve.", true));
        assert!(polish_answer("परीक्षण पाठ एक दो तीन", "Parikshan path ek do teen", false));
        assert!(!polish_answer("परीक्षण पाठ एक दो तीन", "परीक्षण पाठ।", false));
        assert_eq!(polish_plain("Testo con `nome.md` di prova"), "Testo con nome.md di prova");
    }

    /// Copertura del testo: persona cambiata o informazione persa.
    #[test]
    fn polish_copertura_del_testo() {
        let dict = vec!["NOME ESEMPIO".to_string()];
        let miss = |a: &str, b: &str| polish_missing(a, b, &dict);
        // persona cambiata
        assert!(miss("io parola uno parola due", "Parola uno parola due.").contains(&"prima persona".to_string()));
        assert!(miss("parola uno mi parola due", "Parola uno parola due.").contains(&"prima persona".to_string()));
        // numeri e parole approssimative
        assert!(!miss("testo di prova con venti parole", "Testo di prova con 20 parole.").iter().any(|m| m == "venti"));
        assert!(miss("testo di prova con venti parole", "Testo di prova con molte parole.").contains(&"venti".to_string()));
        assert!(miss("testo di prova con quasi tre righe", "Testo di prova con 3 righe.").contains(&"quasi".to_string()));
        assert!(miss("testo di prova con il numero 4.000", "Testo di prova con il numero 4000.").is_empty());
        // termine del dizionario e parola con maiuscola persi
        assert!(miss("testo di prova con NOME ESEMPIO", "Testo di prova con esempio.").contains(&"NOME ESEMPIO".to_string()));
        assert!(miss("testo di prova con Termineprova in mezzo", "Testo di prova in mezzo.").contains(&"Termineprova".to_string()));
        // parola inglese persa ("script")
        assert!(miss("testo di prova con la parola script", "Testo di prova con la parola codice.").contains(&"script".to_string()));
        // riscrittura buona: niente da segnalare
        assert!(miss("io parola uno, mi parola due con quasi venti righe", "Io parola uno: mi parola due con quasi 20 righe.").is_empty());
    }

    /// Verbi sostituiti, Lei diventato tu, dettagli persi.
    #[test]
    fn polish_verbi_registro_e_dettagli() {
        let dict: Vec<String> = Vec::new();
        let parole = |m: &[String]| m.iter().any(|x| x.starts_with("parole:"));
        // verbi del testo sostituiti da altri verbi
        let verbi = "testo di prova con parola alfa e parola beta, controlla gamma e delta, verifica omega e lascia stare il resto";
        let verbi_out = "Testo di prova con parola alfa e parola beta: esamina gamma e delta, osserva omega senza alterare il resto.";
        assert!(parole(&polish_missing(verbi, verbi_out, &dict)), "{:?}", polish_missing(verbi, verbi_out, &dict));
        let verbi_buona = "Testo di prova con parola alfa e parola beta: controlla gamma e delta, verifica omega e lascia stare il resto.";
        assert!(polish_missing(verbi, verbi_buona, &dict).is_empty(), "{:?}", polish_missing(verbi, verbi_buona, &dict));
        // Lei diventato tu, "il primo valore" e "resta" persi
        let lei = "testo di prova per Nome Esempio, le confermo che il primo valore alfa resta di 1.500 come il valore beta, mi scriva pure";
        let lei_out = "Testo di prova per Nome Esempio, confermo che il valore alfa è di 1.500 come il valore beta. Mi scrivi pure.";
        let m = polish_missing(lei, lei_out, &dict);
        assert!(m.contains(&"registro Lei: le confermo".to_string()) && m.contains(&"registro Lei: mi scriva".to_string()), "{m:?}");
        assert!(m.contains(&"registro tu: scrivi".to_string()), "{m:?}");
        assert!(content_missing(&ltokens(lei), &ltokens(lei_out)).contains(&"primo".to_string()));
        let lei_buona = "Testo di prova per Nome Esempio, le confermo che il primo valore alfa resta di 1.500 come il valore beta. Mi scriva pure.";
        assert!(polish_missing(lei, lei_buona, &dict).is_empty(), "{:?}", polish_missing(lei, lei_buona, &dict));
        // parole di contenuto perse: "oggi" e "chiesto" contano come informazione
        let perse = "testo di prova con parola oggi e parola chiesto, poi parola alfa, parola beta, parola gamma e parola delta in fila";
        let perse_out = "Testo di prova con parola alfa, parola beta, parola gamma e parola delta in fila.";
        let m = polish_missing(perse, perse_out, &dict);
        assert!(parole(&m), "{m:?}");
        let gone = content_missing(&ltokens(perse), &ltokens(perse_out));
        assert!(gone.contains(&"oggi".to_string()) && gone.contains(&"chiesto".to_string()), "{gone:?}");
    }

    #[test]
    fn elenco_appiattito() {
        let t = "Elenco di prova:\n- voce alfa\n- voce per Nome Esempio\n\nFrase finale.";
        assert_eq!(flatten_lists(t), "Elenco di prova: voce alfa, voce per Nome Esempio. Frase finale.");
    }

    #[test]
    fn maiuscola_dal_campo() {
        let keep = HashSet::from(["termineprova".to_string()]);
        assert_eq!(apply_field_context("Parola due.", Some("Parola uno"), &keep), " parola due.");
        assert_eq!(apply_field_context("Termineprova due.", Some("parola uno"), &keep), " Termineprova due.");
        assert_eq!(apply_field_context("Parola due.", Some("Frase uno. "), &keep), "Parola due.");
    }

    #[test]
    fn uscita_del_modello_ripulita() {
        assert_eq!(sanitize_model_output("<think>\n</think>\n\nTesto pulito: \"Frase di prova.\""), "Frase di prova.");
        assert_eq!(sanitize_model_output("Stile: casual.\nTrascrizione: prova\nTesto pulito: Prova."), "Prova.");
        assert_eq!(sanitize_model_output("Formato: testo normale.\nTrascrizione: prova"), "");
        assert_eq!(sanitize_model_output("Ecco:<ul><li>uno</li><li>due</li></ul>"), "Ecco:\n- uno\n- due");
    }

    // Termini lontani: "temi" non suona come TUBI (0,50) e torna al grezzo; la
    // storpiatura "tubbi" (0,20) resta TUBI
    #[test]
    fn termine_lontano_torna_al_grezzo_vicino_resta() {
        let terms = vec!["TUBI".to_string()];
        let (t, n) = revert_far_terms("parola uno i temi, parola due", "Parola uno i TUBI, parola due.", &terms);
        assert_eq!((t.as_str(), n), ("Parola uno i temi, parola due.", 1));
        let (t, n) = revert_far_terms("parola uno i tubbi, parola due", "Parola uno i TUBI, parola due.", &terms);
        assert_eq!((t.as_str(), n), ("Parola uno i TUBI, parola due.", 0));
    }

    // dal grezzo "Terminecs" (0,22) il termine TERMINEX resta, l'errore lì è dell'ASR; da
    // "zetafon" (molto sopra la soglia) il modello non può portarlo
    #[test]
    fn termine_storpiato_resta_termine_lontano_torna() {
        let terms = vec!["TERMINEX".to_string()];
        let out = "Testo di prova con TERMINEX in mezzo.";
        let (t, n) = revert_far_terms("testo di prova con Terminecs in mezzo", out, &terms);
        assert_eq!((t.as_str(), n), (out, 0));
        let (t, n) = revert_far_terms("testo di prova con zetafon in mezzo", out, &terms);
        assert_eq!((t.as_str(), n), ("Testo di prova con zetafon in mezzo.", 1));
        // termine aggiunto dal nulla: si toglie
        let (t, n) = revert_far_terms("testo di prova in mezzo", "Testo di prova TERMINEX in mezzo.", &terms);
        assert_eq!((t.as_str(), n), ("Testo di prova in mezzo.", 1));
    }

    // storpiature vicine o già sostituite restano: "cloud" per Claude, "filecloud.md" incollato
    #[test]
    fn termine_vicino_o_gia_nel_grezzo_resta() {
        let terms = vec!["Claude".to_string(), "claude.md".to_string(), "TERMINEPROVA".to_string()];
        for (raw, out) in [
            ("testo cloud di prova", "Testo Claude di prova."),
            ("testo filecloud.md di prova", "Testo file claude.md di prova."),
            ("testo con TERMINEPROVA", "Testo con TERMINEPROVA."),
        ] {
            assert_eq!(revert_far_terms(raw, out, &terms), (out.to_string(), 0), "{raw}");
        }
    }

    // Parole tolte: la coda e i pezzi di frase persi tornano al loro posto
    #[test]
    fn tratti_tolti_tornano() {
        let raw = "frase alfa beta gamma delta che resta fuori dalla prova, poi frase epsilon zeta con la coda lunga del campione";
        let out = "Frase alfa beta gamma delta, poi frase epsilon zeta.";
        let (t, n) = restore_dropped(raw, out, &[]);
        assert_eq!(t, "Frase alfa beta gamma delta che resta fuori dalla prova, poi frase epsilon zeta con la coda lunga del campione.");
        assert_eq!(n, 2);
        // parola corta tolta in mezzo alla frase: "tu" torna
        let (t, _) = restore_dropped("testo alfa tu beta di prova", "Testo alfa beta di prova.", &[]);
        assert_eq!(t, "Testo alfa tu beta di prova.");
    }

    // tagli giusti del modello restano: riempitivi, autocorrezioni, ripetizioni, false partenze,
    // numeri, simboli detti, parole incollate, termini del dizionario, code di frase
    #[test]
    fn tagli_giusti_restano() {
        for (raw, out) in [
            ("ehm allora tipo parola di prova", "Parola di prova."),
            ("parola alfa anzi parola beta", "Parola beta."),
            ("parola parola alfa beta di prova", "Parola alfa beta di prova."),
            ("parola il campo alfa il campione beta di prova", "Parola il campione beta di prova."),
            ("testo di prova con duemila cinquecento parole", "Testo di prova con 2.500 parole."),
            ("testo con il file punto py e slash prova", "Testo con il file.py e /prova."),
            ("testo di termine prova", "Testo di TERMINEPROVA."),
            ("testo di prova con la voce nomprova e altro", "Testo di prova con NOMEPROVA e altro."),
            ("testo di prova con la coda e", "Testo di prova con la coda."),
        ] {
            assert_eq!(restore_dropped(raw, out, &["NOMEPROVA".to_string()]), (out.to_string(), 0), "{raw}");
        }
    }

    // niente parole cambiate: una parola detta sostituita da un'altra torna com'era,
    // anche se il modello ha cambiato solo la desinenza o la persona
    #[test]
    fn parole_cambiate_tornano_al_grezzo() {
        for (raw, out, want) in [
            ("e gli ho parola di prova", "Egli mi ha parola di prova.", "E gli ho parola di prova."),
            ("parola alfa il testo nel campo di prova", "Parola alfa nel testo nel campo di prova?", "Parola alfa il testo nel campo di prova?"),
            ("parola l'alfa colorato di prova", "Parola l'alfa colorata di prova.", "Parola l'alfa colorato di prova."),
            ("parola alfa che fanno di prova", "Parola alfa che fa di prova.", "Parola alfa che fanno di prova."),
            ("parola alfa veramente di prova", "Parola alfa davvero di prova.", "Parola alfa veramente di prova."),
        ] {
            let (t, n) = revert_changed_words(raw, out, &[]);
            assert_eq!(t, want, "{raw}");
            assert_eq!(n, 1, "{raw}");
        }
    }

    // parole che il modello ha aggiunto si tolgono; un tratto riscritto con più parole torna intero
    #[test]
    fn parole_aggiunte_si_tolgono() {
        assert_eq!(revert_changed_words("parola alfa il testo", "Parola alfa anche il testo.", &[]).0, "Parola alfa il testo.");
        assert_eq!(revert_changed_words("parola di prova", "Ok, parola di prova.", &[]).0, "Parola di prova.");
        assert_eq!(revert_changed_words("parola alfa sistemato", "Parola alfa messo a posto.", &[]).0, "Parola alfa sistemato.");
    }

    // le modifiche ammesse restano: accenti, h, elisioni, parole incollate, storpiature corrette,
    // numeri, simboli detti, termini del dizionario, autocorrezioni, ordine, snippet
    #[test]
    fn modifiche_ammesse_restano() {
        let terms = vec!["Claude Code".to_string(), "NOME ESEMPIO".to_string()];
        for (raw, out) in [
            ("parola perchè di prova", "Parola perché di prova?"),
            ("parola o fatto di prova", "Parola ho fatto di prova."),
            ("parola la immagine di prova", "Parola l'immagine di prova."),
            ("parola su la pagina di prova", "Parola sulla pagina di prova."),
            ("parola la formatazione di prova", "Parola la formattazione di prova."),
            ("parola slash slash alfa beta", "Parola /alfa beta."),
            ("parola cloud code di prova", "Parola Claude Code di prova."),
            ("la parola Nome Esempio", "La parola NOME ESEMPIO."),
            ("parola con duemila di prova", "Parola con 2.000 di prova."),
            ("parola alfa scusa beta", "Parola beta."),
            ("parola di prova [[S1]] e altro", "Parola di prova [[S1]] e altro."),
        ] {
            assert_eq!(revert_changed_words(raw, out, &terms), (out.to_string(), 0), "{raw}");
        }
        // una parola spostata non si perde
        assert_eq!(revert_changed_words("alfa beta gamma delta", "Gamma delta alfa beta.", &[]).1, 0);
    }
}
