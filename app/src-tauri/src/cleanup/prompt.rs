//! Prompt della pulizia: file di testo versionati in `prompts\`, segnaposto riempiti qui.
//!
//! Ogni file ha due parti separate dalla riga `=== UTENTE ===`: il messaggio di sistema (regole,
//! dizionario, lingua: uguale fra una dettatura e l'altra, così llama-server riusa il prefisso in
//! cache) e il messaggio utente (stile, formato, contesto, testo: cambia ogni volta, sta in fondo).

use super::{CleanupContext, CleanupLevel, CommandKind, CommandResult, DictionaryEntry, TranscriptFormat, Tone};

const SEPARATOR: &str = "=== UTENTE ===";
/// Voci del dizionario messe nel prompt al massimo (le altre restano alle sostituzioni).
pub const MAX_DICTIONARY_LINES: usize = 200;

#[derive(Clone, Debug)]
pub struct Template {
    pub system: String,
    pub user: String,
}

impl Template {
    pub fn parse(src: &str) -> Template {
        let src = src.replace("\r\n", "\n");
        match src.split_once(SEPARATOR) {
            Some((s, u)) => Template { system: s.trim().to_string(), user: u.trim().to_string() },
            None => Template { system: src.trim().to_string(), user: "{{testo}}".to_string() },
        }
    }
}

#[derive(Clone, Debug)]
pub struct PromptSet {
    pub exact: Template,
    pub tidy: Template,
    pub smooth: Template,
    pub rewrite: Template,
    pub polish: Template,
    pub command: Template,
    pub classifier: Template,
}

impl PromptSet {
    pub fn load() -> PromptSet {
        PromptSet {
            exact: Template::parse(include_str!("../../prompts/cleanup-exact.txt")),
            tidy: Template::parse(include_str!("../../prompts/cleanup-tidy.txt")),
            smooth: Template::parse(include_str!("../../prompts/cleanup-smooth.txt")),
            rewrite: Template::parse(include_str!("../../prompts/cleanup-rewrite.txt")),
            polish: Template::parse(include_str!("../../prompts/polish.txt")),
            command: Template::parse(include_str!("../../prompts/command.txt")),
            classifier: Template::parse(include_str!("../../prompts/classificatore-correzioni.txt")),
        }
    }

    fn level(&self, level: CleanupLevel) -> &Template {
        match level {
            CleanupLevel::Exact => &self.exact,
            CleanupLevel::Tidy => &self.tidy,
            CleanupLevel::Smooth => &self.smooth,
            CleanupLevel::Rewrite => &self.rewrite,
        }
    }

    /// Messaggio di sistema: regole, esempi e dizionario completo (senza le voci identità), in
    /// forma fissa: è il prefisso che llama-server tiene in cache. Nel messaggio utente il
    /// dizionario finiva a volte ricopiato nell'uscita del modello, per questo sta qui.
    pub fn cleanup_system(&self, level: CleanupLevel, entries: &[DictionaryEntry], language: &str) -> String {
        self.level(level)
            .system
            .replace("{{dizionario}}", &render_dictionary(entries))
            .replace("{{lingua}}", language_name(language))
    }

    /// Messaggio utente: blocco `<contesto>` (stile, formato, app) e testo grezzo fra
    /// `<trascrizione>` e `</trascrizione>`, in fondo. `_entries` non si usa più (il dizionario
    /// sta nel sistema), resta per non cambiare la firma.
    pub fn cleanup_user(&self, level: CleanupLevel, ctx: &CleanupContext, text: &str, lists_allowed: bool, _entries: &[DictionaryEntry]) -> String {
        let filled = self
            .level(level)
            .user
            .replace("{{stile}}", style_text(ctx.style))
            .replace("{{formato}}", &format_text(ctx.format, lists_allowed))
            .replace("{{contesto}}", &context_text(ctx))
            .replace("{{testo}}", text.trim());
        drop_empty_lines(&filled)
    }

    pub fn polish(&self, entries: &[DictionaryEntry], instruction_ids: &[String], text: &str) -> (String, String) {
        let system = self
            .polish
            .system
            .replace("{{dizionario}}", &render_dictionary(entries))
            .replace("{{lingua}}", "la stessa del testo");
        let lines: Vec<String> = instruction_ids
            .iter()
            .map(|id| {
                let t = super::polish_instruction_text(id);
                // un id sconosciuto è un'istruzione personale scritta dall'utente
                if t.is_empty() { format!("- {id}") } else { format!("- {t}") }
            })
            .collect();
        let user = self
            .polish
            .user
            .replace("{{istruzioni}}", &lines.join("\n"))
            .replace("{{contesto}}", "")
            .replace("{{testo}}", text.trim());
        (system, drop_empty_lines(&user))
    }

