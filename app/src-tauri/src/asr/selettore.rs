//! Scelta del motore e passaggio alla riserva.
//!
//! Regole: si usa il principale se c'è, è pronto e non ha fallito due volte
//! di seguito; altrimenti la riserva. Se il principale fallisce a metà
//! dettatura, la stessa dettatura passa alla riserva con l'audio tenuto da
//! parte, così il testo non si perde. Dopo due fallimenti di seguito il
//! principale viene riavviato e si torna a lui quando è di nuovo pronto.
//! Ogni passaggio emette `asr:engine-state` con `detail`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, Result};
use log::{info, warn};

use super::{emit_state, AsrEngine, AsrEvents, AsrState, RawTranscript, SessionConfig, SharedAsr};

const FAILURES_BEFORE_SWITCH: u32 = 2;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Which {
    Primary,
    Backup,
}

struct Active {
    which: Which,
    cfg: SessionConfig,
    /// Copia dell'audio per salvare la dettatura con la riserva.
    samples: Vec<f32>,
}

pub struct FallbackEngine {
    primary: Option<SharedAsr>,
    backup: Option<SharedAsr>,
    events: Arc<dyn AsrEvents>,
    failures: AtomicU32,
    active: Mutex<Option<Active>>,
    last_used: Mutex<Option<Which>>,
}

impl FallbackEngine {
    pub fn new(primary: Option<SharedAsr>, backup: Option<SharedAsr>, events: Arc<dyn AsrEvents>) -> Self {
        Self {
            primary,
            backup,
            events,
            failures: AtomicU32::new(0),
            active: Mutex::new(None),
            last_used: Mutex::new(None),
        }
    }

    fn engine(&self, which: Which) -> Option<&SharedAsr> {
        match which {
            Which::Primary => self.primary.as_ref(),
            Which::Backup => self.backup.as_ref(),
        }
    }

    fn choose(&self) -> Result<Which> {
        let primary_ok = self
            .primary
            .as_ref()
            .is_some_and(|p| p.is_ready() && self.failures.load(Ordering::Acquire) < FAILURES_BEFORE_SWITCH);
        if primary_ok {
            return Ok(Which::Primary);
        }
        if self.backup.is_some() {
            return Ok(Which::Backup);
        }
        // Senza riserva si prova comunque il principale (darà errore chiaro).
        self.primary
            .as_ref()
            .map(|_| Which::Primary)
            .ok_or_else(|| anyhow!("nessun motore di trascrizione"))
    }

    fn note_switch(&self, which: Which, reason: &str) {
        let mut last = self.last_used.lock().unwrap();
        if *last == Some(which) {
            return;
        }
        let first = last.is_none();
        *last = Some(which);
        drop(last);
        if first && which == Which::Primary {
            return;
        }
        let mut state = self.engine(which).map(|e| e.state()).unwrap_or_default();
        state.detail = Some(match which {
            Which::Backup => format!("riserva attiva: {reason}"),
            Which::Primary => "motore principale di nuovo attivo".to_string(),
        });
        info!("asr: {}", state.detail.as_deref().unwrap_or(""));
        emit_state(self.events.as_ref(), &state);
    }

    fn reason(&self) -> String {
        match &self.primary {
            None => "scelta dalle impostazioni".into(),
            Some(_) if self.failures.load(Ordering::Acquire) >= FAILURES_BEFORE_SWITCH => {
                "motore principale fallito due volte di seguito".into()
            }
            Some(_) => "motore principale non pronto".into(),
        }
    }

    fn primary_failed(&self, e: &anyhow::Error) {
        let n = self.failures.fetch_add(1, Ordering::AcqRel) + 1;
        warn!("asr: motore principale fallito ({e}), {n} di seguito");
        if n >= FAILURES_BEFORE_SWITCH {
            if let Some(p) = &self.primary {
                // Riavvio: finché non torna pronto si usa la riserva.
                if let Err(e) = p.restart() {
                    warn!("asr: riavvio del principale fallito: {e}");
                }
                self.failures.store(0, Ordering::Release);
            }
        }
    }

    fn rescue(&self, samples: Vec<f32>, cfg: SessionConfig, why: &str) -> Result<RawTranscript> {
        let backup = self.backup.as_ref().ok_or_else(|| anyhow!("{why}, nessuna riserva"))?;
        self.note_switch(Which::Backup, why);
        backup.transcribe_clip(samples, cfg)
    }
}

impl AsrEngine for FallbackEngine {
    fn start_session(&self, cfg: SessionConfig) -> Result<()> {
        let which = self.choose()?;
        if which == Which::Backup {
            self.note_switch(which, &self.reason());
        }
        self.engine(which).expect("motore scelto").start_session(cfg.clone())?;
        *self.active.lock().unwrap() = Some(Active {
            which,
            cfg,
            samples: Vec::with_capacity(16_000 * 60),
        });
        Ok(())
    }

