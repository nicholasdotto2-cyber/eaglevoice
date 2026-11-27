//! Una dettatura: pezzi tagliati dal VAD, decodificati uno alla volta mentre
//! la registrazione continua, testi uniti alla fine.
//!
//! `feed` manda i campioni a un thread della sessione e non blocca il
//! recorder. Il thread taglia, decodifica ogni pezzo chiuso ed emette
//! `asr:partial-text`; `finish` gli fa chiudere l'ultimo pezzo e aspetta.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::JoinHandle;
use std::time::Instant;

use anyhow::{anyhow, Result};
use log::{debug, info, warn};

use super::filtri::{filter_hallucinations, strip_unsupported_closing, PieceVoice};
use super::pezzi::{CutterConfig, Piece, PieceCutter};
use super::prompt::{build_prompt, merge_vocabulary};
use super::{
    AsrEvents, AsrSettings, RawTranscript, ScorerFactory, Segment, SessionConfig,
    EVENT_LANGUAGE_DETECTED, EVENT_PARTIAL_TEXT, SAMPLE_RATE,
};

/// Testo di un pezzo e lingua rilevata dal motore, se richiesta.
#[derive(Debug, Clone, Default)]
pub struct DecodedPiece {
    pub text: String,
    pub language: Option<String>,
}

/// Chi trascrive un pezzo: whisper-server, o un finto nelle prove.
/// `language = None` chiede il rilevamento.
pub trait PieceDecoder: Send + Sync {
    fn decode(&self, samples: &[f32], prompt: &str, language: Option<&str>) -> Result<DecodedPiece>;
}

#[derive(Debug, Clone)]
pub struct SessionParams {
    pub vocabulary: Vec<String>,
    /// Lingua forzata; `None` con `detect_among` pieno: rilevamento sul primo pezzo.
    pub language: Option<String>,
    pub detect_among: Vec<String>,
    pub primary_language: String,
    pub carry_prompt: bool,
    pub prompt_max_tokens: usize,
    pub min_speech_samples: usize,
    pub live: bool,
    pub cutter: CutterConfig,
}

impl SessionParams {
    pub fn new(s: &AsrSettings, cfg: &SessionConfig) -> Self {
        let forced = cfg
            .language
            .clone()
            .filter(|l| !l.is_empty() && l != "auto")
            .or_else(|| (!s.language_detection).then(|| s.primary_language.clone()));
        Self {
            vocabulary: cfg.vocabulary.clone(),
            language: forced,
            detect_among: s.languages.clone(),
            primary_language: s.primary_language.clone(),
            carry_prompt: s.carry_prompt,
            prompt_max_tokens: s.prompt_max_tokens as usize,
            min_speech_samples: s.min_speech_ms as usize * SAMPLE_RATE / 1000,
            live: s.chunked_decode,
            cutter: CutterConfig::from_settings(s),
        }
    }
}

fn ms(samples: usize) -> u32 {
    (samples as u64 * 1000 / SAMPLE_RATE as u64) as u32
}

/// Il lavoro di una sessione, senza thread: lo usano sia la sessione dal vivo
/// sia la trascrizione di un audio intero.
struct SessionCore {
    params: SessionParams,
    decoder: Arc<dyn PieceDecoder>,
    events: Arc<dyn AsrEvents>,
    cutter: PieceCutter,
    language: Option<String>,
    segments: Vec<Segment>,
    /// Audio trattenuto quando la decodifica dal vivo è spenta.
    held: Vec<Piece>,
    error: Option<anyhow::Error>,
    emit_partial: bool,
}

impl SessionCore {
    fn new(
        params: SessionParams,
        decoder: Arc<dyn PieceDecoder>,
        events: Arc<dyn AsrEvents>,
        make_scorer: Option<ScorerFactory>,
        emit_partial: bool,
    ) -> Self {
        let scorer = make_scorer.and_then(|f| match f() {
            Ok(s) => Some(s),
            Err(e) => {
                warn!("asr: VAD non caricato ({e}), tagli solo al massimo");
                None
            }
        });
        let language = params.language.clone();
        Self {
            cutter: PieceCutter::new(scorer, params.cutter),
            params,
            decoder,
            events,
            language,
            segments: Vec::new(),
            held: Vec::new(),
            error: None,
            emit_partial,
        }
    }

