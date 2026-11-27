//! Categoria dell'app attiva da una tabella configurabile, e
//! riconoscimento dei terminali. Funzioni pure: niente Win32 qui.

use serde::{Deserialize, Serialize};
use specta::Type;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, Type)]
pub enum AppCategory {
    #[serde(rename = "email")]
    Email,
    #[serde(rename = "chat_personal")]
    ChatPersonale,
    #[serde(rename = "chat_work")]
    ChatLavoro,
    #[serde(rename = "ai")]
    Ai,
    #[serde(rename = "terminal")]
    Terminale,
    #[serde(rename = "document")]
    Documento,
    #[serde(rename = "code")]
    Codice,
    #[serde(rename = "browser")]
    Browser,
    #[default]
    #[serde(rename = "other")]
    Altro,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Type)]
#[serde(rename_all = "snake_case")]
pub enum MatchKind {
    /// Nome dell'eseguibile, senza distinzione di maiuscole; ".exe" facoltativo.
    Process,
    /// Sottostringa dell'URL, senza distinzione di maiuscole.
    Url,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Type)]
pub struct CategoryRule {
    pub match_kind: MatchKind,
    pub pattern: String,
    pub category: AppCategory,
}

fn rule(kind: MatchKind, pattern: &str, category: AppCategory) -> CategoryRule {
    CategoryRule {
        match_kind: kind,
        pattern: pattern.to_string(),
        category,
    }
}

/// Tabella di default con le app più comuni per scrivere su Windows.
/// Le regole URL vincono sulle regole per processo (sono più specifiche).
pub fn default_category_table() -> Vec<CategoryRule> {
    use AppCategory::*;
    use MatchKind::*;
    vec![
        rule(Url, "chatgpt.com", Ai),
        rule(Url, "claude.ai", Ai),
        rule(Url, "perplexity.ai", Ai),
        rule(Url, "gemini.google.com", Ai),
        rule(Url, "mail.google.com", Email),
        rule(Url, "gmail.com", Email),
        rule(Url, "outlook.", Email),
        rule(Url, "docs.google.com", Documento),
        rule(Url, "web.whatsapp.com", ChatPersonale),
        rule(Process, "claude.exe", Ai),
        rule(Process, "ChatGPT.exe", Ai),
        rule(Process, "Code.exe", Codice),
        rule(Process, "comet.exe", Browser),
        rule(Process, "chrome.exe", Browser),
        rule(Process, "msedge.exe", Browser),
        rule(Process, "brave.exe", Browser),
        rule(Process, "firefox.exe", Browser),
        rule(Process, "WindowsTerminal.exe", Terminale),
        rule(Process, "cmd.exe", Terminale),
        rule(Process, "powershell.exe", Terminale),
        rule(Process, "pwsh.exe", Terminale),
        rule(Process, "mintty.exe", Terminale),
        rule(Process, "conhost.exe", Terminale),
        rule(Process, "OpenConsole.exe", Terminale),
        rule(Process, "WhatsApp.exe", ChatPersonale),
        rule(Process, "WhatsApp.Root.exe", ChatPersonale),
        rule(Process, "WINWORD.EXE", Documento),
        rule(Process, "OUTLOOK.EXE", Email),
        rule(Process, "olk.exe", Email),
    ]
}