    fn feed(&self, samples: &[f32]) {
        let mut g = self.active.lock().unwrap();
        let Some(a) = g.as_mut() else { return };
        if a.which == Which::Primary && self.backup.is_some() {
            a.samples.extend_from_slice(samples);
        }
        if let Some(e) = self.engine(a.which) {
            e.feed(samples);
        }
    }

    fn finish(&self) -> Result<RawTranscript> {
        let a = self
            .active
            .lock()
            .unwrap()
            .take()
            .ok_or_else(|| anyhow!("nessuna sessione asr aperta"))?;
        let engine = self.engine(a.which).expect("motore scelto");
        match (a.which, engine.finish()) {
            (Which::Primary, Ok(t)) => {
                self.failures.store(0, Ordering::Release);
                self.note_switch(Which::Primary, "");
                Ok(t)
            }
            (Which::Primary, Err(e)) => {
                self.primary_failed(&e);
                if self.backup.is_none() {
                    return Err(e);
                }
                self.rescue(a.samples, a.cfg, &format!("motore principale fallito: {e}"))
            }
            (Which::Backup, r) => r,
        }
    }

    fn add_session_vocabulary(&self, extra: Vec<String>) {
        let mut g = self.active.lock().unwrap();
        let Some(a) = g.as_mut() else { return };
        // Anche nella configurazione salvata: il salvataggio con la riserva
        // dopo un fallimento usa lo stesso vocabolario.
        a.cfg.vocabulary = super::prompt::merge_vocabulary(&a.cfg.vocabulary, &extra);
        if let Some(e) = self.engine(a.which) {
            e.add_session_vocabulary(extra);
        }
    }

    fn cancel(&self) {
        if let Some(a) = self.active.lock().unwrap().take() {
            if let Some(e) = self.engine(a.which) {
                e.cancel();
            }
        }
    }

    fn is_ready(&self) -> bool {
        self.primary.as_ref().is_some_and(|p| p.is_ready())
            || self.backup.as_ref().is_some_and(|b| b.is_ready())
    }

    fn state(&self) -> AsrState {
        match self.choose() {
            Ok(Which::Backup) if self.primary.is_some() => {
                // Il principale non è pronto: si mostra il suo stato con la nota.
                let mut s = self.primary.as_ref().unwrap().state();
                s.detail = Some(format!(
                    "{} (riserva attiva)",
                    s.detail.unwrap_or_else(|| self.reason())
                ));
                s
            }
            Ok(w) => self.engine(w).map(|e| e.state()).unwrap_or_default(),
            Err(_) => AsrState::default(),
        }
    }

    fn transcribe_clip(&self, samples: Vec<f32>, cfg: SessionConfig) -> Result<RawTranscript> {
        match self.choose()? {
            Which::Primary => {
                let p = self.primary.as_ref().expect("principale");
                let copy = self.backup.is_some().then(|| samples.clone());
                match p.transcribe_clip(samples, cfg.clone()) {
                    Ok(t) => Ok(t),
                    Err(e) => match copy {
                        Some(s) => {
                            self.primary_failed(&e);
                            self.rescue(s, cfg, &format!("motore principale fallito: {e}"))
                        }
                        None => Err(e),
                    },
                }
            }
            Which::Backup => {
                self.note_switch(Which::Backup, &self.reason());
                self.backup.as_ref().expect("riserva").transcribe_clip(samples, cfg)
            }
        }
    }

    fn rewarm(&self) -> bool {
        self.primary.as_ref().is_some_and(|p| p.rewarm())
    }

    fn restart(&self) -> Result<()> {
        self.failures.store(0, Ordering::Release);
        match &self.primary {
            Some(p) => p.restart(),
            None => Ok(()),
        }
    }

