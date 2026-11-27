//! Taglio dell'audio in pezzi mentre arriva, sulle pause del VAD.
//!
//! Regola scelta dopo le prove sul motore: pezzi di al
//! massimo 25 s; quando il pezzo arriva al massimo si chiude a metà
//! dell'ultima pausa prima del massimo; senza pause, taglio netto. Così al
//! rilascio del tasto resta solo l'ultimo pezzo. In più, se
//! `early_silence_samples` è maggiore di zero, un pezzo lungo almeno
//! `min_piece_samples` si chiude anche quando il VAD vede un silenzio di
//! quella durata (opzione non ancora provata, spenta di default).
//!
//! Le probabilità di parlato vengono dal Silero di Handy, una per frame; la
//! segmentazione è quella di `audio_toolkit::chunking::speech_segments`.

use std::ops::Range;

use log::warn;

use crate::audio_toolkit::chunking::speech_segments;
use crate::audio_toolkit::SpeechScorer;

/// Frame nominale quando non c'è un VAD (taglio solo al massimo).
const BLIND_FRAME: usize = 512;

#[derive(Debug, Clone, Copy)]
pub struct CutterConfig {
    pub max_samples: usize,
    pub min_piece_samples: usize,
    pub early_silence_samples: usize,
}

impl CutterConfig {
    pub fn from_settings(s: &super::AsrSettings) -> Self {
        let rate = super::SAMPLE_RATE;
        Self {
            max_samples: (s.chunk_max_seconds.max(5) as usize) * rate,
            min_piece_samples: 3 * rate,
            early_silence_samples: s.chunk_min_silence_ms as usize * rate / 1000,
        }
    }
}

/// Un pezzo chiuso: posizione nel flusso, audio, quantità di parlato.
#[derive(Debug, Clone)]
pub struct Piece {
    pub start: usize,
    pub samples: Vec<f32>,
    pub speech_samples: usize,
    /// Campioni in coda al pezzo dopo la fine dell'ultimo parlato del VAD
    /// (tutto il pezzo se non c'è parlato, zero senza VAD).
    pub tail_silence_samples: usize,
}

pub struct PieceCutter {
    scorer: Option<Box<dyn SpeechScorer>>,
    frame: usize,
    cfg: CutterConfig,
    /// Audio dal `start` in poi, non ancora in un pezzo.
    pending: Vec<f32>,
    /// Una probabilità per frame intero di `pending`.
    probs: Vec<f32>,
    /// Campioni di `pending` già valutati (multiplo di `frame`).
    scored: usize,
    start: usize,
}

impl PieceCutter {
    /// Senza `scorer` il parlato non si misura (vale tutto il pezzo) e si
    /// taglia solo al massimo.
    pub fn new(scorer: Option<Box<dyn SpeechScorer>>, cfg: CutterConfig) -> Self {
        let frame = scorer
            .as_ref()
            .map(|s| s.frame_samples())
            .unwrap_or(BLIND_FRAME);
        Self {
            scorer,
            frame,
            cfg,
            pending: Vec::new(),
            probs: Vec::new(),
            scored: 0,
            start: 0,
        }
    }

    /// Campioni ricevuti in tutto.
    pub fn samples(&self) -> usize {
        self.start + self.pending.len()
    }

    /// Aggiunge audio; restituisce i pezzi che si sono chiusi.
    pub fn push(&mut self, pcm: &[f32]) -> Vec<Piece> {
        self.pending.extend_from_slice(pcm);
        self.score();
        let mut out = Vec::new();
        loop {
            if self.pending.len() >= self.cfg.max_samples {
                let cut = self.cut_at_max();
                out.push(self.take(cut));
            } else if let Some(cut) = self.early_cut() {
                out.push(self.take(cut));
            } else {
                break;
            }
        }
        out
    }

    /// L'ultimo pezzo, se c'è audio.
    pub fn finish(&mut self) -> Option<Piece> {
        if self.pending.is_empty() {
            return None;
        }
        let len = self.pending.len();
        Some(self.take(len))
    }