    fn joined(&self) -> String {
        self.segments
            .iter()
            .map(|s| s.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }

    fn push(&mut self, pcm: &[f32]) {
        for piece in self.cutter.push(pcm) {
            if self.params.live {
                self.process(piece);
            } else {
                self.held.push(piece);
            }
        }
    }

    fn process(&mut self, piece: Piece) {
        if self.error.is_some() {
            return;
        }
        let start_ms = ms(piece.start);
        let end_ms = ms(piece.start + piece.samples.len());
        if piece.speech_samples < self.params.min_speech_samples {
            debug!(
                "asr: pezzo {start_ms}-{end_ms} ms con {} ms di parlato, saltato",
                ms(piece.speech_samples)
            );
            return;
        }
        let previous = self.params.carry_prompt.then(|| self.joined());
        let prompt = build_prompt(
            &self.params.vocabulary,
            previous.as_deref().filter(|p| !p.is_empty()),
            self.params.prompt_max_tokens,
        );
        let detect = self.language.is_none() && !self.params.detect_among.is_empty();
        let lang = if detect {
            None
        } else {
            Some(
                self.language
                    .clone()
                    .unwrap_or_else(|| self.params.primary_language.clone()),
            )
        };
        let t0 = Instant::now();
        match self.decoder.decode(&piece.samples, &prompt, lang.as_deref()) {
            Ok(d) => {
                info!(
                    "asr: pezzo {} ({:.1} s di audio) trascritto in {} ms",
                    self.segments.len() + 1,
                    piece.samples.len() as f32 / SAMPLE_RATE as f32,
                    t0.elapsed().as_millis()
                );
                if detect {
                    let found = d.language.as_deref().and_then(language_code);
                    let chosen = match found {
                        Some(code) if self.params.detect_among.contains(&code) => code,
                        _ => self.params.primary_language.clone(),
                    };
                    self.events.emit_json(
                        EVENT_LANGUAGE_DETECTED,
                        serde_json::json!({ "language": chosen }),
                    );
                    self.language = Some(chosen);
                }
                let voice = PieceVoice {
                    piece_ms: ms(piece.samples.len()),
                    speech_ms: ms(piece.speech_samples),
                    tail_silence_ms: ms(piece.tail_silence_samples),
                };
                let text = strip_unsupported_closing(&filter_hallucinations(&d.text), voice);
                if !text.is_empty() {
                    self.segments.push(Segment {
                        start_ms,
                        end_ms,
                        text,
                    });
                    if self.emit_partial {
                        self.events
                            .emit_json(EVENT_PARTIAL_TEXT, serde_json::json!({ "text": self.joined() }));
                    }
                }
            }
            Err(e) => {
                warn!("asr: pezzo {start_ms}-{end_ms} ms fallito: {e}");
                self.error = Some(e);
            }
        }
    }

    fn finish(mut self) -> Result<RawTranscript> {
        for piece in std::mem::take(&mut self.held) {
            self.process(piece);
        }
        if let Some(last) = self.cutter.finish() {
            self.process(last);
        }
        if let Some(e) = self.error.take() {
            return Err(e);
        }
        let text = self.joined();
        Ok(RawTranscript {
            text,
            language: self.language.or(Some(self.params.primary_language)),
            segments: self.segments,
        })
    }
}

/// Normalizza la lingua rilevata al codice ISO 639-1. whisper-server
/// restituisce il nome lungo ("english"), altri motori il codice ("en").
/// Lingua sconosciuta: `None`, e la sessione ripiega sulla lingua principale.
pub fn language_code(raw: &str) -> Option<String> {
    let l = raw.trim().to_ascii_lowercase();
    let code = match l.as_str() {
        "english" => "en",
        "italian" => "it",
        "spanish" | "castilian" => "es",
        "french" => "fr",
        "german" => "de",
        "portuguese" => "pt",
        "dutch" | "flemish" => "nl",
        "russian" => "ru",
        "chinese" | "mandarin" => "zh",
        "japanese" => "ja",
        "korean" => "ko",
        "polish" => "pl",
        "turkish" => "tr",
        "arabic" => "ar",
        "swedish" => "sv",
        "danish" => "da",
        "norwegian" => "no",
        "finnish" => "fi",
        "greek" => "el",
        "czech" => "cs",
        "romanian" | "moldavian" | "moldovan" => "ro",
        "hungarian" => "hu",
        "ukrainian" => "uk",
        "catalan" | "valencian" => "ca",
        "hindi" => "hi",
        "hebrew" => "he",
        "croatian" => "hr",
        "slovenian" => "sl",
        "slovak" => "sk",
        "bulgarian" => "bg",
        "serbian" => "sr",
        "latin" => "la",
        c if c.len() == 2 && c.chars().all(|ch| ch.is_ascii_alphabetic()) => c,
        _ => return None,
    };
    Some(code.to_string())
}

enum Msg {
    Samples(Vec<f32>),
    Vocabulary(Vec<String>),
    Finish(mpsc::Sender<Result<RawTranscript>>),
}

/// Sessione dal vivo con il suo thread.
pub struct LiveSession {
    tx: mpsc::Sender<Msg>,
    cancelled: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl LiveSession {
    pub fn start(
        params: SessionParams,
        decoder: Arc<dyn PieceDecoder>,
        events: Arc<dyn AsrEvents>,
        make_scorer: Option<ScorerFactory>,
    ) -> Self {
        let (tx, rx) = mpsc::channel::<Msg>();
        let cancelled = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&cancelled);
        let handle = std::thread::Builder::new()
            .name("asr-sessione".into())
            .spawn(move || {
                let mut core = SessionCore::new(params, decoder, events, make_scorer, true);
                while let Ok(msg) = rx.recv() {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    match msg {
                        Msg::Samples(pcm) => core.push(&pcm),
                        Msg::Vocabulary(extra) => {
                            core.params.vocabulary =
                                merge_vocabulary(&core.params.vocabulary, &extra);
                            debug!(
                                "asr: vocabolario della sessione a {} termini",
                                core.params.vocabulary.len()
                            );
                        }
                        Msg::Finish(reply) => {
                            let _ = reply.send(core.finish());
                            return;
                        }
                    }
                }
            })
            .expect("thread della sessione asr");
        Self {
            tx,
            cancelled,
            handle: Some(handle),
        }
    }

