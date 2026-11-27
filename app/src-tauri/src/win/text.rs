//! Funzioni pure sul testo: taglio su confine di parola, nomi propri,
//! confronto a parole fra testo incollato e testo corretto, ancore.

use super::WordChange;
use std::collections::HashSet;

/// Lunghezza massima di ciascun pezzo del campo.
pub const FIELD_PIECE_MAX_CHARS: usize = 500;

/// Tiene gli ultimi `max` caratteri (il testo vicino al cursore), partendo da
/// un confine di parola.
pub fn truncate_tail_on_word(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let skip = total - max;
    let start_byte = s.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    let tail = &s[start_byte..];
    // Se il taglio cade dentro una parola, salta il resto della parola.
    let prev_is_space = s[..start_byte]
        .chars()
        .next_back()
        .map(char::is_whitespace)
        .unwrap_or(true);
    if prev_is_space {
        return tail.to_string();
    }
    match tail.find(char::is_whitespace) {
        Some(i) => tail[i..].trim_start().to_string(),
        None => tail.to_string(),
    }
}

/// Tiene i primi `max` caratteri, finendo su un confine di parola.
pub fn truncate_head_on_word(s: &str, max: usize) -> String {
    let total = s.chars().count();
    if total <= max {
        return s.to_string();
    }
    let end_byte = s.char_indices().nth(max).map(|(i, _)| i).unwrap_or(s.len());
    let head = &s[..end_byte];
    let next_is_space = s[end_byte..]
        .chars()
        .next()
        .map(char::is_whitespace)
        .unwrap_or(true);
    if next_is_space {
        return head.to_string();
    }
    match head.rfind(char::is_whitespace) {
        Some(i) if i > 0 => head[..i].trim_end().to_string(),
        _ => head.to_string(),
    }
}

/// Parole comuni italiane (e qualche inglese) che a inizio frase o in un
/// titolo hanno la maiuscola ma non sono nomi propri.
const COMMON_WORDS: &[&str] = &[
    "il", "lo", "la", "i", "gli", "le", "un", "uno", "una", "di", "a", "da", "in", "con", "su",
    "per", "tra", "fra", "e", "ed", "o", "ma", "se", "che", "chi", "cosa", "come", "dove",
    "quando", "perché", "perche", "non", "si", "sì", "no", "ok", "ciao", "grazie", "questo",
    "questa", "quello", "quella", "allora", "poi", "anche", "ecco", "io", "tu", "lui", "lei",
    "noi", "voi", "loro", "mi", "ti", "ci", "vi", "del", "della", "dei", "delle", "al", "alla",
    "nel", "nella", "sono", "è", "ho", "hai", "ha", "abbiamo", "fai", "fa", "puoi", "può",
    "va", "bene", "buongiorno", "buonasera", "salve", "nuova", "nuovo", "chat", "file",
    "the", "a", "an", "and", "or", "of", "to", "in", "on", "for", "with", "new", "untitled",
    "home", "search", "settings",
];

fn is_common(word: &str) -> bool {
    let w = word.to_lowercase();
    COMMON_WORDS.contains(&w.as_str())
}

fn trim_punct(w: &str) -> &str {
    w.trim_matches(|c: char| !c.is_alphanumeric())
}

/// Nomi propri con un'euristica locale (senza LLM): parole con maiuscola
/// non a inizio frase, CamelCase, sigle, parole con cifre. Massimo `max`,
/// senza duplicati, nell'ordine in cui compaiono.
pub fn extract_proper_nouns(texts: &[&str], max: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for text in texts {
        let mut sentence_start = true;
        for raw in text.split_whitespace() {
            let ends_sentence = raw.ends_with(['.', '!', '?', ':', '\n']);
            let w = trim_punct(raw);
            let starts_now = sentence_start;
            sentence_start = ends_sentence;
            if w.chars().count() < 2 || is_common(w) {
                continue;
            }
            let chars: Vec<char> = w.chars().collect();
            let has_digit = chars.iter().any(|c| c.is_ascii_digit());
            let has_alpha = chars.iter().any(|c| c.is_alphabetic());
            let upper_count = chars.iter().filter(|c| c.is_uppercase()).count();
            let first_upper = chars[0].is_uppercase();
            let is_acronym = upper_count >= 2 && chars.iter().all(|c| !c.is_lowercase());
            let is_camel = chars.iter().skip(1).any(|c| c.is_uppercase())
                && chars.iter().any(|c| c.is_lowercase());
            let is_capitalized = first_upper && !starts_now;
            let is_name = (has_digit && has_alpha) || is_acronym || is_camel || is_capitalized;
            if !is_name || (has_digit && !has_alpha) {
                continue;
            }
            let key = w.to_lowercase();
            if seen.insert(key) {
                out.push(w.to_string());
                if out.len() >= max {
                    return out;
                }
            }
        }
    }
    out
}

