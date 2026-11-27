//! Dizionario che impara dalle correzioni a mano dopo l'incolla.
//!
//! Nome `learn`: è il passo che trasforma un'osservazione di `win\observe` in voci del
//! dizionario; il vecchio `Dictionary::observe_correction` (candidati dopo 2 viste) resta per
//! compatibilità ma non è più la strada principale.
//!
//! Catena:
//! 1. porte d'ingresso: niente apprendimento con ancore perse, testo uguale, testo troppo lungo,
//!    campo svuotato senza istantanea, cambio oltre il 50% delle parole (riscrittura);
//! 2. allineamento: via prefisso e suffisso comuni, poi programmazione dinamica sulle parole con
//!    costo di sostituzione pari alla distanza normalizzata sui caratteri;
//! 3. gruppi di modifiche consecutive; scartati se un lato è vuoto o il corretto supera 4 parole;
//! 4. filtri locali: parole comuni e stopword, termini già nel dizionario, contrazioni, numeri,
//!    forme che non si somigliano; al massimo 4 candidati, prima quelli con maiuscole;
//! 5. classificatore con il Qwen locale (uscita JSON vincolata e validata, tetto 1,5 s);
//! 6. voce `user_edits` subito nel dizionario; sostituzione esatta solo se la forma sbagliata non
//!    è una parola comune né un termine del dizionario, altrimenti solo vocabolario.

use serde::{Deserialize, Serialize};
use specta::Type;
use std::time::Duration;

use super::dictionary::{is_common_word, is_stopword, normalized_distance, Dictionary, LearnOutcome};
use super::postprocess::tokens;

/// Parole oltre le quali non si impara (testo troppo lungo per una correzione puntuale).
pub const MAX_WORDS: usize = 600;
/// Parole al massimo nel lato corretto di un gruppo.
pub const MAX_GROUP_WORDS: usize = 4;
/// Candidati al massimo passati al classificatore.
pub const MAX_CANDIDATES: usize = 4;
/// Quota massima di parole cambiate; sopra è una riscrittura.
pub const MAX_CHANGED_RATIO: f64 = 0.5;
/// Le dettature corte contano come se avessero almeno 4 parole: "cloud code" → "Claude Code"
/// cambia 2 parole su 2 ed è una correzione, non una riscrittura.
pub const MIN_RATIO_BASE: usize = 4;
/// Distanza normalizzata massima fra forma sbagliata e corretta: le correzioni vere restano
/// ben sotto ("codecs" → "CODEX"), due parole diverse stanno molto sopra.
pub const MAX_PAIR_DISTANCE: f64 = 0.6;
/// Tetto del classificatore locale.
pub const CLASSIFIER_TIMEOUT: Duration = Duration::from_millis(1500);

/// Osservazione di una correzione, indipendente da `win\`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, Default, PartialEq)]
pub struct Observation {
    /// Testo incollato dall'app.
    pub pasted: String,
    /// Testo dopo le correzioni (l'ultima istantanea prima di uno svuotamento, se c'è stato).
    pub edited: String,
    /// Motivo di fine osservazione (`next_paste_started`, `observation_window_elapsed`,
    /// `textbox_emptied`, `anchor_mismatch`, `edit_distance_exceeded`, ...).
    pub ended_by: String,
    /// Le ancore (testo prima e dopo l'incollato) sono rimaste intere.
    pub anchors_intact: bool,
    pub app_name: String,
}

impl Observation {
    /// Adattatore dal payload di `win:text-corrected` di oggi, che non ha ancora il campo delle
    /// ancore: le si considera perse quando l'osservazione è finita su un motivo che lo dice.
    pub fn from_text_corrected(t: &crate::win::TextCorrected) -> Self {
        let lost = matches!(t.ended_by.as_str(), "anchor_mismatch" | "pasted_text_not_found" | "field_unreadable");
        Observation {
            pasted: t.pasted.clone(),
            edited: t.edited.clone(),
            ended_by: t.ended_by.clone(),
            anchors_intact: !lost,
            app_name: t.app_name.clone(),
        }
    }
}

/// Perché un'osservazione non insegna nulla (per i log e i test).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Skip {
    AnchorsLost,
    Unchanged,
    TooLong,
    Emptied,
    Rewrite,
    NoCandidates,
    ClassifierUnavailable,
    ClassifierRejected,
}

/// Coppia candidata.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub wrong: String,
    pub right: String,
    /// Frase del testo corretto intorno alla coppia, per il classificatore.
    pub context: String,
}

