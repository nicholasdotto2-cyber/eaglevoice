//! Comandi Tauri del modulo: si registrano in `lib.rs`.
//! Lo stato atteso è `SharedAsr` (`Arc<dyn AsrEngine>`) gestito con `app.manage`.

use tauri::State;

use super::{AsrState, SessionConfig, SharedAsr};

#[tauri::command]
#[specta::specta]
pub fn asr_get_state(engine: State<'_, SharedAsr>) -> AsrState {
    engine.state()
}

#[tauri::command]
#[specta::specta]
pub fn asr_restart_engine(engine: State<'_, SharedAsr>) -> Result<(), String> {
    engine.restart().map_err(|e| e.to_string())
}

/// Trascrive un WAV 16 kHz mono (prove, "ritenta" della cronologia).
#[tauri::command]
#[specta::specta]
pub async fn asr_transcribe_file(
    engine: State<'_, SharedAsr>,
    path: String,
    vocabulary: Vec<String>,
) -> Result<String, String> {
    let engine = SharedAsr::clone(&engine);
    tokio::task::spawn_blocking(move || {
        let samples =
            crate::audio_toolkit::read_wav_samples(&path).map_err(|e| format!("{path}: {e}"))?;
        engine
            .transcribe_clip(
                samples,
                SessionConfig {
                    vocabulary,
                    language: None,
                },
            )
            .map(|t| t.text)
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}
