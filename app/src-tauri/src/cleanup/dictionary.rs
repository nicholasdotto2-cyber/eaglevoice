//! Dizionario personale: parole, sostituzioni, snippet e candidati appresi dalle
//! correzioni dopo l'incolla. SQLite in `dictionary.sqlite` nella cartella dati dell'app.

use once_cell::sync::Lazy;
use rusqlite::{params, Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use specta::Type;
use std::collections::HashSet;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use super::postprocess::{ltokens, tokens};

#[derive(Serialize, Deserialize, Type, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DictionarySource {
    Manual,
    Learned,
    Imported,
    /// Imparata dalle correzioni a mano dopo l'incolla (`learn.rs`).
    UserEdits,
}

impl DictionarySource {
    fn as_str(&self) -> &'static str {
        match self {
            DictionarySource::Manual => "manual",
            DictionarySource::Learned => "learned",
            DictionarySource::Imported => "imported",
            DictionarySource::UserEdits => "user_edits",
        }
    }
    fn parse(s: &str) -> Self {
        match s {
            "learned" => DictionarySource::Learned,
            "imported" => DictionarySource::Imported,
            "user_edits" => DictionarySource::UserEdits,
            _ => DictionarySource::Manual,
        }
    }
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct DictionaryEntry {
    pub id: String,
    pub phrase: String,
    pub replacement: Option<String>,
    pub is_snippet: bool,
    pub starred: bool,
    pub frequency: u32,
    pub last_used: Option<String>,
    pub source: DictionarySource,
    /// Campo aggiunto: sostituzione da applicare solo con contesto AI intorno
    /// (per esempio "cloud" diventa "Claude"), altrimenti decide il modello.
    #[serde(default)]
    pub ambiguous: bool,
}

#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct LearnedCandidate {
    pub id: String,
    pub wrong: String,
    pub right: String,
    pub seen_count: u32,
    pub example: String,
    pub first_seen: String,
}

/// Coppia `from → to` dell'evento `win:text-corrected`.
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct TextChange {
    pub from: String,
    pub to: String,
}

/// Sostituzioni dei termini di esempio (lista inventata) per i test: nel prodotto non esistono
/// (vedi `default_replacements`). Terzo campo: ambigua (vale solo con contesto AI intorno).
#[cfg(test)]
pub const BENCH_REPLACEMENTS: &[(&str, &str, bool)] = &[
    ("cloud dot md", "claude.md", false),
    ("cloud.md", "claude.md", false),
    ("cloud code", "Claude Code", false),
    ("cloud", "Claude", true),
    ("demo lab", "DEMOLAB", false),
    ("eleven labs", "ElevenLabs", false),
    ("nomeuno", "NomeUno", false),
    ("nomeprova2", "nome.prova2", false),
    ("memory md", "memory.md", false),
    ("follow up", "follow-up", false),
    ("git hub", "GitHub", false),
    ("python", "Python", false),
    ("jason", "JSON", false),
    ("noshon", "Notion", false),
];

/// Sostituzioni predefinite aggiunte all'import di un dizionario JSON: quelle di esempio nei
/// test, nessuna nel prodotto.
fn default_replacements() -> &'static [(&'static str, &'static str, bool)] {
    #[cfg(test)]
    {
        BENCH_REPLACEMENTS
    }
    #[cfg(not(test))]
    {
        &[]
    }
}

/// Liste locali delle correzioni fatte a mano (fuori da git): `build.rs` accende
/// `termini_locali` / `grafie_locali` se i file esistono, altrimenti le liste sono vuote.
#[cfg(termini_locali)]
const TERMINI_CORREZIONI: &str = include_str!("termini_correzioni.txt");
#[cfg(not(termini_locali))]
const TERMINI_CORREZIONI: &str = "";
#[cfg(grafie_locali)]
const GRAFIE_CORREZIONI: &str = include_str!("grafie_correzioni.txt");
#[cfg(not(grafie_locali))]
const GRAFIE_CORREZIONI: &str = "";

/// Termini da proteggere oltre al dizionario: quelli corretti a mano dall'utente
/// (`termini_correzioni.txt`, locale) e le forme scritte delle sostituzioni predefinite. Servono solo al
/// controllo dei termini dopo il modello; le sostituzioni imparate stanno già nel dizionario.
pub fn correction_terms() -> Vec<String> {
    let mut v = correction_terms_list();
    v.extend(default_replacements().iter().map(|(_, to, _)| to.to_string()));
    v.sort();
    v.dedup();
    v
}

