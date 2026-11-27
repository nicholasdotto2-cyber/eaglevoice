//! Modalità qualità: API Anthropic Messages in streaming, stesso prompt del motore locale.
//!
//! Parametri dalla doc letta il 2026-10-18:
//! - https://platform.claude.com/docs/en/build-with-claude/streaming: `POST /v1/messages` con
//!   `"stream": true`, header `x-api-key`, `anthropic-version: 2023-06-01`; eventi SSE
//!   `message_start`, `content_block_start`, `content_block_delta` (`text_delta`),
//!   `content_block_stop`, `message_delta`, `message_stop`, più `ping` ed `error`; tipi nuovi
//!   da ignorare.
//! - https://platform.claude.com/docs/en/build-with-claude/effort: `output_config.effort`
//!   (`low` per compiti semplici e veloci); su Claude Haiku 5.5 `thinking: {"type": "disabled"}`
//!   è ammesso fino a `high`; su Claude Sonnet 5.5 il thinking iniziale si spegne con
//!   `{"type": "between_tools"}` (ammesso a low, medium, high).
//! - https://platform.claude.com/docs/en/build-with-claude/prompt-caching: `cache_control`
//!   `ephemeral` sull'ultimo blocco di `system`; minimo 512 token su Haiku 5.5 e Sonnet 5.5,
//!   sotto il minimo nessun errore, solo niente cache.
//! - https://platform.claude.com/docs/en/models/overview: id `claude-haiku-5-5`, `claude-sonnet-5-5`.
//!
//! La chiave si legge solo da `ANTHROPIC_API_KEY` e non va mai nei log.

use futures_util::StreamExt;
use std::time::Duration;

pub const DEFAULT_QUALITY_MODEL: &str = "claude-haiku-5-5";
/// Polish e modalità comando con la modalità qualità attiva.
pub const HEAVY_QUALITY_MODEL: &str = "claude-sonnet-5-5";
pub const API_URL: &str = "https://api.anthropic.com/v1/messages";
pub const API_VERSION: &str = "2023-06-01";
pub const EFFORT: &str = "low";

#[derive(Debug, Clone, PartialEq)]
pub enum QualityError {
    KeyMissing,
    Network(String),
    Api(String),
}

impl std::fmt::Display for QualityError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            QualityError::KeyMissing => write!(f, "ANTHROPIC_API_KEY assente"),
            QualityError::Network(m) => write!(f, "rete: {m}"),
            QualityError::Api(m) => write!(f, "API: {m}"),
        }
    }
}

/// Chiave presente nell'ambiente (non restituisce mai il valore).
pub fn key_present() -> bool {
    std::env::var("ANTHROPIC_API_KEY").map(|v| !v.trim().is_empty()).unwrap_or(false)
}

/// Impostazione del thinking per modello, dalla doc sull'effort.
fn thinking_for(model: &str) -> Option<serde_json::Value> {
    if model.starts_with("claude-haiku") {
        Some(serde_json::json!({ "type": "disabled" }))
    } else if model.starts_with("claude-sonnet") {
        Some(serde_json::json!({ "type": "between_tools" }))
    } else {
        None
    }
}

pub fn request_body(model: &str, system: &str, user: &str, max_tokens: u32) -> serde_json::Value {
    let mut b = serde_json::json!({
        "model": model,
        "max_tokens": max_tokens,
        "stream": true,
        "system": [ { "type": "text", "text": system, "cache_control": { "type": "ephemeral" } } ],
        "messages": [ { "role": "user", "content": user } ],
        "output_config": { "effort": EFFORT }
    });
    if let Some(t) = thinking_for(model) {
        b["thinking"] = t;
    }
    b
}

#[derive(Debug, PartialEq)]
pub enum Event {
    Text(String),
    Stop,
    Error(String),
    Other,
}

/// Una riga `data:` dello stream Anthropic.
pub fn parse_data(data: &str) -> Event {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(data.trim()) else { return Event::Other };
    match v.get("type").and_then(|t| t.as_str()) {
        Some("content_block_delta") => {
            let d = &v["delta"];
            if d.get("type").and_then(|t| t.as_str()) == Some("text_delta") {
                Event::Text(d.get("text").and_then(|t| t.as_str()).unwrap_or("").to_string())
            } else {
                Event::Other
            }
        }
        Some("message_stop") => Event::Stop,
        Some("error") => Event::Error(
            v["error"].get("type").and_then(|t| t.as_str()).unwrap_or("error").to_string(),
        ),
        _ => Event::Other,
    }
}

/// Chiamata in streaming. `on_partial` riceve il testo accumulato.
pub async fn stream_message(
    model: &str,
    system: &str,
    user: &str,
    max_tokens: u32,
    on_partial: &mut (dyn FnMut(&str) + Send),
) -> Result<String, QualityError> {
    let key = std::env::var("ANTHROPIC_API_KEY").unwrap_or_default();
    if key.trim().is_empty() {
        return Err(QualityError::KeyMissing);
    }
    let client = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(3))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| QualityError::Network(e.to_string()))?;
    let resp = client
        .post(API_URL)
        .header("x-api-key", key.trim())
        .header("anthropic-version", API_VERSION)
        .header("content-type", "application/json")
        .json(&request_body(model, system, user, max_tokens))
        .send()
        .await
        .map_err(|e| QualityError::Network(short_err(&e)))?;
    if !resp.status().is_success() {
        let st = resp.status().as_u16();
        // il corpo dell'errore non contiene la chiave; si tiene corto
        let body = resp.text().await.unwrap_or_default();
        return Err(QualityError::Api(format!("{st} {}", body.chars().take(200).collect::<String>())));
    }
    let mut stream = resp.bytes_stream();
    let mut buf: Vec<u8> = Vec::new();
    let mut text = String::new();
    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| QualityError::Network(short_err(&e)))?;
        buf.extend_from_slice(&chunk);
        while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = buf.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let Some(data) = line.trim().strip_prefix("data:") else { continue };
            match parse_data(data) {
                Event::Text(t) => {
                    text.push_str(&t);
                    on_partial(&text);
                }
                Event::Stop => break 'outer,
                Event::Error(e) => return Err(QualityError::Api(e)),
                Event::Other => {}
            }
        }
    }
    Ok(text)
}

fn short_err(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "timeout".into()
    } else if e.is_connect() {
        "connect".into()
    } else {
        "request".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn richiesta_haiku_senza_thinking_effort_basso() {
        let b = request_body(DEFAULT_QUALITY_MODEL, "sys", "testo", 256);
        assert_eq!(b["model"], "claude-haiku-5-5");
        assert_eq!(b["stream"], true);
        assert_eq!(b["thinking"]["type"], "disabled");
        assert_eq!(b["output_config"]["effort"], "low");
        assert_eq!(b["system"][0]["cache_control"]["type"], "ephemeral");
        let s = request_body(HEAVY_QUALITY_MODEL, "sys", "testo", 256);
        assert_eq!(s["thinking"]["type"], "between_tools");
    }

    #[test]
    fn eventi_dello_stream() {
        assert_eq!(
            parse_data(r#"{"type": "content_block_delta","index": 0,"delta": {"type": "text_delta", "text": "ello"}}"#),
            Event::Text("ello".into())
        );
        assert_eq!(parse_data(r#"{"type": "ping"}"#), Event::Other);
        assert_eq!(parse_data(r#"{"type": "message_stop"}"#), Event::Stop);
        assert_eq!(
            parse_data(r#"{"type": "error", "error": {"type": "overloaded_error", "message": "Overloaded"}}"#),
            Event::Error("overloaded_error".into())
        );
    }
}