    pub fn feed(&self, samples: &[f32]) {
        let _ = self.tx.send(Msg::Samples(samples.to_vec()));
    }

    /// Termini in più per i pezzi non ancora decodificati (in ordine con i
    /// campioni già mandati: vale dal primo pezzo che il thread chiude dopo).
    pub fn add_vocabulary(&self, extra: Vec<String>) {
        let _ = self.tx.send(Msg::Vocabulary(extra));
    }

    pub fn finish(mut self) -> Result<RawTranscript> {
        let (reply, rx) = mpsc::channel();
        self.tx
            .send(Msg::Finish(reply))
            .map_err(|_| anyhow!("sessione asr già chiusa"))?;
        let out = rx
            .recv()
            .map_err(|_| anyhow!("sessione asr interrotta"))?;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
        out
    }

    /// Annulla senza aspettare: un pezzo in corso finisce e viene scartato.
    pub fn cancel(self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

/// Un audio intero, con gli stessi pezzi e lo stesso prompt della sessione,
/// decodificato tutto adesso.
pub fn transcribe_clip(
    params: SessionParams,
    decoder: Arc<dyn PieceDecoder>,
    events: Arc<dyn AsrEvents>,
    make_scorer: Option<ScorerFactory>,
    samples: &[f32],
) -> Result<RawTranscript> {
    let mut core = SessionCore::new(params, decoder, events, make_scorer, false);
    core.push(samples);
    core.finish()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::asr::pezzi::tests::{silence, tone, EnergyScorer};
    use crate::asr::NoEvents;
    use std::sync::Mutex;

    /// Decoder finto: restituisce "pezzo N" e registra durata e prompt.
    #[derive(Default)]
    struct FakeDecoder {
        calls: Mutex<Vec<(usize, String, Option<String>)>>,
        /// Lingua restituita; `None` vale "en".
        lang: Option<&'static str>,
    }

    impl PieceDecoder for FakeDecoder {
        fn decode(&self, samples: &[f32], prompt: &str, language: Option<&str>) -> Result<DecodedPiece> {
            let mut c = self.calls.lock().unwrap();
            c.push((samples.len(), prompt.to_string(), language.map(str::to_string)));
            Ok(DecodedPiece {
                text: format!("pezzo {}", c.len()),
                language: Some(self.lang.unwrap_or("en").into()),
            })
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<(String, serde_json::Value)>>);
    impl AsrEvents for Recorder {
        fn emit_json(&self, event: &str, payload: serde_json::Value) {
            self.0.lock().unwrap().push((event.to_string(), payload));
        }
    }

    fn params(live: bool) -> SessionParams {
        let mut p = SessionParams::new(
            &AsrSettings::default(),
            &SessionConfig {
                vocabulary: vec!["Termineprova".into()],
                language: None,
            },
        );
        p.live = live;
        p
    }

    fn scorer() -> Option<ScorerFactory> {
        Some(Arc::new(|| Ok(Box::new(EnergyScorer) as Box<dyn crate::audio_toolkit::SpeechScorer>)))
    }

    #[test]
    fn pezzi_decodificati_durante_il_feed_e_uniti_alla_fine() {
        // 20 s parlato, 1 s pausa, 20 s parlato, 1 s pausa, 10 s parlato: 3 pezzi.
        let mut audio = tone(20.0);
        audio.extend(silence(1.0));
        audio.extend(tone(20.0));
        audio.extend(silence(1.0));
        audio.extend(tone(10.0));
        let dec = Arc::new(FakeDecoder::default());
        let ev = Arc::new(Recorder::default());
        let s = LiveSession::start(params(true), dec.clone(), ev.clone(), scorer());
        for block in audio.chunks(640) {
            s.feed(block);
        }
        // Prima di finish i primi due pezzi sono già passati dal motore.
        let deadline = Instant::now() + std::time::Duration::from_secs(5);
        while dec.calls.lock().unwrap().len() < 2 && Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(dec.calls.lock().unwrap().len(), 2);
        let out = s.finish().unwrap();
        assert_eq!(out.text, "pezzo 1 pezzo 2 pezzo 3");
        assert_eq!(out.segments.len(), 3);
        assert_eq!(out.language.as_deref(), Some("it"));
        assert!(out.segments[0].end_ms <= out.segments[1].start_ms);
        let calls = dec.calls.lock().unwrap();
        assert!(calls.iter().all(|c| c.2.as_deref() == Some("it")));
        assert!(calls[0].1.ends_with("sempre di Termineprova."));
        assert_eq!(calls.iter().map(|c| c.0).sum::<usize>(), audio.len());
        let partial: Vec<_> = ev
            .0
            .lock()
            .unwrap()
            .iter()
            .filter(|(e, _)| e == EVENT_PARTIAL_TEXT)
            .map(|(_, p)| p["text"].as_str().unwrap().to_string())
            .collect();
        assert_eq!(partial, vec!["pezzo 1", "pezzo 1 pezzo 2", "pezzo 1 pezzo 2 pezzo 3"]);
    }

    #[test]
    fn asr_nomi_dello_schermo_prima_del_dizionario_e_dentro_il_tetto() {
        let dec = Arc::new(FakeDecoder::default());
        let s = LiveSession::start(params(true), dec.clone(), Arc::new(NoEvents), scorer());
        s.feed(&tone(1.0));
        // Doppione di "Termineprova" in minuscolo e nome nuovo dello schermo.
        s.add_vocabulary(vec!["termineprova".into(), "Nome Esempio".into()]);
        s.feed(&tone(1.0));
        s.finish().unwrap();
        assert_eq!(
            dec.calls.lock().unwrap()[0].1,
            "Oggi parlo di Nome Esempio, e poi di Termineprova, sempre di Termineprova."
        );

        // Tanti nomi a schermo: il prompt resta nel tetto e il dizionario in coda.
        let dec = Arc::new(FakeDecoder::default());
        let s = LiveSession::start(params(true), dec.clone(), Arc::new(NoEvents), scorer());
        s.add_vocabulary((0..300).map(|i| format!("Nome{i:03}")).collect());
        s.feed(&tone(1.0));
        s.finish().unwrap();
        let p = dec.calls.lock().unwrap()[0].1.clone();
        let tokens = crate::asr::prompt::estimate_tokens(&p);
        assert!(tokens <= 224, "{tokens} token");
        assert!(p.ends_with("sempre di Termineprova.") && p.contains("Nome000"), "{p}");
        assert!(!p.contains("Nome299"));
    }

    #[test]
    fn audio_muto_non_chiama_il_motore() {
        let dec = Arc::new(FakeDecoder::default());
        let out = transcribe_clip(params(true), dec.clone(), Arc::new(NoEvents), scorer(), &silence(3.0))
            .unwrap();
        assert_eq!(out.text, "");
        assert!(dec.calls.lock().unwrap().is_empty());
        // 0,2 s di parlato: sotto la soglia di 0,3 s.
        let mut short = silence(1.0);
        short.extend(tone(0.2));
        short.extend(silence(1.0));
        transcribe_clip(params(true), dec.clone(), Arc::new(NoEvents), scorer(), &short).unwrap();
        assert!(dec.calls.lock().unwrap().is_empty());
    }

    /// Decoder finto che risponde sempre lo stesso testo.
    struct TextDecoder(&'static str);
    impl PieceDecoder for TextDecoder {
        fn decode(&self, _: &[f32], _: &str, _: Option<&str>) -> Result<DecodedPiece> {
            Ok(DecodedPiece {
                text: self.0.to_string(),
                language: Some("it".into()),
            })
        }
    }

    fn clip(text: &'static str, audio: &[f32]) -> String {
        transcribe_clip(params(true), Arc::new(TextDecoder(text)), Arc::new(NoEvents), scorer(), audio)
            .unwrap()
            .text
    }

    #[test]
    fn grazie_detto_con_la_voce_in_fondo_resta() {
        // Voce fino alla fine, poi la coda del VAD di cattura (450 ms).
        let mut audio = tone(3.0);
        audio.extend(silence(0.45));
        assert_eq!(clip("Testo di prova. Grazie.", &audio), "Testo di prova. Grazie.");
        // "Grazie." da solo, detto davvero: 0,5 s di voce fra prefill e coda.
        let mut alone = silence(0.45);
        alone.extend(tone(0.5));
        alone.extend(silence(0.45));
        assert_eq!(clip("Grazie.", &alone), "Grazie.");
    }

    #[test]
    fn grazie_inventato_senza_voce_si_toglie() {
        // Coda di 1,5 s senza voce: la chiusura non viene dall'audio.
        let mut audio = tone(3.0);
        audio.extend(silence(1.5));
        assert_eq!(clip("Testo di prova. Grazie.", &audio), "Testo di prova.");
        assert_eq!(clip("Testo di prova, grazie a tutti!", &audio), "Testo di prova,");
        // Pezzo sotto la soglia di voce (0,4 s su 2,7 s), coda corta: nulla resta.
        let mut weak = silence(2.0);
        weak.extend(tone(0.4));
        weak.extend(silence(0.3));
        assert_eq!(clip("Grazie.", &weak), "");
        // Una parola che finisce per "grazie" non è la chiusura.
        assert_eq!(clip("Testo ringrazie", &audio), "Testo ringrazie");
    }

    /// Rilevamento con la lingua principale `primary` e un decoder che
    /// risponde `lang`: restituisce la lingua finale e quella del secondo pezzo.
    fn detected_with(primary: &str, lang: &'static str) -> (Option<String>, Option<String>) {
        let mut settings = AsrSettings::default();
        settings.language_detection = true;
        settings.primary_language = primary.into();
        let p = SessionParams::new(&settings, &SessionConfig::default());
        let dec = Arc::new(FakeDecoder { lang: Some(lang), ..Default::default() });
        let mut audio = tone(20.0);
        audio.extend(silence(1.0));
        audio.extend(tone(10.0));
        let out = transcribe_clip(p, dec.clone(), Arc::new(NoEvents), scorer(), &audio).unwrap();
        let second = dec.calls.lock().unwrap()[1].2.clone();
        (out.language, second)
    }

    #[test]
    fn lingua_rilevata_english_diventa_en() {
        // whisper-server risponde col nome lungo; la principale "it" non deve vincere.
        assert_eq!(detected_with("it", "english"), (Some("en".into()), Some("en".into())));
    }

    #[test]
    fn lingua_rilevata_italian_diventa_it() {
        // Principale "en": se il nome lungo non venisse riconosciuto uscirebbe "en".
        assert_eq!(detected_with("en", "italian"), (Some("it".into()), Some("it".into())));
    }

    #[test]
    fn rilevamento_della_lingua_sul_primo_pezzo() {
        let mut settings = AsrSettings::default();
        settings.language_detection = true;
        let p = SessionParams::new(&settings, &SessionConfig::default());
        let dec = Arc::new(FakeDecoder::default());
        let ev = Arc::new(Recorder::default());
        let mut audio = tone(20.0);
        audio.extend(silence(1.0));
        audio.extend(tone(10.0));
        let out = transcribe_clip(p, dec.clone(), ev.clone(), scorer(), &audio).unwrap();
        assert_eq!(out.language.as_deref(), Some("en"));
        let calls = dec.calls.lock().unwrap();
        assert_eq!(calls[0].2, None, "primo pezzo in rilevamento");
        assert_eq!(calls[1].2.as_deref(), Some("en"), "poi lingua fissata");
        assert!(ev.0.lock().unwrap().iter().any(|(e, _)| e == EVENT_LANGUAGE_DETECTED));
    }
}