/// Solo i termini di `termini_correzioni.txt` (senza le sostituzioni predefinite).
pub fn correction_terms_list() -> Vec<String> {
    TERMINI_CORREZIONI
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// Grafie imposte al pezzo rimesso dal controllo dei termini (`grafie_correzioni.txt`, locale).
pub fn correction_spellings() -> Vec<String> {
    GRAFIE_CORREZIONI
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(String::from)
        .collect()
}

/// Lista delle parole comuni, in ordine di frequenza. Se accanto a questo file c'è
/// `parole_comuni_locali.txt` (lista locale, fuori da git) `build.rs` accende `parole_locali`
/// e si usa quella; altrimenti la lista di base `parole_base.txt`, corta e scritta a mano.
#[cfg(parole_locali)]
const PAROLE_COMUNI: &str = include_str!("parole_comuni_locali.txt");
#[cfg(not(parole_locali))]
const PAROLE_COMUNI: &str = include_str!("parole_base.txt");

/// Parole comuni italiane: un candidato con `to` qui dentro non è un termine da imparare.
static COMMON: Lazy<HashSet<String>> = Lazy::new(|| {
    PAROLE_COMUNI
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(|l| l.to_lowercase())
        .collect()
});

pub fn is_common_word(w: &str) -> bool {
    COMMON.contains(&w.to_lowercase())
}

/// Stopword: le prime 150 parole della lista, che è in ordine di frequenza ("e", "non", "che").
static STOPWORDS: Lazy<HashSet<String>> = Lazy::new(|| {
    PAROLE_COMUNI
        .lines()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .take(150)
        .map(|l| l.to_lowercase())
        .collect()
});

pub fn is_stopword(w: &str) -> bool {
    STOPWORDS.contains(&w.to_lowercase())
}

pub fn common_words_count() -> usize {
    COMMON.len()
}

fn levenshtein(a: &[char], b: &[char]) -> usize {
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut cur = vec![i + 1];
        for (j, y) in b.iter().enumerate() {
            cur.push((prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + usize::from(x != y)));
        }
        prev = cur;
    }
    prev[b.len()]
}

/// Distanza di Levenshtein normalizzata sui caratteri, senza maiuscole.
pub fn normalized_distance(a: &str, b: &str) -> f64 {
    let a: Vec<char> = a.to_lowercase().chars().collect();
    let b: Vec<char> = b.to_lowercase().chars().collect();
    let m = a.len().max(b.len());
    if m == 0 {
        return 0.0;
    }
    levenshtein(&a, &b) as f64 / m as f64
}

/// Filtro dei candidati: `to` non comune, almeno 3 caratteri, distanza plausibile.
pub fn is_plausible_candidate(from: &str, to: &str) -> bool {
    let (from, to) = (from.trim(), to.trim());
    if from.is_empty() || to.chars().count() < 3 || from == to {
        return false;
    }
    let words = ltokens(to);
    if words.is_empty() || words.iter().all(|w| is_common_word(w)) {
        return false;
    }
    normalized_distance(from, to) < 0.6 || to.chars().any(|c| c.is_uppercase())
}

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn new_id(seed: &str) -> String {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let now = chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default();
    let mut h = Sha256::new();
    h.update(format!("{seed}|{now}|{n}").as_bytes());
    let d = h.finalize();
    d.iter().take(12).map(|b| format!("{b:02x}")).collect()
}

fn now_str() -> String {
    chrono::Utc::now().to_rfc3339()
}

