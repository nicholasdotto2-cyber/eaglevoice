//! Uscite tipiche di Whisper su audio muto o corto.
//!
//! Il VAD acceso toglie il caso principale (senza VAD, sui silenzi Whisper
//! scrive spesso "Grazie."). Qui restano due difese:
//! - frasi dei sottotitoli e dei video che l'utente non detta mai, tolte
//!   ovunque compaiano;
//! - ripetizioni in loop ("e poi e poi e poi e poi"), ridotte a una copia.
//! "Grazie." non si toglie sempre: è anche una dettatura vera. Per la voce
//! bassa whisper-server decodifica senza il suo VAD interno, e su audio senza
//! voce inventa "Grazie." quasi sempre. Per questo una chiusura
//! di questo tipo in fondo al pezzo si toglie solo quando l'audio del pezzo
//! non la sostiene (`strip_unsupported_closing`).

use once_cell::sync::Lazy;
use regex::Regex;

static CREDITS: Lazy<Vec<Regex>> = Lazy::new(|| {
    [
        // "Sottotitoli a cura di QTSS", "Sottotitoli e revisione a cura di QTSS",
        // "Sottotitoli creati dalla comunità Amara.org"
        r"(?i)sottotitoli\s+(e\s+revisione\s+)?(a\s+cura\s+di\s+\S+|creati\s+dalla\s+comunit\S*\s+amara\.org)[.!]?",
        r"(?i)\bsubtitles\s+by\s+the\s+amara\.org\s+community[.!]?",
        r"(?i)\bamara\.org\b[.!]?",
        r"(?i)\bgrazie\s+per\s+la\s+visione[.!]?",
        r"(?i)\bthanks?\s+(you\s+)?for\s+watching[.!]?",
        r"(?i)\biscriviti\s+al\s+(mio\s+)?canale[.!]?",
        r"(?i)[\[(](musica|music|applausi|risate)[\])]",
        r"♪+",
    ]
    .iter()
    .map(|p| Regex::new(p).expect("regex dei filtri"))
    .collect()
});

fn norm(word: &str) -> String {
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Riduce a una copia una sequenza di 1-8 parole ripetuta di seguito: almeno
/// 4 volte per una parola sola, almeno 3 per le sequenze più lunghe.
fn collapse_repeats(words: Vec<&str>) -> Vec<&str> {
    let keys: Vec<String> = words.iter().map(|w| norm(w)).collect();
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    'outer: while i < words.len() {
        for n in 1..=8usize {
            let min_reps = if n == 1 { 4 } else { 3 };
            if i + n * min_reps > words.len() {
                continue;
            }
            if keys[i..i + n].iter().all(String::is_empty) {
                continue;
            }
            let mut reps = 1;
            while i + (reps + 1) * n <= words.len()
                && keys[i + reps * n..i + (reps + 1) * n] == keys[i..i + n]
            {
                reps += 1;
            }
            if reps >= min_reps {
                out.extend_from_slice(&words[i..i + n]);
                i += reps * n;
                continue 'outer;
            }
        }
        out.push(words[i]);
        i += 1;
    }
    out
}

/// Testo di un pezzo ripulito dalle allucinazioni note; vuoto se non resta
/// nulla di parlato.
pub fn filter_hallucinations(text: &str) -> String {
    let mut t = text.to_string();
    for re in CREDITS.iter() {
        t = re.replace_all(&t, " ").into_owned();
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    let joined = collapse_repeats(words).join(" ");
    if joined.chars().any(char::is_alphanumeric) {
        joined
    } else {
        String::new()
    }
}

/// Chiusure tipiche dei sottotitoli in fondo al testo di un pezzo. I crediti
/// ("Sottotitoli a cura di...", "Grazie per la visione") li toglie già
/// `filter_hallucinations` ovunque. Il gruppo 1 è il segno prima della
/// chiusura, che resta alla frase precedente.
static CLOSING_END: Lazy<Regex> = Lazy::new(|| {
    Regex::new(
        r"(?i)(^|[.,;:!?]?)\s*\bgrazie(\s+(a\s+tutti|mille|per\s+l['’]?\s*(ascolto|attenzione)))?\s*[.!]*\s*$",
    )
    .expect("regex delle chiusure")
});

/// Oltre questa coda senza voce l'ultima parola del pezzo non è parlato:
/// il VAD di cattura lascia al massimo 450 ms dopo la fine della voce.
pub const CLOSING_TAIL_MS: u32 = 800;
/// Pezzo "sotto la soglia di voce": meno di 600 ms di parlato e meno di un
/// quarto del pezzo. Un "Grazie." vero da solo, dopo il VAD di cattura,
/// occupa più di un quarto del pezzo.
pub const CLOSING_MIN_SPEECH_MS: u32 = 600;

/// Quanto parlato il VAD dei pezzi vede nel pezzo da cui viene il testo.
#[derive(Debug, Clone, Copy)]
pub struct PieceVoice {
    pub piece_ms: u32,
    pub speech_ms: u32,
    /// Coda del pezzo dopo la fine dell'ultimo parlato.
    pub tail_silence_ms: u32,
}

impl PieceVoice {
    fn supports_closing(&self) -> bool {
        let tail_voiced = self.tail_silence_ms <= CLOSING_TAIL_MS;
        let weak = self.speech_ms < CLOSING_MIN_SPEECH_MS && self.speech_ms * 4 < self.piece_ms;
        tail_voiced && !weak
    }
}

/// Toglie "Grazie." (e varianti) in fondo al testo di un pezzo quando
/// l'audio non lo sostiene: coda del pezzo senza voce, o pezzo intero sotto
/// la soglia di voce. Un "Grazie." con la voce in fondo resta.
pub fn strip_unsupported_closing(text: &str, voice: PieceVoice) -> String {
    if voice.supports_closing() {
        return text.to_string();
    }
    let Some(c) = CLOSING_END.captures(text) else {
        return text.to_string();
    };
    let whole = c.get(0).expect("match intero");
    let sep = c.get(1).map_or("", |g| g.as_str());
    let kept = format!("{}{sep}", text[..whole.start()].trim_end());
    if kept.chars().any(char::is_alphanumeric) {
        kept
    } else {
        String::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toglie_crediti_e_ripetizioni_ma_non_le_frasi_vere() {
        assert_eq!(filter_hallucinations("Sottotitoli a cura di QTSS"), "");
        assert_eq!(
            filter_hallucinations("Testo di prova uno. Sottotitoli creati dalla comunità Amara.org"),
            "Testo di prova uno."
        );
        assert_eq!(filter_hallucinations(" [Musica] "), "");
        assert_eq!(
            filter_hallucinations("parola uno due uno due uno due uno due fine"),
            "parola uno due fine"
        );
        assert_eq!(filter_hallucinations("prova prova prova prova prova prova"), "prova");
        assert_eq!(filter_hallucinations("Grazie."), "Grazie.");
        assert_eq!(
            filter_hallucinations("Testo di prova con parole normali."),
            "Testo di prova con parole normali."
        );
        // Due ripetizioni sono lingua normale.
        assert_eq!(filter_hallucinations("prova prova, fine"), "prova prova, fine");
    }
}