/// Parola normalizzata per il confronto: senza punteggiatura ai bordi.
/// Le maiuscole contano (una correzione "claude" → "Claude" è da imparare).
fn norm(w: &str) -> &str {
    trim_punct(w)
}

/// Parole cambiate fra testo incollato e testo corretto (diff a parole con
/// LCS). Si tengono solo le sostituzioni (da qualcosa a qualcosa): inserimenti
/// e cancellazioni puri non sono correzioni da dizionario. I cambi di sola
/// punteggiatura si ignorano.
pub fn word_changes(pasted: &str, edited: &str) -> Vec<WordChange> {
    let a: Vec<&str> = pasted.split_whitespace().collect();
    let b: Vec<&str> = edited.split_whitespace().collect();
    let (n, m) = (a.len(), b.len());
    // Tabella LCS (testi corti: un incolla tipico ha decine di parole).
    let mut dp = vec![vec![0u32; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[i][j] = if norm(a[i]) == norm(b[j]) {
                dp[i + 1][j + 1] + 1
            } else {
                dp[i + 1][j].max(dp[i][j + 1])
            };
        }
    }
    let mut changes = Vec::new();
    let (mut i, mut j) = (0, 0);
    let mut del: Vec<&str> = Vec::new();
    let mut ins: Vec<&str> = Vec::new();
    let flush = |del: &mut Vec<&str>, ins: &mut Vec<&str>, out: &mut Vec<WordChange>| {
        if !del.is_empty() && !ins.is_empty() {
            let from: Vec<&str> = del.iter().map(|w| norm(w)).filter(|w| !w.is_empty()).collect();
            let to: Vec<&str> = ins.iter().map(|w| norm(w)).filter(|w| !w.is_empty()).collect();
            if !from.is_empty() && !to.is_empty() && from != to {
                out.push(WordChange {
                    from: from.join(" "),
                    to: to.join(" "),
                });
            }
        }
        del.clear();
        ins.clear();
    };
    while i < n || j < m {
        if i < n && j < m && norm(a[i]) == norm(b[j]) {
            flush(&mut del, &mut ins, &mut changes);
            i += 1;
            j += 1;
        } else if j < m && (i == n || dp[i][j + 1] >= dp[i + 1][j]) {
            ins.push(b[j]);
            j += 1;
        } else {
            del.push(a[i]);
            i += 1;
        }
    }
    flush(&mut del, &mut ins, &mut changes);
    changes
}