pub struct Dictionary {
    conn: Mutex<Connection>,
}

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS entries (
  id TEXT PRIMARY KEY,
  phrase TEXT NOT NULL,
  replacement TEXT,
  is_snippet INTEGER NOT NULL DEFAULT 0,
  starred INTEGER NOT NULL DEFAULT 0,
  frequency INTEGER NOT NULL DEFAULT 0,
  last_used TEXT,
  source TEXT NOT NULL DEFAULT 'manual',
  ambiguous INTEGER NOT NULL DEFAULT 0,
  created_at TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS candidates (
  id TEXT PRIMARY KEY,
  wrong TEXT NOT NULL,
  right TEXT NOT NULL,
  seen_count INTEGER NOT NULL DEFAULT 1,
  example TEXT NOT NULL DEFAULT '',
  first_seen TEXT NOT NULL,
  status TEXT NOT NULL DEFAULT 'pending',
  UNIQUE(wrong, right)
);
CREATE TABLE IF NOT EXISTS learned_words (
  id TEXT PRIMARY KEY,
  wrong TEXT NOT NULL,
  right TEXT NOT NULL,
  count INTEGER NOT NULL DEFAULT 1,
  created_at TEXT NOT NULL,
  active INTEGER NOT NULL DEFAULT 1,
  entry_id TEXT NOT NULL DEFAULT '',
  as_replacement INTEGER NOT NULL DEFAULT 0,
  UNIQUE(wrong, right)
);
";

/// Osservazioni prima che un candidato compaia nell'hub.
pub const CANDIDATE_MIN_SEEN: u32 = 2;

impl Dictionary {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(SCHEMA)?;
        Ok(Dictionary { conn: Mutex::new(conn) })
    }

    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        conn.execute_batch(SCHEMA)?;
        Ok(Dictionary { conn: Mutex::new(conn) })
    }

    fn row_to_entry(r: &rusqlite::Row) -> rusqlite::Result<DictionaryEntry> {
        Ok(DictionaryEntry {
            id: r.get(0)?,
            phrase: r.get(1)?,
            replacement: r.get(2)?,
            is_snippet: r.get::<_, i64>(3)? != 0,
            starred: r.get::<_, i64>(4)? != 0,
            frequency: r.get::<_, i64>(5)? as u32,
            last_used: r.get(6)?,
            source: DictionarySource::parse(&r.get::<_, String>(7)?),
            ambiguous: r.get::<_, i64>(8)? != 0,
        })
    }

    /// Voci in ordine stabile (alfabetico): il prompt costruito da qui non cambia quando cambia
    /// solo una frequenza, così la cache del prefisso regge.
    pub fn list(&self) -> anyhow::Result<Vec<DictionaryEntry>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id, phrase, replacement, is_snippet, starred, frequency, last_used, source, ambiguous
             FROM entries ORDER BY lower(phrase), id",
        )?;
        let v = st.query_map([], Self::row_to_entry)?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    pub fn add(&self, mut e: DictionaryEntry) -> anyhow::Result<DictionaryEntry> {
        if e.phrase.trim().is_empty() {
            anyhow::bail!("frase vuota");
        }
        if e.id.trim().is_empty() {
            e.id = new_id(&e.phrase);
        }
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO entries (id, phrase, replacement, is_snippet, starred, frequency, last_used, source, ambiguous, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![e.id, e.phrase.trim(), e.replacement, e.is_snippet as i64, e.starred as i64, e.frequency as i64,
                    e.last_used, e.source.as_str(), e.ambiguous as i64, now_str()],
        )?;
        Ok(e)
    }

    pub fn update(&self, e: &DictionaryEntry) -> anyhow::Result<()> {
        let c = self.conn.lock().unwrap();
        let n = c.execute(
            "UPDATE entries SET phrase = ?2, replacement = ?3, is_snippet = ?4, starred = ?5, frequency = ?6,
             last_used = ?7, source = ?8, ambiguous = ?9 WHERE id = ?1",
            params![e.id, e.phrase.trim(), e.replacement, e.is_snippet as i64, e.starred as i64, e.frequency as i64,
                    e.last_used, e.source.as_str(), e.ambiguous as i64],
        )?;
        if n == 0 {
            anyhow::bail!("voce {} non trovata", e.id);
        }
        Ok(())
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM entries WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Frequenza più uno e ultimo uso adesso.
    pub fn touch(&self, ids: &[String]) -> anyhow::Result<()> {
        let c = self.conn.lock().unwrap();
        let now = now_str();
        for id in ids {
            c.execute("UPDATE entries SET frequency = frequency + 1, last_used = ?2 WHERE id = ?1", params![id, now])?;
        }
        Ok(())
    }

    fn exists(c: &Connection, phrase: &str, replacement: Option<&str>) -> rusqlite::Result<bool> {
        let n: i64 = c.query_row(
            "SELECT count(*) FROM entries WHERE lower(phrase) = lower(?1) AND ifnull(replacement, '') = ifnull(?2, '')",
            params![phrase.trim(), replacement],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Vocabolario per l'ASR: forma scritta delle voci non snippet, stellate prima, poi per
    /// frequenza.
    pub fn vocabulary(&self) -> anyhow::Result<Vec<String>> {
        let mut v = self.list()?;
        v.retain(|e| !e.is_snippet);
        v.sort_by(|a, b| b.starred.cmp(&a.starred).then(b.frequency.cmp(&a.frequency)).then(a.phrase.cmp(&b.phrase)));
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for e in v {
            let term = e.replacement.clone().unwrap_or(e.phrase.clone());
            let Some(term) = asr_form(&term) else { continue };
            if seen.insert(term.to_lowercase()) {
                out.push(term);
            }
        }
        Ok(out)
    }

    /// Import da un file JSON: un array di voci con `term` (obbligatorio), `replacement` (testo o
    /// `null`, si può omettere) e `snippet` (booleano, falso se manca). Esempio:
    ///
    /// ```json
    /// [{"term": "Nomeprova", "replacement": null},
    ///  {"term": "nome esempio", "replacement": "NomeEsempio", "snippet": false}]
    /// ```
    ///
    /// Le voci senza `term`, vuote o già presenti si saltano; si aggiungono anche le sostituzioni
    /// dei termini di esempio. Restituisce le voci aggiunte.
    pub fn import_from_json(&self, path: &Path) -> anyhow::Result<u32> {
        let raw = std::fs::read_to_string(path)?;
        let items: Vec<serde_json::Value> = serde_json::from_str(&raw)?;
        let entries = items
            .iter()
            .filter_map(|it| {
                let term = it.get("term")?.as_str()?;
                Some(DictionaryEntry {
                    id: String::new(),
                    phrase: term.to_string(),
                    replacement: it.get("replacement").and_then(|v| v.as_str()).map(|s| s.to_string()),
                    is_snippet: it.get("snippet").and_then(|v| v.as_bool()).unwrap_or(false),
                    starred: false,
                    frequency: 0,
                    last_used: None,
                    source: DictionarySource::Imported,
                    ambiguous: false,
                })
            })
            .collect();
        self.import_entries(entries)
    }

    /// Aggiunge voci importate (vuote e doppioni saltati, sostituzione vuota tolta) più le
    /// sostituzioni predefinite (`default_replacements`). Restituisce le voci aggiunte.
    pub(crate) fn import_entries(&self, entries: Vec<DictionaryEntry>) -> anyhow::Result<u32> {
        let mut added = 0;
        for mut e in entries {
            if e.phrase.trim().is_empty() {
                continue;
            }
            e.replacement = e.replacement.filter(|s| !s.trim().is_empty());
            e.source = DictionarySource::Imported;
            {
                let c = self.conn.lock().unwrap();
                if Self::exists(&c, &e.phrase, e.replacement.as_deref())? {
                    continue;
                }
            }
            self.add(e)?;
            added += 1;
        }
        for (from, to, ambiguous) in default_replacements() {
            {
                let c = self.conn.lock().unwrap();
                if Self::exists(&c, from, Some(to))? {
                    continue;
                }
            }
            self.add(DictionaryEntry {
                id: String::new(),
                phrase: from.to_string(),
                replacement: Some(to.to_string()),
                is_snippet: false,
                starred: false,
                frequency: 0,
                last_used: None,
                source: DictionarySource::Imported,
                ambiguous: *ambiguous,
            })?;
            added += 1;
        }
        Ok(added)
    }

    /// Candidati da mostrare nell'hub: in attesa e visti almeno 2 volte.
    pub fn candidates(&self) -> anyhow::Result<Vec<LearnedCandidate>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT id, wrong, right, seen_count, example, first_seen FROM candidates
             WHERE status = 'pending' AND seen_count >= ?1 ORDER BY seen_count DESC, first_seen",
        )?;
        let v = st
            .query_map(params![CANDIDATE_MIN_SEEN], |r| {
                Ok(LearnedCandidate {
                    id: r.get(0)?,
                    wrong: r.get(1)?,
                    right: r.get(2)?,
                    seen_count: r.get::<_, i64>(3)? as u32,
                    example: r.get(4)?,
                    first_seen: r.get(5)?,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Accetta un candidato: voce `learned` con sostituzione `wrong → right`.
    pub fn accept_candidate(&self, id: &str) -> anyhow::Result<DictionaryEntry> {
        let (wrong, right): (String, String) = {
            let c = self.conn.lock().unwrap();
            let row = c
                .query_row("SELECT wrong, right FROM candidates WHERE id = ?1", params![id], |r| Ok((r.get(0)?, r.get(1)?)))
                .optional()?;
            let Some(row) = row else { anyhow::bail!("candidato {id} non trovato") };
            c.execute("UPDATE candidates SET status = 'accepted' WHERE id = ?1", params![id])?;
            row
        };
        self.add(DictionaryEntry {
            id: String::new(),
            phrase: wrong,
            replacement: Some(right),
            is_snippet: false,
            starred: false,
            frequency: 0,
            last_used: None,
            source: DictionarySource::Learned,
            ambiguous: false,
        })
    }

    pub fn reject_candidate(&self, id: &str) -> anyhow::Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute("UPDATE candidates SET status = 'rejected' WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Registra le correzioni osservate dopo l'incolla. Se `changes` è vuoto le ricava dal
    /// confronto a parole fra `pasted` ed `edited`. Restituisce (candidati toccati, almeno uno
    /// è appena diventato visibile nell'hub).
    pub fn observe_correction(&self, pasted: &str, edited: &str, changes: &[TextChange]) -> anyhow::Result<(u32, bool)> {
        let derived;
        let changes = if changes.is_empty() {
            derived = diff_changes(pasted, edited);
            &derived
        } else {
            changes
        };
        let c = self.conn.lock().unwrap();
        let mut touched = 0;
        let mut visible = false;
        for ch in changes {
            let (from, to) = (ch.from.trim(), ch.to.trim());
            if !is_plausible_candidate(from, to) {
                continue;
            }
            // sostituzione già nel dizionario: niente candidato (una voce solo di vocabolario
            // come "CODEX" non basta: la sostituzione "Codecs → CODEX" serve lo stesso)
            let known: i64 = c.query_row(
                "SELECT count(*) FROM entries WHERE lower(replacement) = lower(?1) AND lower(phrase) = lower(?2)",
                params![to, from],
                |r| r.get(0),
            )?;
            if known > 0 {
                continue;
            }
            let example = example_around(edited, to);
            let prev: Option<(String, i64, String)> = c
                .query_row(
                    "SELECT id, seen_count, status FROM candidates WHERE wrong = ?1 AND right = ?2",
                    params![from, to],
                    |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
                )
                .optional()?;
            match prev {
                Some((_, _, status)) if status != "pending" => continue,
                Some((id, seen, _)) => {
                    c.execute("UPDATE candidates SET seen_count = seen_count + 1 WHERE id = ?1", params![id])?;
                    if seen + 1 == CANDIDATE_MIN_SEEN as i64 {
                        visible = true;
                    }
                }
                None => {
                    c.execute(
                        "INSERT INTO candidates (id, wrong, right, seen_count, example, first_seen) VALUES (?1, ?2, ?3, 1, ?4, ?5)",
                        params![new_id(to), from, to, example, now_str()],
                    )?;
                }
            }
            touched += 1;
        }
        Ok((touched, visible))
    }
}

/// Parola imparata dalle correzioni (pagina "Imparate" dell'hub).
#[derive(Serialize, Deserialize, Type, Clone, Debug, PartialEq)]
pub struct LearnedWord {
    pub id: String,
    /// Forma sbagliata d'origine, com'era nel testo incollato.
    pub wrong: String,
    pub right: String,
    pub count: u32,
    pub created_at: String,
    /// Falso dopo "Annulla" o se la voce del dizionario è stata tolta a mano.
    pub active: bool,
}

/// Esito di `Dictionary::learn_pair`.
#[derive(Debug, Clone, PartialEq)]
pub enum LearnOutcome {
    /// Voce nuova: id della parola imparata.
    Added(String),
    /// Coppia già imparata e attiva: conteggio più uno.
    Seen(String),
    /// Coppia annullata in passato: non si reimpara.
    Refused,
}

impl Dictionary {
    /// Parola o sostituzione già nel dizionario (frase o forma sostituita, senza maiuscole).
    pub fn has_term(&self, term: &str) -> bool {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT count(*) FROM entries WHERE lower(phrase) = lower(?1) OR lower(ifnull(replacement, '')) = lower(?1)",
            params![term.trim()],
            |r| r.get::<_, i64>(0),
        )
        .map(|n| n > 0)
        .unwrap_or(false)
    }

    /// Stato della coppia fra le imparate: `Some(true)` attiva, `Some(false)` annullata.
    pub fn learned_state(&self, wrong: &str, right: &str) -> Option<bool> {
        let c = self.conn.lock().unwrap();
        c.query_row(
            "SELECT l.active, (SELECT count(*) FROM entries e WHERE e.id = l.entry_id) FROM learned_words l
             WHERE lower(l.wrong) = lower(?1) AND l.right = ?2",
            params![wrong.trim(), right.trim()],
            |r| Ok(r.get::<_, i64>(0)? != 0 && r.get::<_, i64>(1)? > 0),
        )
        .optional()
        .ok()
        .flatten()
    }

    /// Impara la coppia: voce `user_edits` subito nel dizionario. Con `as_replacement` la voce è
    /// la sostituzione `wrong → right` a confini di parola, altrimenti solo vocabolario (`right`).
    /// Una coppia già attiva aumenta conteggio e frequenza; una annullata resta fuori.
    pub fn learn_pair(&self, wrong: &str, right: &str, as_replacement: bool) -> anyhow::Result<LearnOutcome> {
        let (wrong, right) = (wrong.trim(), right.trim());
        match self.learned_state(wrong, right) {
            Some(true) => {
                let c = self.conn.lock().unwrap();
                let (id, entry): (String, String) = c.query_row(
                    "SELECT id, entry_id FROM learned_words WHERE lower(wrong) = lower(?1) AND right = ?2",
                    params![wrong, right],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )?;
                c.execute("UPDATE learned_words SET count = count + 1 WHERE id = ?1", params![id])?;
                c.execute("UPDATE entries SET frequency = frequency + 1, last_used = ?2 WHERE id = ?1", params![entry, now_str()])?;
                return Ok(LearnOutcome::Seen(id));
            }
            Some(false) => {
                // annullata, oppure voce tolta a mano dal dizionario: la scelta dell'utente vince
                return Ok(LearnOutcome::Refused);
            }
            None => {}
        }
        let entry = self.add(DictionaryEntry {
            id: String::new(),
            phrase: if as_replacement { wrong.to_string() } else { right.to_string() },
            replacement: if as_replacement { Some(right.to_string()) } else { None },
            is_snippet: false,
            starred: false,
            frequency: 1,
            last_used: Some(now_str()),
            source: DictionarySource::UserEdits,
            // "cloud" → "Claude" imparata: vale solo con contesto AI, come la voce predefinita
            // (il "cloud" dell'archiviazione resta)
            ambiguous: as_replacement && default_replacements().iter().any(|(f, _, a)| *a && f.eq_ignore_ascii_case(wrong)),
        })?;
        let id = new_id(right);
        let c = self.conn.lock().unwrap();
        c.execute(
            "INSERT INTO learned_words (id, wrong, right, count, created_at, active, entry_id, as_replacement)
             VALUES (?1, ?2, ?3, 1, ?4, 1, ?5, ?6)",
            params![id, wrong, right, now_str(), entry.id, as_replacement as i64],
        )?;
        Ok(LearnOutcome::Added(id))
    }

    /// Candidati `pending` del vecchio meccanismo (da rivalutare con `learn::migrate`).
    pub fn pending_candidates(&self) -> anyhow::Result<Vec<(String, String, String, String)>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare("SELECT id, wrong, right, example FROM candidates WHERE status = 'pending' ORDER BY first_seen, id")?;
        let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Esito della rivalutazione di un candidato: `accepted`, `rejected` o `filtered`.
    pub fn set_candidate_status(&self, id: &str, status: &str) -> anyhow::Result<()> {
        let c = self.conn.lock().unwrap();
        c.execute("UPDATE candidates SET status = ?2 WHERE id = ?1", params![id, status])?;
        Ok(())
    }

    /// Parole imparate, le più recenti prima. `active` è falso anche se la voce del dizionario
    /// non c'è più.
    pub fn learned_words(&self) -> anyhow::Result<Vec<LearnedWord>> {
        let c = self.conn.lock().unwrap();
        let mut st = c.prepare(
            "SELECT l.id, l.wrong, l.right, l.count, l.created_at,
                    l.active != 0 AND EXISTS (SELECT 1 FROM entries e WHERE e.id = l.entry_id)
             FROM learned_words l ORDER BY l.created_at DESC, l.id",
        )?;
        let v = st
            .query_map([], |r| {
                Ok(LearnedWord {
                    id: r.get(0)?,
                    wrong: r.get(1)?,
                    right: r.get(2)?,
                    count: r.get::<_, i64>(3)? as u32,
                    created_at: r.get(4)?,
                    active: r.get::<_, i64>(5)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(v)
    }

    /// Voce del dizionario e "sostituzione esatta" della parola imparata.
    pub fn learned_entry(&self, id: &str) -> anyhow::Result<Option<(String, bool)>> {
        let c = self.conn.lock().unwrap();
        Ok(c.query_row("SELECT entry_id, as_replacement FROM learned_words WHERE id = ?1", params![id], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)? != 0))
        })
        .optional()?)
    }

    /// "Annulla": la voce sparisce dal dizionario (quindi da sostituzioni e prompt dell'ASR) e la
    /// coppia resta segnata come rifiutata, così non si reimpara alla prossima correzione.
    pub fn undo_learned(&self, id: &str) -> anyhow::Result<()> {
        let Some((entry, _)) = self.learned_entry(id)? else { anyhow::bail!("parola imparata {id} non trovata") };
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM entries WHERE id = ?1", params![entry])?;
        c.execute("UPDATE learned_words SET active = 0 WHERE id = ?1", params![id])?;
        Ok(())
    }

    /// Una forma sbagliata che l'utente scrive lui stesso nel testo corretto non è
    /// sbagliata per lui. Ogni sostituzione imparata la cui forma sbagliata compare esatta in
    /// `edited` diventa solo vocabolario (la forma giusta resta nel dizionario e nel prompt).
    /// Restituisce quante voci sono cambiate.
    pub fn note_kept_forms(&self, edited: &str) -> anyhow::Result<u32> {
        let rows: Vec<(String, String, String, String)> = {
            let c = self.conn.lock().unwrap();
            let mut st = c.prepare("SELECT id, wrong, right, entry_id FROM learned_words WHERE active != 0 AND as_replacement != 0")?;
            let v = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?.collect::<Result<Vec<_>, _>>()?;
            v
        };
        let mut n = 0;
        for (id, wrong, right, entry) in rows {
            if !super::postprocess::contains_exact_form(edited, &wrong) {
                continue;
            }
            let c = self.conn.lock().unwrap();
            c.execute("UPDATE entries SET phrase = ?2, replacement = NULL, ambiguous = 0 WHERE id = ?1", params![entry, right])?;
            c.execute("UPDATE learned_words SET as_replacement = 0 WHERE id = ?1", params![id])?;
            n += 1;
        }
        Ok(n)
    }

    /// Toglie la parola imparata e la sua voce: la coppia potrà essere reimparata.
    pub fn delete_learned(&self, id: &str) -> anyhow::Result<()> {
        let Some((entry, _)) = self.learned_entry(id)? else { anyhow::bail!("parola imparata {id} non trovata") };
        let c = self.conn.lock().unwrap();
        c.execute("DELETE FROM entries WHERE id = ?1", params![entry])?;
        c.execute("DELETE FROM learned_words WHERE id = ?1", params![id])?;
        Ok(())
    }
}

/// Grafia di un termine per il prompt dell'ASR (whisper ricopia la grafia del prompt).
/// `None` per le voci che non servono o fanno danno: email, URL, testi lunghi, più di 3 parole,
/// parole di una lettera ("sNOTE NOMEPROVA x"), voci senza lettere.
/// Regola per parola: sigla tutta maiuscola fino a 4 lettere resta ("API"); maiuscole storte
/// ("nOMEPROVA", "fOTO3") e tutto maiuscolo lungo ("TERMINEPROVA") diventano iniziale maiuscola
/// ("Nomeprova", "Foto3", "Termineprova"); forme volute ("ElevenLabs", "claude.md", "npm") restano.
pub fn asr_form(term: &str) -> Option<String> {
    let t = term.trim();
    if t.is_empty() || t.contains('@') || t.contains("://") || t.chars().count() > 40 {
        return None;
    }
    let words: Vec<&str> = t.split_whitespace().collect();
    if words.len() > 3 || !t.chars().any(|c| c.is_alphabetic()) {
        return None;
    }
    if words.iter().any(|w| w.chars().filter(|c| c.is_alphanumeric()).count() < 2) {
        return None;
    }
    let fixed: Vec<String> = words
        .iter()
        .map(|w| {
            let letters: Vec<char> = w.chars().filter(|c| c.is_alphabetic()).collect();
            let all_upper = letters.iter().all(|c| c.is_uppercase());
            let first_lower = w.chars().next().is_some_and(|c| c.is_lowercase());
            let later_upper = w.chars().skip(1).any(|c| c.is_uppercase());
            if all_upper && letters.len() <= 4 {
                w.to_string()
            } else if all_upper || (first_lower && later_upper) {
                let lower = w.to_lowercase();
                let mut c = lower.chars();
                c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
            } else {
                w.to_string()
            }
        })
        .collect();
    Some(fixed.join(" "))
}

fn example_around(text: &str, word: &str) -> String {
    let lower = text.to_lowercase();
    let pos = lower.find(&word.to_lowercase()).filter(|p| text.is_char_boundary(*p)).unwrap_or(0);
    let start = text[..pos].char_indices().rev().nth(50).map_or(0, |(i, _)| i);
    let end = text[pos..].char_indices().nth(word.len() + 50).map_or(text.len(), |(i, _)| pos + i);
    text[start..end].trim().to_string()
}

/// Coppie di parole cambiate fra testo incollato e testo corretto (pezzi fino a 3 parole).
pub fn diff_changes(pasted: &str, edited: &str) -> Vec<TextChange> {
    let a = tokens(pasted);
    let b = tokens(edited);
    let la: Vec<String> = a.iter().map(|s| s.to_lowercase()).collect();
    let lb: Vec<String> = b.iter().map(|s| s.to_lowercase()).collect();
    let (n, m) = (la.len(), lb.len());
    let mut dp = vec![0u32; (n + 1) * (m + 1)];
    let idx = |i: usize, j: usize| i * (m + 1) + j;
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            dp[idx(i, j)] = if la[i] == lb[j] { dp[idx(i + 1, j + 1)] + 1 } else { dp[idx(i + 1, j)].max(dp[idx(i, j + 1)]) };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0, 0);
    while i < n || j < m {
        if i < n && j < m && (la[i] == lb[j]) {
            // stesse lettere, maiuscole diverse ("codex" diventa "CODEX")
            if a[i] != b[j] {
                out.push(TextChange { from: a[i].clone(), to: b[j].clone() });
            }
            i += 1;
            j += 1;
            continue;
        }
        let (si, sj) = (i, j);
        while i < n || j < m {
            if i < n && j < m && la[i] == lb[j] {
                break;
            }
            if j >= m || (i < n && dp[idx(i + 1, j)] >= dp[idx(i, j + 1)]) {
                i += 1;
            } else {
                j += 1;
            }
        }
        if i > si && j > sj && i - si <= 3 && j - sj <= 3 {
            out.push(TextChange { from: a[si..i].join(" "), to: b[sj..j].join(" ") });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lista_parole_comuni_abbastanza_lunga() {
        #[cfg(parole_locali)]
        assert!(common_words_count() >= 2000, "{}", common_words_count());
        #[cfg(not(parole_locali))]
        assert!(common_words_count() >= 300, "{}", common_words_count());
        assert!(is_common_word("perché") && is_common_word("casa"));
        assert!(!is_common_word("claude"));
    }

    #[test]
    fn filtro_dei_candidati() {
        assert!(is_plausible_candidate("cloud", "Claude"));
        assert!(is_plausible_candidate("Codecs", "CODEX"));
        assert!(is_plausible_candidate("noshon", "notion"));
        assert!(!is_plausible_candidate("fatto", "fatte")); // parola comune
        assert!(!is_plausible_candidate("la", "il")); // troppo corta
        assert!(!is_plausible_candidate("bursetto", "v7x")); // lontana e senza maiuscole
    }

    #[test]
    fn candidato_visibile_dopo_due_osservazioni_e_accettato() {
        let d = Dictionary::open_in_memory().unwrap();
        let ch = [TextChange { from: "Codecs".into(), to: "CODEX".into() }];
        assert_eq!(d.observe_correction("parola Codecs", "parola CODEX", &ch).unwrap(), (1, false));
        assert!(d.candidates().unwrap().is_empty());
        assert_eq!(d.observe_correction("", "testo CODEX", &ch).unwrap(), (1, true));
        let c = d.candidates().unwrap();
        assert_eq!(c.len(), 1);
        let e = d.accept_candidate(&c[0].id).unwrap();
        assert_eq!((e.phrase.as_str(), e.replacement.as_deref(), e.source), ("Codecs", Some("CODEX"), DictionarySource::Learned));
        assert!(d.candidates().unwrap().is_empty());
    }

    #[test]
    fn correzioni_ricavate_dal_confronto() {
        let ch = diff_changes("Parola cloud e parola Codecs.", "Parola Claude e parola CODEX.");
        assert_eq!(ch, vec![
            TextChange { from: "cloud".into(), to: "Claude".into() },
            TextChange { from: "Codecs".into(), to: "CODEX".into() },
        ]);
    }

    #[test]
    fn vocabolario_stellate_poi_frequenza() {
        let d = Dictionary::open_in_memory().unwrap();
        let mk = |p: &str, r: Option<&str>, star: bool, f: u32, snip: bool| DictionaryEntry {
            id: String::new(), phrase: p.into(), replacement: r.map(|s| s.into()), is_snippet: snip, starred: star,
            frequency: f, last_used: None, source: DictionarySource::Manual, ambiguous: false,
        };
        d.add(mk("Notion", None, false, 17, false)).unwrap();
        d.add(mk("TERMINEPROVA", None, true, 1, false)).unwrap();
        d.add(mk("codecs", Some("CODEX"), false, 45, false)).unwrap();
        d.add(mk("my email", Some("a@b.it"), false, 99, true)).unwrap();
        assert_eq!(d.vocabulary().unwrap(), vec!["Termineprova", "Codex", "Notion"]);
    }

    #[test]
    fn grafia_per_asr_normalizzata() {
        assert_eq!(asr_form("nOMEPROVA").as_deref(), Some("Nomeprova"));
        assert_eq!(asr_form("npm").as_deref(), Some("npm"));
        assert_eq!(asr_form("AI STUDIO").as_deref(), Some("AI Studio"));
        assert_eq!(asr_form("ElevenLabs").as_deref(), Some("ElevenLabs"));
        assert_eq!(asr_form("piano-lavori").as_deref(), Some("piano-lavori"));
        assert_eq!(asr_form("nome.prova@example.com"), None);
        assert_eq!(asr_form("tESTO PROVA x"), None);
        assert_eq!(asr_form("Testo lungo di prova con molte parole, scritto solo per questo controllo!"), None);
    }

    #[test]
    fn import_dal_formato_json() {
        let d = Dictionary::open_in_memory().unwrap();
        let path = std::env::temp_dir().join(format!("dizionario-import-{}.json", new_id("prova")));
        std::fs::write(
            &path,
            r#"[
                {"term": "Termineprova", "replacement": null},
                {"term": "nome prova", "replacement": "NomeProva", "snippet": false},
                {"term": "firma breve", "replacement": "Nome Esempio, Firma di prova", "snippet": true},
                {"term": "termineuno", "replacement": "  "},
                {"term": "   "},
                {"replacement": "senza termine"},
                {"term": "termineprova"}
            ]"#,
        )
        .unwrap();
        let added = d.import_from_json(&path).unwrap();
        let again = d.import_from_json(&path).unwrap();
        std::fs::remove_file(&path).ok();
        // 4 voci nuove ("termineprova" è un doppione di "Termineprova") più le sostituzioni di esempio
        assert_eq!(added as usize, 4 + BENCH_REPLACEMENTS.len());
        assert_eq!(again, 0);
        let all = d.list().unwrap();
        let get = |p: &str| all.iter().find(|e| e.phrase == p).unwrap_or_else(|| panic!("{p} mancante"));
        let o = get("nome prova");
        assert_eq!((o.replacement.as_deref(), o.is_snippet, o.source), (Some("NomeProva"), false, DictionarySource::Imported));
        assert!(get("firma breve").is_snippet);
        assert_eq!(get("Termineprova").replacement, None);
        assert_eq!(get("termineuno").replacement, None);
        assert!(!all.iter().any(|e| e.phrase == "termineprova"));
    }
}