/// Eseguibili trattati come terminali (Ctrl+Shift+V, campo non letto).
pub fn default_terminal_processes() -> Vec<String> {
    [
        "WindowsTerminal.exe",
        "cmd.exe",
        "powershell.exe",
        "pwsh.exe",
        "mintty.exe",
        "conhost.exe",
        "OpenConsole.exe",
        "wezterm-gui.exe",
        "alacritty.exe",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// Classi di finestra dei terminali noti.
pub const TERMINAL_WINDOW_CLASSES: &[&str] = &[
    "ConsoleWindowClass",
    "CASCADIA_HOSTING_WINDOW_CLASS",
    "PseudoConsoleWindow",
    "mintty",
];

/// Browser di cui si prova a leggere la barra degli indirizzi.
pub const BROWSER_PROCESSES: &[&str] = &[
    "chrome.exe",
    "msedge.exe",
    "comet.exe",
    "brave.exe",
    "firefox.exe",
    "opera.exe",
    "vivaldi.exe",
];

fn exe_matches(pattern: &str, exe: &str) -> bool {
    let p = pattern.trim().to_lowercase();
    let e = exe.trim().to_lowercase();
    if p.is_empty() {
        return false;
    }
    let p_stem = p.strip_suffix(".exe").unwrap_or(&p);
    let e_stem = e.strip_suffix(".exe").unwrap_or(&e);
    p_stem == e_stem
}

/// Nome del file dell'eseguibile da un percorso completo.
pub fn exe_file_name(path: &str) -> &str {
    path.rsplit(['\\', '/']).next().unwrap_or(path)
}

/// Categoria per processo e URL: prima le regole URL, poi quelle per processo.
pub fn classify(table: &[CategoryRule], process_exe: &str, url: Option<&str>) -> AppCategory {
    let exe = exe_file_name(process_exe);
    if let Some(url) = url {
        let u = url.to_lowercase();
        for r in table.iter().filter(|r| r.match_kind == MatchKind::Url) {
            let p = r.pattern.trim().to_lowercase();
            if !p.is_empty() && u.contains(&p) {
                return r.category;
            }
        }
    }
    for r in table.iter().filter(|r| r.match_kind == MatchKind::Process) {
        if exe_matches(&r.pattern, exe) {
            return r.category;
        }
    }
    AppCategory::Altro
}

/// Terminale per classe di finestra o per eseguibile in tabella.
pub fn is_terminal(window_class: &str, process_exe: &str, terminal_processes: &[String]) -> bool {
    if TERMINAL_WINDOW_CLASSES
        .iter()
        .any(|c| c.eq_ignore_ascii_case(window_class.trim()))
    {
        return true;
    }
    let exe = exe_file_name(process_exe);
    terminal_processes.iter().any(|p| exe_matches(p, exe))
}

pub fn is_browser(process_exe: &str) -> bool {
    let exe = exe_file_name(process_exe);
    BROWSER_PROCESSES.iter().any(|b| exe_matches(b, exe))
}

/// Nome leggibile dell'app; per gli eseguibili sconosciuti il nome del file senza ".exe".
pub fn friendly_app_name(process_exe: &str) -> String {
    let exe = exe_file_name(process_exe);
    let known: &[(&str, &str)] = &[
        ("claude.exe", "Claude"),
        ("code.exe", "Visual Studio Code"),
        ("chatgpt.exe", "ChatGPT"),
        ("comet.exe", "Comet"),
        ("windowsterminal.exe", "Windows Terminal"),
        ("chrome.exe", "Google Chrome"),
        ("msedge.exe", "Microsoft Edge"),
        ("whatsapp.exe", "WhatsApp"),
        ("whatsapp.root.exe", "WhatsApp"),
        ("winword.exe", "Microsoft Word"),
        ("outlook.exe", "Outlook"),
        ("olk.exe", "Outlook"),
        ("notepad.exe", "Blocco note"),
        ("explorer.exe", "Esplora file"),
    ];
    let lower = exe.to_lowercase();
    for (k, v) in known {
        if *k == lower {
            return v.to_string();
        }
    }
    exe.strip_suffix(".exe")
        .or_else(|| exe.strip_suffix(".EXE"))
        .unwrap_or(exe)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifica_per_processo_e_url() {
        let t = default_category_table();
        assert_eq!(
            classify(&t, r"C:\Program Files\AnthropicClaude\app-1\claude.exe", None),
            AppCategory::Ai
        );
        assert_eq!(classify(&t, "Code.exe", None), AppCategory::Codice);
        assert_eq!(classify(&t, "code", None), AppCategory::Codice);
        assert_eq!(classify(&t, "comet.exe", None), AppCategory::Browser);
        assert_eq!(
            classify(&t, "comet.exe", Some("https://chatgpt.com/c/123")),
            AppCategory::Ai
        );
        assert_eq!(
            classify(&t, "comet.exe", Some("mail.google.com/mail/u/0/#inbox")),
            AppCategory::Email
        );
        assert_eq!(
            classify(&t, "msedge.exe", Some("https://docs.google.com/document/d/x")),
            AppCategory::Documento
        );
        assert_eq!(classify(&t, "pwsh.exe", None), AppCategory::Terminale);
        assert_eq!(classify(&t, "WindowsTerminal.exe", None), AppCategory::Terminale);
        assert_eq!(
            classify(&t, "comet.exe", Some("https://claude.ai/chat/abc")),
            AppCategory::Ai
        );
        assert_eq!(classify(&t, "WhatsApp.Root.exe", None), AppCategory::ChatPersonale);
        assert_eq!(classify(&t, "sconosciuto.exe", None), AppCategory::Altro);
        // Tabella personalizzata: una regola nuova vince.
        let mut t2 = t.clone();
        t2.insert(0, rule(MatchKind::Url, "github.com", AppCategory::Codice));
        assert_eq!(
            classify(&t2, "comet.exe", Some("https://github.com/x")),
            AppCategory::Codice
        );
    }

    #[test]
    fn riconosce_terminale_per_classe_o_processo() {
        let tp = default_terminal_processes();
        assert!(is_terminal("CASCADIA_HOSTING_WINDOW_CLASS", "qualsiasi.exe", &tp));
        assert!(is_terminal("ConsoleWindowClass", "", &tp));
        assert!(is_terminal("mintty", "", &tp));
        assert!(is_terminal("Altro", r"C:\Windows\System32\cmd.exe", &tp));
        assert!(!is_terminal("Chrome_WidgetWin_1", "Code.exe", &tp));
        assert!(!is_terminal("Chrome_WidgetWin_1", "claude.exe", &tp));
    }

    #[test]
    fn categorie_serializzate_con_i_valori_stabili() {
        let s = serde_json::to_string(&AppCategory::ChatPersonale).unwrap();
        assert_eq!(s, "\"chat_personal\"");
        let s = serde_json::to_string(&AppCategory::Altro).unwrap();
        assert_eq!(s, "\"other\"");
    }
}