/// Gruppo di modifiche consecutive: parole tolte dall'incollato e parole messe nel corretto.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub wrong: Vec<String>,
    pub right: Vec<String>,
    /// Indice della prima parola del gruppo nel corretto.
    pub at: usize,
}

/// Costo di sostituzione fra due parole: 0 se uguali, poco se cambiano solo le maiuscole,
/// altrimenti la distanza normalizzata sui caratteri (mai più di inserire più cancellare).
fn sub_cost(a: &str, b: &str) -> f64 {
    if a == b {
        0.0
    } else if a.to_lowercase() == b.to_lowercase() {
        0.05
    } else {
        normalized_distance(a, b).max(0.1)
    }
}

/// Allinea due liste di parole e restituisce i gruppi di modifiche consecutive (lati vuoti
/// compresi: il filtro sta in `candidate_groups`).
pub fn align(a: &[String], b: &[String]) -> Vec<Group> {
    let (pre, ops, ma, mb) = align_ops(a, b);
    let mut groups = Vec::new();
    let mut cur: Option<Group> = None;
    let mut next_b = 0; // parole del corretto già viste, per l'indice del gruppo
    for (oi, oj) in ops {
        let same = matches!((oi, oj), (Some(x), Some(y)) if ma[x] == mb[y]);
        if same {
            if let Some(g) = cur.take() {
                groups.push(g);
            }
        } else {
            let g = cur.get_or_insert(Group { wrong: Vec::new(), right: Vec::new(), at: pre + next_b });
            if let Some(x) = oi {
                g.wrong.push(ma[x].clone());
            }
            if let Some(y) = oj {
                g.right.push(mb[y].clone());
            }
        }
        if oj.is_some() {
            next_b += 1;
        }
    }
    if let Some(g) = cur.take() {
        groups.push(g);
    }
    groups
}

/// Coppie di parole allineate (indici dentro `ma` e `mb`, `None` per un lato vuoto), dopo il
/// prefisso e il suffisso comuni. Restituisce (lunghezza del prefisso, coppie, `ma`, `mb`).
#[allow(clippy::type_complexity)]
fn align_ops<'a>(a: &'a [String], b: &'a [String]) -> (usize, Vec<(Option<usize>, Option<usize>)>, &'a [String], &'a [String]) {
    // prefisso e suffisso comuni fuori dalla programmazione dinamica
    let mut pre = 0;
    while pre < a.len() && pre < b.len() && a[pre] == b[pre] {
        pre += 1;
    }
    let mut suf = 0;
    while suf < a.len() - pre && suf < b.len() - pre && a[a.len() - 1 - suf] == b[b.len() - 1 - suf] {
        suf += 1;
    }
    let (ma, mb) = (&a[pre..a.len() - suf], &b[pre..b.len() - suf]);
    let (n, m) = (ma.len(), mb.len());
    let w = m + 1;
    let mut dp = vec![0f64; (n + 1) * w];
    for i in 0..=n {
        dp[i * w] = i as f64;
    }
    for j in 0..=m {
        dp[j] = j as f64;
    }
    for i in 1..=n {
        for j in 1..=m {
            let s = dp[(i - 1) * w + j - 1] + sub_cost(&ma[i - 1], &mb[j - 1]);
            let d = dp[(i - 1) * w + j] + 1.0;
            let ins = dp[i * w + j - 1] + 1.0;
            dp[i * w + j] = s.min(d).min(ins);
        }
    }
    // percorso all'indietro: (parola dell'incollato, parola del corretto), None per un lato vuoto
    let mut ops: Vec<(Option<usize>, Option<usize>)> = Vec::new();
    let (mut i, mut j) = (n, m);
    while i > 0 || j > 0 {
        let cur = dp[i * w + j];
        if i > 0 && j > 0 && (cur - (dp[(i - 1) * w + j - 1] + sub_cost(&ma[i - 1], &mb[j - 1]))).abs() < 1e-9 {
            ops.push((Some(i - 1), Some(j - 1)));
            i -= 1;
            j -= 1;
        } else if i > 0 && (cur - (dp[(i - 1) * w + j] + 1.0)).abs() < 1e-9 {
            ops.push((Some(i - 1), None));
            i -= 1;
        } else {
            ops.push((None, Some(j - 1)));
            j -= 1;
        }
    }
    ops.reverse();
    (pre, ops, ma, mb)
}

/// Riempitivi che una correzione può togliere o aggiungere dentro una coppia senza riscriverla.
const PAIR_FILLERS: &[&str] = &["ehm", "uhm", "eh", "mmh", "tipo", "cioè", "appunto", "diciamo"];

