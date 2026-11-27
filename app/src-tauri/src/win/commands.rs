//! Comandi Tauri del modulo `win\` (registrati in `lib.rs`).

use std::time::Duration;
use tauri::AppHandle;

use super::{AppContext, CategoryRule, PasteOutcome};

/// Diagnostica: contesto della finestra in primo piano (in pratica la finestra
/// di Handy stessa, se chiamato dall'interfaccia). Emette anche `win:context`.
#[tauri::command]
#[specta::specta]
pub fn win_capture_context_now(app: AppHandle) -> AppContext {
    let timeout = Duration::from_millis(super::current_settings().context_timeout_ms);
    #[cfg(target_os = "windows")]
    let ctx = super::context::capture_context(timeout);
    #[cfg(not(target_os = "windows"))]
    let ctx = {
        let _ = timeout;
        AppContext::default()
    };
    super::emit_context(&app, &ctx);
    ctx
}

#[tauri::command]
#[specta::specta]
pub fn win_get_category_table() -> Vec<CategoryRule> {
    super::current_settings().category_table
}

/// Sostituisce la tabella delle categorie e la salva tramite il persist hook.
#[tauri::command]
#[specta::specta]
pub fn win_set_category_table(app: AppHandle, table: Vec<CategoryRule>) -> Result<(), String> {
    if table.iter().any(|r| r.pattern.trim().is_empty()) {
        return Err("Ogni regola deve avere un pattern non vuoto".into());
    }
    let mut settings = super::current_settings();
    settings.category_table = table;
    super::apply_settings(settings.clone());
    super::persist(&app, &settings);
    Ok(())
}

/// Incolla di prova nella finestra attiva; emette `win:paste-outcome`.
#[tauri::command]
#[specta::specta]
pub fn win_test_paste(app: AppHandle, text: String) -> PasteOutcome {
    #[cfg(target_os = "windows")]
    let outcome = {
        let timeout = Duration::from_millis(super::current_settings().context_timeout_ms);
        let ctx = super::context::capture_context(timeout);
        super::paste::paste_text(&text, &ctx, false)
    };
    #[cfg(not(target_os = "windows"))]
    let outcome = PasteOutcome {
        success: false,
        reason: Some(format!("non supportato su questo sistema ({} caratteri)", text.len())),
        elapsed_ms: 0,
        method: crate::settings::PasteMethod::CtrlV,
        clipboard_read_ms: None,
        press_enter_sent: false,
        focus_restored: false,
    };
    super::emit_paste_outcome(&app, &outcome);
    outcome
}