    fn score(&mut self) {
        let Some(scorer) = self.scorer.as_mut() else {
            return;
        };
        let whole = (self.pending.len() - self.scored) / self.frame * self.frame;
        if whole == 0 {
            return;
        }
        match scorer.score(&self.pending[self.scored..self.scored + whole]) {
            Ok(p) => {
                self.probs.extend(p);
                self.scored += whole;
            }
            Err(e) => {
                warn!("asr: VAD fallito ({e}), tagli solo al massimo");
                self.scorer = None;
            }
        }
    }

    fn blind(&self) -> bool {
        self.scorer.is_none()
    }

    fn segments(&self) -> Vec<Range<usize>> {
        speech_segments(&self.probs, self.frame, self.scored)
    }

    /// Metà dell'ultima pausa che cade fra il minimo e il massimo del pezzo;
    /// altrimenti il massimo.
    fn cut_at_max(&self) -> usize {
        let max = self.cfg.max_samples;
        if !self.blind() {
            let segs = self.segments();
            let mut best = None;
            for pair in segs.windows(2) {
                let mid = (pair[0].end + pair[1].start) / 2;
                if mid >= self.cfg.min_piece_samples && mid <= max {
                    best = Some(mid);
                }
            }
            if let Some(last) = segs.last() {
                let mid = (last.end + self.scored) / 2;
                if last.end < self.scored && mid >= self.cfg.min_piece_samples && mid <= max {
                    best = Some(best.map_or(mid, |b: usize| b.max(mid)));
                }
            }
            if let Some(mid) = best {
                return (mid / self.frame * self.frame).max(self.frame);
            }
        }
        // Allineato al frame, così le probabilità restano in passo col resto.
        (max / self.frame * self.frame).max(self.frame)
    }

    fn early_cut(&self) -> Option<usize> {
        let need = self.cfg.early_silence_samples;
        if need == 0 || self.blind() || self.scored < self.cfg.min_piece_samples {
            return None;
        }
        let segs = self.segments();
        let last = segs.last()?;
        if last.end >= self.scored || self.scored - last.end < need {
            return None;
        }
        let cut = (last.end + need / 2) / self.frame * self.frame;
        (cut >= self.cfg.min_piece_samples).then_some(cut)
    }