/// Dentro la coppia, parole aggiunte o tolte che non sono riempitivi né il pezzo di una parola
/// unita o divisa ("git hub" → "GitHub" sì; "nomeprova per il sito" → "NOMEPROVA PER SITO nuovo" no:
/// "il" tolto, "nuovo" aggiunto). Restituisce anche la quota di parole cambiate davvero (le sole
/// maiuscole non contano), per il controllo di riscrittura della migrazione.
fn pair_word_changes(wrong: &str, right: &str) -> (bool, f64) {
    let (ww, rw) = (tokens(wrong), tokens(right));
    let (_, ops, ma, mb) = align_ops(&ww, &rw);
    let (lw, lr) = (letters(wrong), letters(right));
    let mut extra = false;
    let mut changed = 0usize;
    for (oi, oj) in ops {
        match (oi, oj) {
            (Some(x), Some(y)) => {
                let (a, b) = (ma[x].to_lowercase(), mb[y].to_lowercase());
                if a != b {
                    changed += 1;
                    // una parola al posto di un'altra che non le somiglia ("il" contro "RAG"):
                    // l'allineamento la preferisce a una tolta più una aggiunta
                    let (la, lb) = (letters(&a), letters(&b));
                    if normalized_distance(&a, &b) > MAX_PAIR_DISTANCE && !la.contains(&lb) && !lb.contains(&la) {
                        extra = true;
                    }
                }
            }
            (Some(x), None) => {
                changed += 1;
                let w = ma[x].to_lowercase();
                if !PAIR_FILLERS.contains(&w.as_str()) && !lr.contains(&letters(&w)) {
                    extra = true;
                }
            }
            (None, Some(y)) => {
                changed += 1;
                let w = mb[y].to_lowercase();
                if !PAIR_FILLERS.contains(&w.as_str()) && !lw.contains(&letters(&w)) {
                    extra = true;
                }
            }
            (None, None) => {}
        }
    }
    (extra, changed as f64 / ww.len().max(MIN_RATIO_BASE) as f64)
}

/// Porte d'ingresso: `Some(motivo)` se l'osservazione non deve insegnare nulla.
pub fn gate(obs: &Observation) -> Option<Skip> {
    if !obs.anchors_intact || obs.ended_by == "anchor_mismatch" {
        return Some(Skip::AnchorsLost);
    }
    if obs.edited.trim().is_empty() {
        // campo svuotato senza istantanea, o cancellazione totale
        return Some(Skip::Emptied);
    }
    if obs.pasted.trim() == obs.edited.trim() {
        return Some(Skip::Unchanged);
    }
    if obs.ended_by == "edit_distance_exceeded" {
        return Some(Skip::Rewrite);
    }
    let (a, b) = (tokens(&obs.pasted), tokens(&obs.edited));
    if a.len() > MAX_WORDS || b.len() > MAX_WORDS {
        return Some(Skip::TooLong);
    }
    if a.is_empty() || b.is_empty() {
        return Some(Skip::Emptied);
    }
    let changed: usize = align(&a, &b).iter().map(|g| g.wrong.len().max(g.right.len())).sum();
    if changed == 0 {
        return Some(Skip::Unchanged);
    }
    if changed as f64 / a.len().max(MIN_RATIO_BASE) as f64 > MAX_CHANGED_RATIO {
        return Some(Skip::Rewrite);
    }
    None
}

fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).collect::<String>().to_lowercase()
}