    pub fn command(&self, entries: &[DictionaryEntry], selected: &str, spoken: &str) -> (String, String) {
        let system = self
            .command
            .system
            .replace("{{dizionario}}", &render_dictionary(entries))
            .replace("{{lingua}}", "italiano");
        let sel = if selected.trim().is_empty() { "(nessuno)" } else { selected.trim() };
        let user = self
            .command
            .user
            .replace("{{contesto}}", "")
            .replace("{{selezione}}", sel)
            .replace("{{comando}}", spoken.trim());
        (system, user.trim().to_string())
    }
}

fn drop_empty_lines(s: &str) -> String {
    // toglie solo le righe nate vuote da un segnaposto vuoto in testa al messaggio
    let mut out: Vec<&str> = Vec::new();
    for l in s.lines() {
        if l.trim().is_empty() && out.last().map_or(true, |p: &&str| p.trim().is_empty()) {
            continue;
        }
        out.push(l);
    }
    out.join("\n").trim().to_string()
}

/// Codice di lingua normalizzato: quelli noti restano ("en", "es", "fr", "de"), vuoto o
/// sconosciuto diventa "it" (la lingua di default e del prefisso riscaldato).
pub fn normalize_language(code: &str) -> &'static str {
    match code.trim().to_lowercase().as_str() {
        "en" => "en",
        "es" => "es",
        "fr" => "fr",
        "de" => "de",
        _ => "it",
    }
}

pub fn language_name(code: &str) -> &'static str {
    match normalize_language(code) {
        "en" => "inglese",
        "es" => "spagnolo",
        "fr" => "francese",
        "de" => "tedesco",
        _ => "italiano",
    }
}

/// Dizionario nel prompt: una riga per voce, in ordine stabile; snippet esclusi (li gestiscono i
/// segnaposto).
pub fn render_dictionary(entries: &[DictionaryEntry]) -> String {
    let mut lines = Vec::new();
    for e in entries.iter().filter(|e| !e.is_snippet) {
        // voci identità ("by the way → by the way"): rumore, non entrano nel prompt
        if e.replacement.as_deref().is_some_and(|r| r.trim().eq_ignore_ascii_case(e.phrase.trim())) {
            continue;
        }
        let line = match &e.replacement {
            Some(to) if e.ambiguous => format!(
                "- {} → {} (solo quando si parla di AI, agenti, prompt o codice; il cloud come servizio di archiviazione resta \"{}\")",
                e.phrase, to, e.phrase
            ),
            Some(to) => format!("- {} → {}", e.phrase, to),
            None => {
                // voci imparate storte ("nOMEPROVA", "sSITO NOMEPROVA x"): il modello le ricopia così
                // come sono, quindi si correggono le maiuscole storte e si saltano quelle con
                // parole di una lettera
                let words: Vec<&str> = e.phrase.split_whitespace().collect();
                if words.iter().any(|w| w.chars().filter(|c| c.is_alphanumeric()).count() < 2) {
                    continue;
                }
                let fixed: Vec<String> = words.iter().map(|w| fix_odd_case(w)).collect();
                format!("- {}", fixed.join(" "))
            }
        };
        lines.push(line);
        if lines.len() >= MAX_DICTIONARY_LINES {
            break;
        }
    }
    if lines.is_empty() {
        "- (nessuna voce)".to_string()
    } else {
        lines.join("\n")
    }
}

/// "nOMEPROVA" diventa "Nomeprova", "fOTO7" diventa "Foto7"; le altre forme restano.
pub fn fix_odd_case(w: &str) -> String {
    let first_lower = w.chars().next().is_some_and(|c| c.is_lowercase());
    let later_upper = w.chars().skip(1).any(|c| c.is_uppercase());
    if first_lower && later_upper {
        let lower = w.to_lowercase();
        let mut c = lower.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    } else {
        w.to_string()
    }
}

pub fn style_text(style: Tone) -> &'static str {
    match style {
        Tone::Default => "",
        Tone::Polished => "Stile: curato. Frasi complete e ordinate, niente espressioni colloquiali aggiunte, ma non cambiare il contenuto.",
        Tone::Relaxed => "Stile: rilassato e diretto, come parla l'utente. Non renderlo più formale.",
        Tone::Loose => "Stile: sciolto, da messaggio veloce. Frasi brevi, punteggiatura leggera.",
        Tone::Lively => "Stile: vivace e cordiale. Punti esclamativi dove vengono naturali, senza aggiungere contenuti.",
    }
}

pub fn format_text(format: TranscriptFormat, lists_allowed: bool) -> String {
    let f = match format {
        TranscriptFormat::Smart | TranscriptFormat::Raw => "Formato: testo normale.",
        TranscriptFormat::Chat => "Formato: messaggio di chat.",
        TranscriptFormat::Lower => "Formato: messaggio di chat.",
    };
    if lists_allowed {
        f.to_string()
    } else {
        format!("{f} Elenchi: no, scrivi tutto in frasi.")
    }
}

