//! Prompt iniziale di Whisper costruito dal vocabolario.
//!
//! Forma scelta dopo le prove: una frase
//! italiana naturale che contiene i termini, i più importanti in fondo, con il
//! termine più importante ripetuto nella coda. Whisper copia lo stile del
//! prompt e guarda soprattutto la coda (massimo 224 token).

use std::collections::HashSet;

/// Stima dei token Whisper di un testo, pensata come limite superiore: per
/// ogni parola separata da spazi, un token ogni 2 caratteri (almeno uno), più
/// uno se la parola contiene punteggiatura o simboli. Tarata sul tokenizer vero
/// di whisper-large-v3-turbo: un prompt di dizionario di prova vale
/// 178 token veri, e su un vocabolario lungo la vecchia stima (un token ogni 3
/// caratteri) dava 224 dove i token veri erano 327 (rapporto 1,46). Questa
/// regola sta sopra entrambi, così whisper non taglia la testa del prompt.
pub fn estimate_tokens(text: &str) -> usize {
    text.split_whitespace()
        .map(|w| {
            let chars = w.chars().count();
            let symbols = usize::from(w.chars().any(|c| !c.is_alphanumeric()));
            chars.div_ceil(2).max(1) + symbols
        })
        .sum()
}

/// Termini puliti: spazi tolti, vuoti e doppioni (senza maiuscole) scartati,
/// ordine di importanza mantenuto.
fn dedupe(vocabulary: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    vocabulary
        .iter()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|t| !t.is_empty() && seen.insert(t.to_lowercase()))
        .collect()
}

/// Vocabolario della sessione con i termini aggiunti dopo l'apertura (nomi
/// propri dello schermo): restano dopo quelli già presenti, cioè meno
/// importanti, così nel prompt stanno prima del dizionario e sono i primi a
/// cadere quando si tocca il tetto. Doppioni (senza maiuscole) scartati.
pub fn merge_vocabulary(base: &[String], extra: &[String]) -> Vec<String> {
    let mut seen: HashSet<String> = base
        .iter()
        .map(|t| t.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase())
        .collect();
    let mut out = base.to_vec();
    for t in extra {
        let clean = t.split_whitespace().collect::<Vec<_>>().join(" ");
        if !clean.is_empty() && seen.insert(clean.to_lowercase()) {
            out.push(clean);
        }
    }
    out
}

/// La frase con i termini: `ordered[0]` è il più importante e chiude la frase,
/// ripetuto; gli altri lo precedono dal meno al più importante.
fn sentence(ordered: &[String]) -> String {
    match ordered {
        [] => String::new(),
        [top] => format!("Oggi parlo di {top}, sempre di {top}."),
        [top, rest @ ..] => {
            let others: Vec<&str> = rest.iter().rev().map(String::as_str).collect();
            format!(
                "Oggi parlo di {}, e poi di {top}, sempre di {top}.",
                others.join(", ")
            )
        }
    }
}

/// Coda di `text` che sta in `budget` token, tagliata a confine di parola.
fn tail_within(text: &str, budget: usize) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut used = 0;
    let mut start = words.len();
    while start > 0 {
        let cost = estimate_tokens(words[start - 1]);
        if used + cost > budget {
            break;
        }
        used += cost;
        start -= 1;
    }
    words[start..].join(" ")
}

/// Prompt per un pezzo. `vocabulary` arriva GIÀ ORDINATO dal chiamante: il
/// primo termine è il più importante e finisce in coda al prompt (ripetuto),
/// perché Whisper guarda soprattutto la coda. Qui non si riordina: entrano i
/// termini più importanti finché la frase sta in `max_tokens`; il testo
/// precedente va prima dei termini con lo spazio che resta, tenendone la coda.
pub fn build_prompt(vocabulary: &[String], previous_text: Option<&str>, max_tokens: usize) -> String {
    let terms = dedupe(vocabulary);
    let mut chosen: Vec<String> = Vec::new();
    for term in terms {
        chosen.push(term);
        if estimate_tokens(&sentence(&chosen)) > max_tokens {
            chosen.pop();
            // Un termine lungo può non entrare mentre uno corto dopo sì.
            continue;
        }
    }
    let terms_part = sentence(&chosen);
    let left = max_tokens.saturating_sub(estimate_tokens(&terms_part));
    let previous = previous_text
        .map(|p| tail_within(p, left))
        .unwrap_or_default();
    match (previous.is_empty(), terms_part.is_empty()) {
        (true, _) => terms_part,
        (false, true) => previous,
        (false, false) => format!("{previous} {terms_part}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn il_termine_piu_importante_chiude_il_prompt_ripetuto() {
        let p = build_prompt(&v(&["Termineprova", "NomeProva", "GitHub"]), None, 224);
        assert_eq!(
            p,
            "Oggi parlo di GitHub, NomeProva, e poi di Termineprova, sempre di Termineprova."
        );
        assert!(p.ends_with("sempre di Termineprova."));
        // Doppioni e vuoti spariscono.
        let q = build_prompt(&v(&["Termineprova", "termineprova", "  ", "Python"]), None, 224);
        assert_eq!(q, "Oggi parlo di Python, e poi di Termineprova, sempre di Termineprova.");
    }

    #[test]
    fn taglio_a_224_token_tiene_i_piu_importanti() {
        let many: Vec<String> = (0..300).map(|i| format!("Termine{i:03}")).collect();
        let p = build_prompt(&many, None, 224);
        assert!(estimate_tokens(&p) <= 224, "{} token", estimate_tokens(&p));
        assert!(p.contains("Termine000") && p.contains("Termine010"));
        assert!(!p.contains("Termine299"));
    }

    #[test]
    fn testo_precedente_prima_dei_termini_se_c_e_spazio() {
        let p = build_prompt(&v(&["Termineprova"]), Some("testo precedente di prova"), 224);
        assert_eq!(p, "testo precedente di prova Oggi parlo di Termineprova, sempre di Termineprova.");
        // Con poco spazio resta solo la coda del testo precedente.
        let terms = "Oggi parlo di Termineprova, sempre di Termineprova.";
        let budget = estimate_tokens(terms) + 2;
        let p = build_prompt(&v(&["Termineprova"]), Some("a b c d"), budget);
        assert_eq!(p, format!("c d {terms}"));
        // Senza vocabolario resta il testo precedente.
        assert_eq!(build_prompt(&[], Some("prova"), 224), "prova");
        assert_eq!(build_prompt(&[], None, 224), "");
    }
}