/// Filtri locali su una coppia; `Err(motivo)` se va scartata.
pub fn pair_filter(wrong: &str, right: &str, dict: &Dictionary) -> Result<(), &'static str> {
    let (rw, ww) = (tokens(right), tokens(wrong));
    if rw.is_empty() || ww.is_empty() {
        return Err("lato vuoto");
    }
    if rw.len() > MAX_GROUP_WORDS || ww.len() > MAX_GROUP_WORDS + 2 {
        return Err("troppe parole");
    }
    // stopword: "il" → "lo", "e" → "è"
    if rw.iter().all(|w| is_stopword(w)) {
        return Err("stopword");
    }
    // da parola comune a parola comune è un cambio di contenuto o di grammatica ("martedì" →
    // "giovedì", "faremo" → "facciamo", "lunedì" → "Lunedì"); da una non-parola a una parola
    // comune è un errore di trascrizione ("fenestra" → "finestra")
    if rw.iter().all(|w| is_common_word(w)) && ww.iter().all(|w| is_common_word(w)) {
        return Err("parole comuni");
    }
    let (lw, lr) = (letters(wrong), letters(right));
    // contrazioni e spazi: "po" → "po'", "un altro" → "un'altro"
    if lw == lr && wrong.to_lowercase() != right.to_lowercase() && !right.chars().any(|c| c.is_uppercase()) {
        return Err("contrazione");
    }
    if lr.chars().count() < 2 || !right.chars().any(|c| c.is_alphabetic()) {
        return Err("troppo corta");
    }
    // istantanea presa a metà di una modifica: il corretto è un pezzo del sbagliato
    // ("dirmelo" → "dir", "tutte" → "utte")
    if lr.len() < lw.len() && (lw.starts_with(&lr) || lw.ends_with(&lr)) {
        return Err("parola troncata");
    }
    // e il caso inverso: la forma giusta è l'inizio, o quasi, della parola del grezzo, più corta
    // o uguale, e non è una parola nota ("Nomeprovario" → "nomeprovica", presa a metà battitura di
    // "nomeprovicale"). Le correzioni verso parole comuni ("mercatto" → "mercato") restano.
    let common_prefix = lw.chars().zip(lr.chars()).take_while(|(a, b)| a == b).count();
    let lr_n = lr.chars().count();
    if lr != lw
        && lr_n <= lw.chars().count()
        && common_prefix >= 4
        && common_prefix as f64 >= 0.6 * lr_n as f64
        && !right.chars().any(|c| c.is_uppercase())
        && !rw.iter().all(|w| is_common_word(w))
        && !dict.has_term(right)
    {
        return Err("parola troncata");
    }
    // parole aggiunte o tolte dentro la coppia: è una riscrittura, non una grafia
    if pair_word_changes(wrong, right).0 {
        return Err("parole aggiunte o tolte");
    }
    // cambiano solo le cifre ("G8" → "G87"): è un dato, non una grafia
    let only_letters = |s: &str| s.chars().filter(|c| c.is_alphabetic()).collect::<String>().to_lowercase();
    if lw != lr && only_letters(wrong) == only_letters(right) && lr.chars().any(|c| c.is_ascii_digit()) {
        return Err("solo cifre");
    }
    // maiuscole storte a metà parola, da tasto bloccato o istantanea a metà ("variaBILI");
    // le forme volute partono maiuscole ("GitHub", "ElevenLabs")
    let odd = |w: &String| w.chars().next().is_some_and(|c| c.is_lowercase()) && w.chars().filter(|c| c.is_uppercase()).count() >= 2;
    if rw.iter().any(odd) {
        return Err("maiuscole storte");
    }
    if dict.has_term(right) && dict.learned_state(wrong, right).is_none() {
        return Err("già nel dizionario");
    }
    if normalized_distance(&lw, &lr) > MAX_PAIR_DISTANCE {
        return Err("forme lontane");
    }
    Ok(())
}

/// Filtri locali sui gruppi; candidati ordinati con quelli in maiuscolo prima, al massimo 4.
pub fn candidate_groups(groups: &[Group], edited_tokens: &[String], dict: &Dictionary) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for g in groups {
        if g.wrong.is_empty() || g.right.is_empty() || g.right.len() > MAX_GROUP_WORDS || g.wrong.len() > MAX_GROUP_WORDS + 2 {
            continue;
        }
        let (wrong, right) = (g.wrong.join(" "), g.right.join(" "));
        if pair_filter(&wrong, &right, dict).is_err() {
            continue;
        }
        if out.iter().any(|c| c.wrong == wrong && c.right == right) {
            continue;
        }
        let from = g.at.saturating_sub(6);
        let to = (g.at + g.right.len() + 6).min(edited_tokens.len());
        out.push(Candidate { wrong, right, context: edited_tokens[from..to].join(" ") });
    }
    out.sort_by_key(|c| !c.right.chars().any(|ch| ch.is_uppercase()));
    out.truncate(MAX_CANDIDATES);
    out
}

/// Messaggio utente del classificatore.
pub fn classifier_user(template: &str, cands: &[Candidate]) -> String {
    let lines: Vec<String> = cands
        .iter()
        .enumerate()
        .map(|(i, c)| format!("{}. \"{}\" → \"{}\" in \"{}\"", i + 1, c.wrong, c.right, c.context))
        .collect();
    template.replace("{{coppie}}", &lines.join("\n"))
}

/// Schema JSON dell'uscita: esattamente `n` booleani in `decisioni`.
pub fn classifier_schema(n: usize) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": { "decisioni": { "type": "array", "items": { "type": "boolean" }, "minItems": n, "maxItems": n } },
        "required": ["decisioni"],
        "additionalProperties": false
    })
}