fn context_text(ctx: &CleanupContext) -> String {
    let mut parts = Vec::new();
    if !ctx.app_name.trim().is_empty() {
        let cat = if ctx.category.is_empty() { String::new() } else { format!(" (categoria {})", ctx.category) };
        parts.push(format!("App: {}{}.", ctx.app_name.trim(), cat));
    }
    if !ctx.proper_nouns.is_empty() {
        let names: Vec<&str> = ctx.proper_nouns.iter().map(|s| s.as_str()).take(30).collect();
        parts.push(format!("Nomi sullo schermo, da scrivere così: {}.", names.join(", ")));
    }
    parts.join(" ")
}

/// Legge "TIPO: ..." dalla prima riga; senza tipo valido, con una selezione è una modifica,
/// senza è una bozza.
pub fn parse_command_output(out: &str, selected: &str) -> CommandResult {
    let t = out.trim();
    let (first, rest) = t.split_once('\n').unwrap_or((t, ""));
    let head = first.trim().to_lowercase();
    let kind = if head.starts_with("tipo:") {
        let k = head.trim_start_matches("tipo:").trim();
        match k {
            "edit_text" => Some(CommandKind::EditText),
            "draft_text" => Some(CommandKind::DraftText),
            "answer" => Some(CommandKind::Answer),
            _ => None,
        }
    } else {
        None
    };
    match kind {
        Some(k) => CommandResult { kind: k, text: rest.trim().to_string() },
        None => CommandResult {
            kind: if selected.trim().is_empty() { CommandKind::DraftText } else { CommandKind::EditText },
            text: t.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::DictionarySource;

    fn e(p: &str, r: Option<&str>) -> DictionaryEntry {
        DictionaryEntry {
            id: p.into(), phrase: p.into(), replacement: r.map(|s| s.into()), is_snippet: false, starred: false,
            frequency: 0, last_used: None, source: DictionarySource::Imported, ambiguous: false,
        }
    }

    #[test]
    fn dizionario_nel_prefisso_stabile() {
        let p = PromptSet::load();
        let entries = vec![e("CODEX", None), e("eleven labs", Some("ElevenLabs")), e("by the way", Some("by the way"))];
        let s1 = p.cleanup_system(CleanupLevel::Tidy, &entries, "it");
        assert!(s1.contains("- CODEX\n- eleven labs → ElevenLabs"), "{s1}");
        assert!(!s1.contains("by the way"), "le voci identità non entrano");
        assert!(!s1.contains("{{"));
        // testo e contesto stanno solo nel messaggio utente: il sistema non cambia
        let mut ctx = CleanupContext { app_name: "Code".into(), category: "code".into(), ..Default::default() };
        let u1 = p.cleanup_user(CleanupLevel::Tidy, &ctx, "primo testo", true, &entries);
        ctx.app_name = "claude".into();
        let u2 = p.cleanup_user(CleanupLevel::Tidy, &ctx, "secondo testo", false, &entries);
        let s2 = p.cleanup_system(CleanupLevel::Tidy, &entries, "it");
        assert_eq!(s1, s2);
        assert!(u1.starts_with("<contesto>\n"), "{u1}");
        assert!(u1.ends_with("</contesto>\n<trascrizione>\nprimo testo\n</trascrizione>\nTesto pulito:"), "{u1}");
        assert!(!u1.contains("CODEX") && !u1.contains("Dizionario"));
        assert!(u2.contains("Elenchi: no") && u2.contains("App: claude"));
        assert!(!u1.contains("{{") && !u2.contains("{{"));
        let odd = render_dictionary(&[e("nOMEPROVA", None), e("tESTO PROVA x", None)]);
        assert_eq!(odd, "- Nomeprova");
        for lvl in [CleanupLevel::Exact, CleanupLevel::Smooth, CleanupLevel::Rewrite] {
            let s = p.cleanup_system(lvl, &entries, "it");
            assert!(s.contains("- CODEX") && !s.contains("{{"));
            assert!(p.cleanup_user(lvl, &ctx, "prova eleven labs", true, &entries).contains("<trascrizione>\nprova eleven labs\n</trascrizione>"));
        }
    }

    #[test]
    fn prompt_nella_lingua_rilevata() {
        let p = PromptSet::load();
        let en = p.cleanup_system(CleanupLevel::Tidy, &[], "en");
        assert!(en.contains("Lingua della dettatura: inglese") && en.contains("non tradurlo mai"), "{en}");
        let it = p.cleanup_system(CleanupLevel::Tidy, &[], "it");
        assert!(it.contains("Lingua della dettatura: italiano") && en != it);
        // vuota o sconosciuta: italiano
        assert_eq!(p.cleanup_system(CleanupLevel::Tidy, &[], ""), it);
        assert_eq!(p.cleanup_system(CleanupLevel::Tidy, &[], "pt"), it);
        assert_eq!(normalize_language("EN"), "en");
    }

    #[test]
    fn uscita_della_modalita_comando() {
        let r = parse_command_output("TIPO: edit_text\nTesto corto.", "Testo lungo e prolisso.");
        assert_eq!(r, CommandResult { kind: CommandKind::EditText, text: "Testo corto.".into() });
        let r = parse_command_output("Testo di prova della bozza.", "");
        assert_eq!(r.kind, CommandKind::DraftText);
    }
}