    fn shutdown(&self) {
        for e in [&self.primary, &self.backup].into_iter().flatten() {
            e.shutdown();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::{AsrEngineKind, EngineStatus, EVENT_ENGINE_STATE};
    use std::sync::atomic::AtomicBool;

    /// Motore finto: pronto o no, fallisce o no, conta le chiamate.
    struct Fake {
        name: &'static str,
        ready: AtomicBool,
        fail: AtomicBool,
        fed: Mutex<usize>,
        vocab: Mutex<Vec<String>>,
        finished: AtomicU32,
        restarts: AtomicU32,
    }

    impl Fake {
        fn new(name: &'static str, ready: bool, fail: bool) -> Arc<Self> {
            Arc::new(Self {
                name,
                ready: AtomicBool::new(ready),
                fail: AtomicBool::new(fail),
                fed: Mutex::new(0),
                vocab: Mutex::new(Vec::new()),
                finished: AtomicU32::new(0),
                restarts: AtomicU32::new(0),
            })
        }
    }

    impl AsrEngine for Fake {
        fn start_session(&self, _cfg: SessionConfig) -> Result<()> {
            *self.fed.lock().unwrap() = 0;
            Ok(())
        }
        fn feed(&self, samples: &[f32]) {
            *self.fed.lock().unwrap() += samples.len();
        }
        fn finish(&self) -> Result<RawTranscript> {
            self.finished.fetch_add(1, Ordering::AcqRel);
            if self.fail.load(Ordering::Acquire) {
                return Err(anyhow!("{} rotto", self.name));
            }
            Ok(RawTranscript {
                text: self.name.into(),
                ..Default::default()
            })
        }
        fn cancel(&self) {}
        fn add_session_vocabulary(&self, extra: Vec<String>) {
            self.vocab.lock().unwrap().extend(extra);
        }
        fn is_ready(&self) -> bool {
            self.ready.load(Ordering::Acquire)
        }
        fn state(&self) -> AsrState {
            AsrState {
                state: if self.is_ready() { EngineStatus::Ready } else { EngineStatus::Starting },
                engine: AsrEngineKind::WhisperExternal,
                ..Default::default()
            }
        }
        fn transcribe_clip(&self, samples: Vec<f32>, _cfg: SessionConfig) -> Result<RawTranscript> {
            Ok(RawTranscript {
                text: format!("{} {}", self.name, samples.len()),
                ..Default::default()
            })
        }
        fn restart(&self) -> Result<()> {
            self.restarts.fetch_add(1, Ordering::AcqRel);
            Ok(())
        }
    }

    #[derive(Default)]
    struct Events(Mutex<Vec<serde_json::Value>>);
    impl AsrEvents for Events {
        fn emit_json(&self, event: &str, payload: serde_json::Value) {
            if event == EVENT_ENGINE_STATE {
                self.0.lock().unwrap().push(payload);
            }
        }
    }

    fn dictate(e: &FallbackEngine) -> Result<RawTranscript> {
        e.start_session(SessionConfig::default())?;
        e.feed(&[0.1; 1600]);
        e.finish()
    }

    #[test]
    fn asr_vocabolario_di_sessione_ignorato_a_sessione_chiusa() {
        let p = Fake::new("principale", true, false);
        let e = FallbackEngine::new(Some(p.clone()), None, Arc::new(Events::default()));
        // Nessuna sessione: non arriva al motore.
        e.add_session_vocabulary(vec!["Nome Esempio".into()]);
        assert!(p.vocab.lock().unwrap().is_empty());
        // Sessione aperta: arriva.
        e.start_session(SessionConfig::default()).unwrap();
        e.add_session_vocabulary(vec!["Nome Esempio".into()]);
        assert_eq!(*p.vocab.lock().unwrap(), vec!["Nome Esempio".to_string()]);
        // Dopo finish di nuovo chiusa.
        e.finish().unwrap();
        e.add_session_vocabulary(vec!["NomeProva".into()]);
        assert_eq!(p.vocab.lock().unwrap().len(), 1);
    }

    #[test]
    fn principale_pronto_poi_riserva_se_non_pronto() {
        let p = Fake::new("principale", true, false);
        let b = Fake::new("riserva", true, false);
        let ev = Arc::new(Events::default());
        let e = FallbackEngine::new(Some(p.clone()), Some(b.clone()), ev.clone());
        assert_eq!(dictate(&e).unwrap().text, "principale");
        p.ready.store(false, Ordering::Release);
        assert_eq!(dictate(&e).unwrap().text, "riserva");
        let details: Vec<String> = ev.0.lock().unwrap().iter().map(|v| v["detail"].as_str().unwrap_or("").to_string()).collect();
        assert!(details.iter().any(|d| d.contains("riserva attiva")), "{details:?}");
        // Solo riserva da impostazioni (principale assente).
        let only = FallbackEngine::new(None, Some(b.clone()), Arc::new(Events::default()));
        assert_eq!(dictate(&only).unwrap().text, "riserva");
    }

    #[test]
    fn fallimento_salvato_dalla_riserva_e_due_di_seguito_riavviano() {
        let p = Fake::new("principale", true, true);
        let b = Fake::new("riserva", true, false);
        let e = FallbackEngine::new(Some(p.clone()), Some(b.clone()), Arc::new(Events::default()));
        // Primo fallimento: la stessa dettatura esce dalla riserva con tutto l'audio.
        assert_eq!(dictate(&e).unwrap().text, "riserva 1600");
        assert_eq!(p.restarts.load(Ordering::Acquire), 0);
        // Secondo di seguito: riavvio del principale.
        assert_eq!(dictate(&e).unwrap().text, "riserva 1600");
        assert_eq!(p.restarts.load(Ordering::Acquire), 1);
        // Principale guarito: si torna a lui.
        p.fail.store(false, Ordering::Release);
        assert_eq!(dictate(&e).unwrap().text, "principale");
        // Senza riserva l'errore arriva al chiamante.
        let solo = FallbackEngine::new(Some(Fake::new("p", true, true)), None, Arc::new(Events::default()));
        assert!(dictate(&solo).is_err());
    }
}