    fn take(&mut self, cut: usize) -> Piece {
        let cut = cut.min(self.pending.len());
        let (speech_samples, tail_silence_samples) = if self.blind() {
            (cut, 0)
        } else {
            let segs = self.segments();
            let speech = segs
                .iter()
                .map(|s| s.end.min(cut).saturating_sub(s.start))
                .sum();
            let last_end = segs
                .iter()
                .filter(|s| s.start < cut)
                .map(|s| s.end.min(cut))
                .max()
                .unwrap_or(0);
            (speech, cut - last_end)
        };
        let samples: Vec<f32> = self.pending.drain(..cut).collect();
        let frames = (cut / self.frame).min(self.probs.len());
        self.probs.drain(..frames);
        self.scored = self.scored.saturating_sub(frames * self.frame);
        // Un taglio non allineato (solo alla fine) lascia `pending` vuoto.
        if self.scored > self.pending.len() {
            self.scored = self.pending.len() / self.frame * self.frame;
            self.probs.truncate(self.scored / self.frame);
        }
        let piece = Piece {
            start: self.start,
            samples,
            speech_samples,
            tail_silence_samples,
        };
        self.start += cut;
        piece
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::audio_toolkit::vad::{VadFrame, VoiceActivityDetector};

    /// VAD finto per le prove: parlato quando l'energia del frame supera una soglia.
    pub(crate) struct EnergyScorer;

    impl VoiceActivityDetector for EnergyScorer {
        fn push_frame<'a>(&'a mut self, frame: &'a [f32]) -> anyhow::Result<VadFrame<'a>> {
            Ok(if self.score(frame)?[0] > 0.5 {
                VadFrame::Speech(frame)
            } else {
                VadFrame::Noise
            })
        }
        fn frame_samples(&self) -> usize {
            512
        }
    }

    impl SpeechScorer for EnergyScorer {
        fn score(&mut self, frames: &[f32]) -> anyhow::Result<Vec<f32>> {
            Ok(frames
                .chunks(512)
                .map(|f| {
                    let e = f.iter().map(|x| x * x).sum::<f32>() / f.len() as f32;
                    if e > 1e-3 {
                        1.0
                    } else {
                        0.0
                    }
                })
                .collect())
        }
    }

    pub(crate) fn tone(seconds: f32) -> Vec<f32> {
        (0..(seconds * 16_000.0) as usize)
            .map(|i| (i as f32 * 0.05).sin() * 0.3)
            .collect()
    }

    pub(crate) fn silence(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * 16_000.0) as usize]
    }

    fn cfg(max_s: usize, early_ms: usize) -> CutterConfig {
        CutterConfig {
            max_samples: max_s * 16_000,
            min_piece_samples: 3 * 16_000,
            early_silence_samples: early_ms * 16,
        }
    }

    #[test]
    fn taglia_nell_ultima_pausa_prima_del_massimo() {
        // 10 s parlato, 1 s pausa, 10 s parlato, 1 s pausa, 10 s parlato.
        let mut audio = tone(10.0);
        audio.extend(silence(1.0));
        audio.extend(tone(10.0));
        audio.extend(silence(1.0));
        audio.extend(tone(10.0));
        let mut c = PieceCutter::new(Some(Box::new(EnergyScorer)), cfg(25, 0));
        let mut pieces = Vec::new();
        for block in audio.chunks(640) {
            pieces.extend(c.push(block));
        }
        assert_eq!(pieces.len(), 1, "un pezzo chiuso durante la registrazione");
        let first = &pieces[0];
        // Il taglio cade nella seconda pausa (21-22 s), non a 25 s.
        let cut_s = first.samples.len() as f32 / 16_000.0;
        assert!((21.0..=22.0).contains(&cut_s), "taglio a {cut_s} s");
        assert!(first.speech_samples >= 19 * 16_000);
        let last = c.finish().unwrap();
        assert_eq!(last.start, first.samples.len());
        assert_eq!(first.samples.len() + last.samples.len(), audio.len());
        assert!(c.finish().is_none());
    }

    #[test]
    fn senza_pause_taglio_netto_e_silenzio_senza_parlato() {
        let audio = tone(30.0);
        let mut c = PieceCutter::new(Some(Box::new(EnergyScorer)), cfg(25, 0));
        let pieces = c.push(&audio);
        assert_eq!(pieces.len(), 1);
        let cut_s = pieces[0].samples.len() as f32 / 16_000.0;
        assert!((24.9..=25.0).contains(&cut_s), "taglio a {cut_s} s");
        // Silenzio puro: il pezzo ha zero parlato.
        let mut s = PieceCutter::new(Some(Box::new(EnergyScorer)), cfg(25, 0));
        assert!(s.push(&silence(2.0)).is_empty());
        assert_eq!(s.finish().unwrap().speech_samples, 0);
    }

    #[test]
    fn silenzio_lungo_chiude_il_pezzo_prima_se_attivo() {
        let mut audio = tone(6.0);
        audio.extend(silence(1.0));
        audio.extend(tone(2.0));
        let mut c = PieceCutter::new(Some(Box::new(EnergyScorer)), cfg(25, 700));
        let mut pieces = Vec::new();
        for block in audio.chunks(640) {
            pieces.extend(c.push(block));
        }
        assert_eq!(pieces.len(), 1);
        let cut_s = pieces[0].samples.len() as f32 / 16_000.0;
        assert!((6.0..7.0).contains(&cut_s), "taglio a {cut_s} s");
        // Spento (0 ms): nessun taglio prima del massimo.
        let mut off = PieceCutter::new(Some(Box::new(EnergyScorer)), cfg(25, 0));
        assert!(off.push(&audio).is_empty());
    }
}
