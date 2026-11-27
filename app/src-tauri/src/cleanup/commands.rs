//! Comandi Tauri del modulo `cleanup\` (da registrare in `lib.rs` dentro `collect_commands!`)
//! e funzione di avvio da chiamare nel `setup` dell'app.

use std::sync::Arc;
use tauri::{AppHandle, Emitter as _, Manager, State};

use super::{
    quality, CleanupContext, CleanupManager, CleanupSettings, CleanupState, CommandResult, DictionaryEntry,
    LearnedCandidate, EVENT_DICTIONARY_CHANGED,
};
use super::dictionary::LearnedWord;
use super::transforms::{TransformResult, TransformSlot, TransformsState};

/// Crea il modulo, lo mette nello stato di Tauri (`Arc<CleanupManager>`) e avvia llama-server.
pub fn setup(app: &AppHandle, settings: CleanupSettings) -> anyhow::Result<Arc<CleanupManager>> {
    let data_dir = crate::portable::app_data_dir(app)?;
    let handle = app.clone();
    let emitter: super::Emitter = Arc::new(move |event: &str, payload: serde_json::Value| {
        let _ = handle.emit(event, payload);
    });
    let mgr = CleanupManager::new(settings, &data_dir, emitter)?;
    mgr.start_engine();
    app.manage(Arc::clone(&mgr));
    Ok(mgr)
}

type Mgr<'a> = State<'a, Arc<CleanupManager>>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
#[specta::specta]
pub fn cleanup_get_state(mgr: Mgr<'_>) -> CleanupState {
    mgr.state()
}

#[tauri::command]
#[specta::specta]
pub fn cleanup_restart_engine(mgr: Mgr<'_>) {
    mgr.start_engine();
}

/// Legge `ANTHROPIC_API_KEY` dall'ambiente; restituisce solo se c'è.
#[tauri::command]
#[specta::specta]
pub fn cleanup_quality_key_present() -> bool {
    quality::key_present()
}

/// Prova dall'hub: pulisce un testo con il contesto dato.
#[tauri::command]
#[specta::specta]
pub async fn cleanup_preview(mgr: Mgr<'_>, raw: String, context: CleanupContext) -> Result<String, String> {
    let m = Arc::clone(&mgr);
    m.clean(&raw, &context, |_| {}).await.map(|c| c.text).map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn cleanup_polish(mgr: Mgr<'_>, text: String, instructions: Vec<String>) -> Result<String, String> {
    let m = Arc::clone(&mgr);
    m.polish(&text, &instructions).await.map_err(err)
}

#[tauri::command]
#[specta::specta]
pub async fn cleanup_command(mgr: Mgr<'_>, selected: String, spoken: String) -> Result<CommandResult, String> {
    let m = Arc::clone(&mgr);
    m.command(&selected, &spoken).await.map_err(err)
}

fn changed(app: &AppHandle) {
    let _ = app.emit(EVENT_DICTIONARY_CHANGED, serde_json::json!({}));
}

#[tauri::command]
#[specta::specta]
pub fn dictionary_list(mgr: Mgr<'_>) -> Result<Vec<DictionaryEntry>, String> {
    mgr.dictionary.list().map_err(err)
}

#[tauri::command]
#[specta::specta]
pub fn dictionary_add(app: AppHandle, mgr: Mgr<'_>, entry: DictionaryEntry) -> Result<DictionaryEntry, String> {
    let e = mgr.dictionary.add(entry).map_err(err)?;
    changed(&app);
    Ok(e)
}

#[tauri::command]
#[specta::specta]
pub fn dictionary_update(app: AppHandle, mgr: Mgr<'_>, entry: DictionaryEntry) -> Result<(), String> {
    mgr.dictionary.update(&entry).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn dictionary_delete(app: AppHandle, mgr: Mgr<'_>, id: String) -> Result<(), String> {
    mgr.dictionary.delete(&id).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn dictionary_candidates(mgr: Mgr<'_>) -> Result<Vec<LearnedCandidate>, String> {
    mgr.dictionary.candidates().map_err(err)
}

#[tauri::command]
#[specta::specta]
pub fn dictionary_accept_candidate(app: AppHandle, mgr: Mgr<'_>, id: String) -> Result<(), String> {
    mgr.dictionary.accept_candidate(&id).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn dictionary_reject_candidate(app: AppHandle, mgr: Mgr<'_>, id: String) -> Result<(), String> {
    mgr.dictionary.reject_candidate(&id).map_err(err)?;
    changed(&app);
    Ok(())
}

/// Import di un dizionario da un file JSON; restituisce le voci aggiunte.
#[tauri::command]
#[specta::specta]
pub fn dictionary_import_from_json(app: AppHandle, mgr: Mgr<'_>, path: String) -> Result<u32, String> {
    let n = mgr.dictionary.import_from_json(std::path::Path::new(&path)).map_err(err)?;
    changed(&app);
    Ok(n)
}

/// Parole imparate dalle correzioni (pagina "Imparate" dell'hub).
#[tauri::command]
#[specta::specta]
pub fn list_learned_words(mgr: Mgr<'_>) -> Result<Vec<LearnedWord>, String> {
    mgr.dictionary.learned_words().map_err(err)
}

/// "Annulla" dell'avviso di 6 s: la voce sparisce dal dizionario e dal prompt dell'ASR, e la
/// coppia non si reimpara.
#[tauri::command]
#[specta::specta]
pub fn undo_learned_word(app: AppHandle, mgr: Mgr<'_>, id: String) -> Result<(), String> {
    mgr.dictionary.undo_learned(&id).map_err(err)?;
    changed(&app);
    Ok(())
}

/// Toglie la parola imparata dalla lista e dal dizionario (potrà essere reimparata).
#[tauri::command]
#[specta::specta]
pub fn delete_learned_word(app: AppHandle, mgr: Mgr<'_>, id: String) -> Result<(), String> {
    mgr.dictionary.delete_learned(&id).map_err(err)?;
    changed(&app);
    Ok(())
}

#[tauri::command]
#[specta::specta]
pub fn get_transforms(mgr: Mgr<'_>) -> TransformsState {
    mgr.transforms_state()
}

#[tauri::command]
#[specta::specta]
pub fn save_transform(mgr: Mgr<'_>, slot: TransformSlot) -> Result<(), String> {
    mgr.transforms.save(slot)
}

#[tauri::command]
#[specta::specta]
pub fn delete_transform(mgr: Mgr<'_>, slot: u8) -> Result<(), String> {
    mgr.transforms.delete(slot)
}

#[tauri::command]
#[specta::specta]
pub fn reset_transforms(mgr: Mgr<'_>) {
    mgr.transforms.reset()
}

#[tauri::command]
#[specta::specta]
pub fn set_transforms_options(mgr: Mgr<'_>, enabled: bool, auto_apply_active: bool, auto_apply_slot: u8) -> Result<(), String> {
    mgr.transforms.set_options(enabled, auto_apply_active, auto_apply_slot)
}

/// Esegue un Transform (anche "Riprova" del diff). In errore il messaggio è da mostrare così.
#[tauri::command]
#[specta::specta]
pub async fn run_transform(mgr: Mgr<'_>, slot: u8, text: String, context: CleanupContext) -> Result<TransformResult, String> {
    let m = Arc::clone(&mgr);
    m.transform(slot, &text, &context).await.map_err(err)
}