/// Distanza di Levenshtein a caratteri.
pub fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for i in 1..=a.len() {
        cur[0] = i;
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            cur[j] = (prev[j] + 1).min(cur[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

/// Ultimi `n` caratteri.
pub fn tail_chars(s: &str, n: usize) -> &str {
    let total = s.chars().count();
    if total <= n {
        return s;
    }
    let start = s.char_indices().nth(total - n).map(|(i, _)| i).unwrap_or(0);
    &s[start..]
}

/// Primi `n` caratteri.
pub fn head_chars(s: &str, n: usize) -> &str {
    let end = s.char_indices().nth(n).map(|(i, _)| i).unwrap_or(s.len());
    &s[..end]
}

/// Ancore attorno al testo incollato, ricavate dalla prima lettura del campo
/// dopo l'incolla.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchors {
    pub before: String,
    pub after: String,
    /// Posizione (in byte) dell'inizio del testo incollato nella lettura di base.
    pub origin: usize,
}

pub const ANCHOR_CHARS: usize = 40;

/// Cerca `pasted` nel testo letto; se compare più volte sceglie l'occorrenza
/// più vicina a `hint` (lunghezza del testo prima del cursore alla pressione).
pub fn find_anchors(text: &str, pasted: &str, hint_before: Option<&str>) -> Option<Anchors> {
    let needle = pasted.trim();
    if needle.is_empty() {
        return None;
    }
    let hint = hint_before.map(|h| h.len()).unwrap_or(0);
    let idx = text
        .match_indices(needle)
        .map(|(i, _)| i)
        .min_by_key(|i| (*i as i64 - hint as i64).abs())?;
    Some(Anchors {
        before: tail_chars(&text[..idx], ANCHOR_CHARS).to_string(),
        after: head_chars(&text[idx + needle.len()..], ANCHOR_CHARS).to_string(),
        origin: idx,
    })
}

/// Testo fra le ancore nella lettura corrente; `None` se un'ancora non c'è più.
pub fn region_between(text: &str, anchors: &Anchors) -> Option<String> {
    let start = if anchors.before.is_empty() {
        0
    } else {
        let len = anchors.before.len();
        text.match_indices(anchors.before.as_str())
            .map(|(i, _)| i + len)
            .min_by_key(|i| (*i as i64 - anchors.origin as i64).abs())?
    };
    let end = if anchors.after.is_empty() {
        text.len()
    } else {
        start + text[start..].find(anchors.after.as_str())?
    };
    Some(text[start..end].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn taglio_a_500_su_confine_di_parola() {
        let words: Vec<String> = (0..200).map(|i| format!("parola{i}")).collect();
        let s = words.join(" ");
        let tail = truncate_tail_on_word(&s, FIELD_PIECE_MAX_CHARS);
        assert!(tail.chars().count() <= FIELD_PIECE_MAX_CHARS);
        assert!(tail.starts_with("parola"), "inizia con una parola intera: {}", &tail[..20]);
        assert!(tail.ends_with("parola199"));
        let head = truncate_head_on_word(&s, FIELD_PIECE_MAX_CHARS);
        assert!(head.chars().count() <= FIELD_PIECE_MAX_CHARS);
        assert!(head.starts_with("parola0 "));
        let last = head.split_whitespace().last().unwrap();
        assert!(words.contains(&last.to_string()), "finisce con una parola intera");
        // Testo corto: invariato; caratteri multibyte senza panico.
        assert_eq!(truncate_tail_on_word("prova", 500), "prova");
        let accenti = "è ".repeat(400);
        assert!(truncate_head_on_word(&accenti, 500).chars().count() <= 500);
    }

    #[test]
    fn nomi_propri_euristica_locale() {
        let n = extract_proper_nouns(
            &[
                "Prova, testo con Nomeprova di SIGLA e del termine ParolaUno. Dopo arriva GPT4 su ChatGPT.",
                "Nuova chat - Claude",
            ],
            30,
        );
        for atteso in ["Nomeprova", "SIGLA", "ParolaUno", "GPT4", "ChatGPT", "Claude"] {
            assert!(n.contains(&atteso.to_string()), "manca {atteso} in {n:?}");
        }
        // "Prova" a inizio frase e "Dopo" dopo il punto non sono nomi propri.
        assert!(!n.contains(&"Prova".to_string()));
        assert!(!n.contains(&"Dopo".to_string()));
        assert!(extract_proper_nouns(&["Uno Due Tre Quattro"], 2).len() <= 2);
    }

    #[test]
    fn diff_a_parole_cambiate() {
        let c = word_changes(
            "parola uno Clod parola due Nome prova fine.",
            "parola uno Claude parola due NomeProva fine!",
        );
        assert_eq!(
            c,
            vec![
                WordChange { from: "Clod".into(), to: "Claude".into() },
                WordChange { from: "Nome prova".into(), to: "NomeProva".into() },
            ]
        );
        // Sola punteggiatura o inserimento puro: nessun cambio.
        assert!(word_changes("parola uno parola due", "parola uno, parola due?").is_empty());
        assert!(word_changes("parola due", "parola uno due").is_empty());
    }

    #[test]
    fn ancore_e_regione_corretta() {
        let base = "PROVA: parola uno Clod parola due FINE";
        let a = find_anchors(base, "parola uno Clod parola due", Some("PROVA: ")).unwrap();
        let edited = "PROVA: parola uno Claude parola due FINE";
        assert_eq!(region_between(edited, &a).unwrap(), "parola uno Claude parola due");
        assert!(region_between("tutto diverso", &a).is_none());
        assert_eq!(edit_distance("Clod", "Claude"), 3);
    }
}
