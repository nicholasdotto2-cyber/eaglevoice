//! Motore di riserva: il `TranscriptionManager` di Handy (transcribe-cpp
//! in-process, CPU), con il modello scelto nel catalogo di Handy (Parakeet TDT
//! 0.6B v3 Q4_K_M: riconosce meno termini del motore principale, quindi
//! solo riserva). Niente prompt: il vocabolario qui non entra.

use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};

use super::{AsrEngine, AsrEngineKind, AsrState, EngineStatus, RawTranscript, SessionConfig, Segment};
use crate::managers::transcription::TranscriptionManager;

pub struct HandyInprocess {
    tm: Arc<TranscriptionManager>,
    buffer: Mutex<Option<Vec<f32>>>,
}

impl HandyInprocess {
    pub fn new(tm: Arc<TranscriptionManager>) -> Self {
        Self {
            tm,
            buffer: Mutex::new(None),
        }
    }
}

fn to_raw(text: String, samples: usize) -> RawTranscript {
    let text = text.trim().to_string();
    let segments = if text.is_empty() {
        Vec::new()
    } else {
        vec![Segment {
            start_ms: 0,
            end_ms: (samples as u64 * 1000 / 16_000) as u32,
            text: text.clone(),
        }]
    };
    RawTranscript {
        text,
        language: None,
        segments,
    }
}

impl AsrEngine for HandyInprocess {
    fn start_session(&self, _cfg: SessionConfig) -> Result<()> {
        self.tm.initiate_model_load();
        *self.buffer.lock().unwrap() = Some(Vec::with_capacity(16_000 * 60));
        Ok(())
    }

    fn feed(&self, samples: &[f32]) {
        if let Some(b) = self.buffer.lock().unwrap().as_mut() {
            b.extend_from_slice(samples);
        }
    }

    fn finish(&self) -> Result<RawTranscript> {
        let samples = self
            .buffer
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow!("nessuna sessione asr aperta"))?;
        let n = samples.len();
        Ok(to_raw(self.tm.transcribe(samples)?, n))
    }

    fn cancel(&self) {
        self.buffer.lock().unwrap().take();
    }

    /// Il modello si carica su richiesta: la riserva è sempre disponibile.
    fn is_ready(&self) -> bool {
        true
    }

    fn state(&self) -> AsrState {
        AsrState {
            state: if self.tm.is_model_loaded() {
                EngineStatus::Ready
            } else {
                EngineStatus::Stopped
            },
            detail: None,
            model: self.tm.get_current_model(),
            vram_mb: None,
            engine: AsrEngineKind::HandyInprocess,
            port: None,
        }
    }

    fn transcribe_clip(&self, samples: Vec<f32>, _cfg: SessionConfig) -> Result<RawTranscript> {
        let n = samples.len();
        Ok(to_raw(self.tm.transcribe(samples)?, n))
    }
}