/// Validazione rigida: un oggetto con la sola chiave `decisioni`, esattamente `n` booleani.
/// Qualsiasi altra forma vale "nessuna decisione" (`None`), quindi nessun apprendimento.
pub fn parse_decisions(raw: &str, n: usize) -> Option<Vec<bool>> {
    let v: serde_json::Value = serde_json::from_str(raw.trim()).ok()?;
    let obj = v.as_object()?;
    if obj.len() != 1 {
        return None;
    }
    let arr = obj.get("decisioni")?.as_array()?;
    if arr.len() != n {
        return None;
    }
    arr.iter().map(|x| x.as_bool()).collect()
}

/// La coppia può diventare sostituzione esatta? Solo se la forma sbagliata non è una parola
/// comune (né fatta solo di parole comuni) e non è un termine del dizionario: "nomi" →
/// "Nomeprova" no se "nomi" fosse comune; "cloud" no se il dizionario ha già "cloud".
pub fn replacement_allowed(wrong: &str, dict: &Dictionary) -> bool {
    let ws = tokens(wrong);
    !ws.is_empty() && !ws.iter().all(|w| is_common_word(w)) && !dict.has_term(wrong)
}

/// Esito di `learn` per una parola.
#[derive(Debug, Clone, PartialEq)]
pub struct Learned {
    pub id: String,
    pub wrong: String,
    pub right: String,
    /// Vero se la voce è nuova (si mostra l'avviso con "Annulla"); falso se è una ricomparsa.
    pub new: bool,
}

/// Passi locali (porte, allineamento, filtri): candidati da classificare o motivo dello scarto.
pub fn prepare(obs: &Observation, dict: &Dictionary) -> Result<Vec<Candidate>, Skip> {
    if let Some(s) = gate(obs) {
        return Err(s);
    }
    let (a, b) = (tokens(&obs.pasted), tokens(&obs.edited));
    let c = candidate_groups(&align(&a, &b), &b, dict);
    if c.is_empty() {
        Err(Skip::NoCandidates)
    } else {
        Ok(c)
    }
}

/// Migrazione dei candidati `pending` del vecchio meccanismo: ognuno passa di nuovo dai filtri
/// locali (non si aggiungono in blocco). Restituisce (da classificare, scartati con il motivo).
pub fn triage_pending(pending: &[(String, String, String, String)], dict: &Dictionary) -> (Vec<(String, Candidate)>, Vec<(String, &'static str)>) {
    let (mut keep, mut drop) = (Vec::new(), Vec::new());
    for (id, wrong, right, example) in pending {
        // equivalente di `gate` sulla coppia: uguale, o riscritta per più della metà delle parole
        let gated = if wrong.trim() == right.trim() {
            Err("uguale")
        } else if pair_word_changes(wrong, right).1 > MAX_CHANGED_RATIO {
            Err("riscrittura")
        } else {
            Ok(())
        };
        match gated.and_then(|_| pair_filter(wrong, right, dict)) {
            Ok(()) => keep.push((id.clone(), Candidate { wrong: wrong.clone(), right: right.clone(), context: example.clone() })),
            Err(why) => drop.push((id.clone(), why)),
        }
    }
    (keep, drop)
}

/// Scrive nel dizionario i candidati accettati.
pub fn commit(cands: &[Candidate], decisions: &[bool], dict: &Dictionary) -> Vec<Learned> {
    let mut out = Vec::new();
    for (c, ok) in cands.iter().zip(decisions) {
        if !ok {
            continue;
        }
        let as_repl = replacement_allowed(&c.wrong, dict) || dict.learned_state(&c.wrong, &c.right).is_some();
        match dict.learn_pair(&c.wrong, &c.right, as_repl) {
            Ok(LearnOutcome::Added(id)) => out.push(Learned { id, wrong: c.wrong.clone(), right: c.right.clone(), new: true }),
            Ok(LearnOutcome::Seen(id)) => out.push(Learned { id, wrong: c.wrong.clone(), right: c.right.clone(), new: false }),
            Ok(LearnOutcome::Refused) => {}
            Err(e) => log::warn!("dizionario: parola imparata non salvata: {e}"),
        }
    }
    out
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn obs(p: &str, e: &str) -> Observation {
        Observation { pasted: p.into(), edited: e.into(), ended_by: "next_paste_started".into(), anchors_intact: true, app_name: "prova".into() }
    }

    /// 10 correzioni di esempio (frasi e termini inventati).
    pub(crate) const CORREZIONI: &[(&str, &str, &str, &str)] = &[
        ("Esempio uno con cloud code nel testo.", "Esempio uno con Claude Code nel testo.", "cloud code", "Claude Code"),
        ("Esempio due con git hub nel testo.", "Esempio due con GitHub nel testo.", "git hub", "GitHub"),
        ("Esempio tre con codecs nel testo.", "Esempio tre con CODEX nel testo.", "codecs", "CODEX"),
        ("Esempio quattro con nomeuno nel testo.", "Esempio quattro con NomeUno nel testo.", "nomeuno", "NomeUno"),
        ("Esempio cinque con python nel testo.", "Esempio cinque con Python nel testo.", "python", "Python"),
        ("Esempio sei con demo lab nel testo.", "Esempio sei con DEMOLAB nel testo.", "demo lab", "DEMOLAB"),
        ("Esempio sette con eleven labs nel testo.", "Esempio sette con ElevenLabs nel testo.", "eleven labs", "ElevenLabs"),
        ("Esempio otto con noshon nel testo.", "Esempio otto con Notion nel testo.", "noshon", "Notion"),
        ("Esempio nove con cloud.md nel testo.", "Esempio nove con claude.md nel testo.", "cloud.md", "claude.md"),
        ("Esempio dieci con termineprova nel testo.", "Esempio dieci con TERMINEPROVA nel testo.", "termineprova", "TERMINEPROVA"),
    ];

    /// 5 modifiche non correttive.
    pub(crate) const NON_CORRETTIVE: &[(&str, &str)] = &[
        // riscrittura di una frase
        ("Testo di prova numero uno con parole fisse.", "Frase diversa scritta da capo, senza nulla in comune."),
        // cancellazione totale
        ("Testo di prova da cancellare.", ""),
        // cambio di contenuto
        ("Prova con la parola martedì nel testo.", "Prova con la parola giovedì nel testo."),
        // grammatica e stile su parole comuni
        ("Prova con la parola faremo nel testo.", "Prova con la parola facciamo nel testo."),
        // persona diversa
        ("Prova con il nome Alfa nel testo.", "Prova con il nome Zeta nel testo."),
    ];

    #[test]
    fn allineamento_raggruppa_le_parole_consecutive() {
        let g = align(&tokens("parola cloud code parola codecs"), &tokens("parola Claude Code parola CODEX"));
        assert_eq!(g.len(), 2);
        assert_eq!((g[0].wrong.join(" "), g[0].right.join(" ")), ("cloud code".into(), "Claude Code".into()));
        assert_eq!((g[1].wrong.join(" "), g[1].right.join(" ")), ("codecs".into(), "CODEX".into()));
    }

    #[test]
    fn dieci_correzioni_vere_diventano_candidati() {
        let d = Dictionary::open_in_memory().unwrap();
        for (p, e, w, r) in CORREZIONI {
            let c = prepare(&obs(p, e), &d).unwrap_or_else(|s| panic!("{p}: {s:?}"));
            assert_eq!(c.len(), 1, "{p}: {c:?}");
            assert_eq!((c[0].wrong.as_str(), c[0].right.as_str()), (*w, *r));
        }
    }

    #[test]
    fn cinque_modifiche_non_correttive_scartate_dai_filtri() {
        let d = Dictionary::open_in_memory().unwrap();
        let got: Vec<_> = NON_CORRETTIVE.iter().map(|(p, e)| prepare(&obs(p, e), &d).err()).collect();
        assert_eq!(got, vec![
            Some(Skip::Rewrite),
            Some(Skip::Emptied),
            Some(Skip::NoCandidates), // giovedì è una parola comune
            Some(Skip::NoCandidates),
            Some(Skip::NoCandidates), // Alfa → Zeta: forme lontane
        ]);
    }

    #[test]
    fn ancore_perse_e_testo_uguale_non_insegnano() {
        let d = Dictionary::open_in_memory().unwrap();
        let mut o = obs("Esempio con cloud code.", "Esempio con Claude Code.");
        o.anchors_intact = false;
        assert_eq!(prepare(&o, &d), Err(Skip::AnchorsLost));
        assert_eq!(prepare(&obs("Testo di prova.", "Testo di prova."), &d), Err(Skip::Unchanged));
        let lungo = "parola ".repeat(MAX_WORDS + 1);
        assert_eq!(prepare(&obs(&lungo, &format!("{lungo} Claude")), &d), Err(Skip::TooLong));
    }

    #[test]
    fn filtri_stopword_dizionario_e_contrazioni() {
        let d = Dictionary::open_in_memory().unwrap();
        // contrazione
        assert_eq!(prepare(&obs("Testo di prova con un po di parole.", "Testo di prova con un po' di parole."), &d), Err(Skip::Unchanged));
        // maiuscola di una parola comune
        assert_eq!(prepare(&obs("testo di prova con lunedì in mezzo", "testo di prova con Lunedì in mezzo"), &d), Err(Skip::NoCandidates));
        // termine già nel dizionario
        d.learn_pair("git hub", "GitHub", true).unwrap();
        d.add(super::super::DictionaryEntry {
            id: String::new(), phrase: "Notion".into(), replacement: None, is_snippet: false, starred: false, frequency: 0,
            last_used: None, source: super::super::DictionarySource::Manual, ambiguous: false,
        }).unwrap();
        assert_eq!(prepare(&obs("Esempio con noshon.", "Esempio con Notion."), &d), Err(Skip::NoCandidates));
    }

    #[test]
    fn migrazione_rivaluta_i_vecchi_candidati() {
        let d = Dictionary::open_in_memory().unwrap();
        let p = |id: &str, w: &str, r: &str| (id.to_string(), w.to_string(), r.to_string(), String::new());
        let pending = vec![
            p("a", "mercatto", "mercato"),                      // non-parola → parola: si classifica
            p("b", "Noshon", "notion"),                         // termine: si classifica
            p("c", "scrivania", "scriv"),                       // istantanea a metà modifica
            p("d", "Parole del testo vecchio", "Altro testo"),    // riscrittura
            p("e", "settimana", "settimane"),                   // parole comuni
            p("f", "X5", "X57"),                                // cambia solo il numero
            p("g", "sistemma", "sisTEMA"),                      // maiuscole storte
        ];
        let (keep, drop) = triage_pending(&pending, &d);
        assert_eq!(keep.iter().map(|(id, _)| id.as_str()).collect::<Vec<_>>(), vec!["a", "b"]);
        assert_eq!(drop, vec![("c".to_string(), "parola troncata"), ("d".to_string(), "riscrittura"), ("e".to_string(), "parole comuni"), ("f".to_string(), "solo cifre"), ("g".to_string(), "maiuscole storte")]);
    }

    // I due tipi di caso della migrazione (dati inventati)
    #[test]
    fn migrazione_scarta_riscrittura_e_forma_a_meta_battitura() {
        let d = Dictionary::open_in_memory().unwrap();
        let p = |id: &str, w: &str, r: &str| (id.to_string(), w.to_string(), r.to_string(), String::new());
        let pending = vec![p("q", "Nomeprovva per il testo", "NOMEPROVA PER TESTO nuovo"), p("x", "Nomeprovario", "nomeprovica")];
        let (keep, drop) = triage_pending(&pending, &d);
        assert!(keep.is_empty(), "{keep:?}");
        assert_eq!(drop, vec![("q".to_string(), "riscrittura"), ("x".to_string(), "parola troncata")]);
        // stessi filtri anche nel percorso dal vivo, dove la quota la controlla già `gate`
        assert_eq!(pair_filter("Nomeprovva per il testo", "NOMEPROVA PER TESTO nuovo", &d), Err("parole aggiunte o tolte"));
        assert_eq!(pair_filter("Nomeprovario", "nomeprovica", &d), Err("parola troncata"));
        // le unioni restano: "git hub" → "GitHub", "demo lab" → "DEMOLAB"
        assert_eq!(pair_filter("git hub", "GitHub", &d), Ok(()));
        assert_eq!(pair_filter("demo lab", "DEMOLAB", &d), Ok(()));
    }

    #[test]
    fn validazione_rigida_del_classificatore() {
        assert_eq!(parse_decisions(r#"{"decisioni": [true, false]}"#, 2), Some(vec![true, false]));
        assert_eq!(parse_decisions(r#"{"decisioni": [true]}"#, 2), None);
        assert_eq!(parse_decisions(r#"{"decisioni": ["si", false]}"#, 2), None);
        assert_eq!(parse_decisions(r#"{"decisioni": [true], "nota": "x"}"#, 1), None);
        assert_eq!(parse_decisions("Sì, la prima è giusta", 1), None);
    }

    #[test]
    fn annulla_toglie_la_voce_e_la_coppia_non_si_reimpara() {
        let d = Dictionary::open_in_memory().unwrap();
        let c = prepare(&obs(CORREZIONI[0].0, CORREZIONI[0].1), &d).unwrap();
        let l = commit(&c, &[true], &d);
        assert_eq!(l.len(), 1);
        assert!(l[0].new);
        assert!(d.vocabulary().unwrap().contains(&"Claude Code".to_string()));
        let w = d.learned_words().unwrap();
        assert_eq!((w[0].wrong.as_str(), w[0].right.as_str(), w[0].count, w[0].active), ("cloud code", "Claude Code", 1, true));
        // ricomparsa: conteggio più uno, nessuna voce nuova
        let again = commit(&c, &[true], &d);
        assert!(!again[0].new);
        assert_eq!(d.learned_words().unwrap()[0].count, 2);
        assert_eq!(d.list().unwrap().len(), 1);
        // Annulla: la voce sparisce dal dizionario e dal vocabolario dell'ASR
        d.undo_learned(&l[0].id).unwrap();
        assert!(d.list().unwrap().is_empty());
        assert!(d.vocabulary().unwrap().is_empty());
        assert!(!d.learned_words().unwrap()[0].active);
        assert!(commit(&c, &[true], &d).is_empty());
        // Elimina: la coppia sparisce del tutto
        d.delete_learned(&l[0].id).unwrap();
        assert!(d.learned_words().unwrap().is_empty());
    }

    #[test]
    fn sostituzione_imparata_a_confini_di_parola_non_tocca_lumino() {
        let d = Dictionary::open_in_memory().unwrap();
        let c = vec![Candidate { wrong: "termi".into(), right: "Termineprova".into(), context: String::new() }];
        assert!(replacement_allowed("termi", &d));
        commit(&c, &[true], &d);
        let rules = super::super::postprocess::Rules::from_entries(&d.list().unwrap());
        let out = rules.apply_replacements("Prova terminale, poi termi e di nuovo terminale.");
        assert_eq!(out.text, "Prova terminale, poi Termineprova e di nuovo terminale.");
    }

    #[test]
    fn forma_sbagliata_comune_resta_solo_vocabolario() {
        let d = Dictionary::open_in_memory().unwrap();
        assert!(!replacement_allowed("casa", &d));
        commit(&[Candidate { wrong: "casa".into(), right: "KASA".into(), context: String::new() }], &[true], &d);
        let e = &d.list().unwrap()[0];
        assert_eq!((e.phrase.as_str(), e.replacement.as_deref()), ("KASA", None));
        let rules = super::super::postprocess::Rules::from_entries(&d.list().unwrap());
        assert_eq!(rules.apply_replacements("parola casa parola").text, "parola casa parola");
    }

    // La sostituzione imparata vale solo per la forma esatta (più l'iniziale maiuscola
    // a inizio frase); le voci a mano restano senza maiuscole
    #[test]
    fn sostituzione_imparata_solo_forma_esatta() {
        let d = Dictionary::open_in_memory().unwrap();
        commit(&[Candidate { wrong: "termineprova".into(), right: "TERMINEPROVA".into(), context: String::new() }], &[true], &d);
        let rules = super::super::postprocess::Rules::from_entries(&d.list().unwrap());
        assert_eq!(
            rules.apply_replacements("uno termineprova, poi Termineprova e Termineprova Due. Termineprova fine.").text,
            "uno TERMINEPROVA, poi Termineprova e Termineprova Due. TERMINEPROVA fine."
        );
    }

    // Una forma sbagliata che l'utente scrive lui nel testo corretto diventa solo
    // vocabolario; le altre voci attive continuano a sostituire
    #[test]
    fn forma_tenuta_dall_utente_non_si_sostituisce_e_voci_vere_restano() {
        let d = Dictionary::open_in_memory().unwrap();
        let c = |w: &str, r: &str| Candidate { wrong: w.into(), right: r.into(), context: String::new() };
        commit(&[c("cloud.ai", "CLAUDE.ai"), c("pesanty", "pesanti"), c("fenestra", "finestra"), c("demo lab", "DEMOLAB")], &[true; 4], &d);
        let rules = super::super::postprocess::Rules::from_entries(&d.list().unwrap());
        assert_eq!(
            rules.apply_replacements("prova cloud.ai, parola pesanty, parola fenestra, parola demo lab").text,
            "prova CLAUDE.ai, parola pesanti, parola finestra, parola DEMOLAB"
        );
        // l'utente scrive "demo lab" a mano nel testo corretto: la voce smette di sostituire
        assert_eq!(d.note_kept_forms("Testo con demo lab in mezzo.").unwrap(), 1);
        assert_eq!(d.note_kept_forms("Testo con Demo Lab in mezzo.").unwrap(), 0);
        let rules = super::super::postprocess::Rules::from_entries(&d.list().unwrap());
        assert_eq!(
            rules.apply_replacements("prova cloud.ai, parola pesanty, parola fenestra, parola demo lab").text,
            "prova CLAUDE.ai, parola pesanti, parola finestra, parola demo lab"
        );
        assert!(d.has_term("DEMOLAB"));
    }
}
