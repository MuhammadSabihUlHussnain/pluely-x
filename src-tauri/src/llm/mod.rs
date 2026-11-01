//! LLM proxy commands (contract §2). The webview builds Messages API bodies; Rust adds the
//! key, streams the SSE response back over a `Channel<LlmEvent>`, and supports cancel.

pub mod anthropic;
pub mod sse;

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Instant;

use base64::Engine;
use serde::Serialize;
use serde_json::Value;
use tauri::ipc::Channel;
use tauri::{AppHandle, Manager};
use tokio_util::sync::CancellationToken;

use crate::error::AppResult;
use crate::screen::store::ScreenshotStore;
use crate::secrets::{Provider, Secrets};
use anthropic::error_event;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Usage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_input_tokens: u64,
    pub cache_creation_input_tokens: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum LlmEvent {
    #[serde(rename_all = "camelCase")]
    Start {
        request_id: String,
        model: String,
    },
    Text {
        delta: String,
    },
    Thinking,
    #[serde(rename_all = "camelCase")]
    Stop {
        stop_reason: Option<String>,
        usage: Usage,
    },
    Error {
        code: String,
        message: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        status: Option<u16>,
        retryable: bool,
    },
}

/// Shared HTTP client (connection pooling for the API).
pub struct Http(pub reqwest::Client);

impl Http {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .connect_timeout(std::time::Duration::from_secs(10))
            .user_agent(concat!("PluelyX/", env!("CARGO_PKG_VERSION")))
            .build()
            .expect("reqwest client");
        Self(client)
    }
}

impl Default for Http {
    fn default() -> Self {
        Self::new()
    }
}

/// In-flight streams by requestId. The generation number lets a finished stream remove only
/// its own entry (a newer request may have reused the id).
#[derive(Default)]
pub struct LlmState {
    inflight: Mutex<HashMap<String, (u64, CancellationToken)>>,
    next_gen: std::sync::atomic::AtomicU64,
}

impl LlmState {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<String, (u64, CancellationToken)>> {
        self.inflight.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn register(&self, id: &str) -> (u64, CancellationToken) {
        let gen = self
            .next_gen
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let token = CancellationToken::new();
        if let Some((_, old)) = self.lock().insert(id.to_string(), (gen, token.clone())) {
            old.cancel(); // same id reused: the newer request wins
        }
        (gen, token)
    }

    fn finish(&self, id: &str, gen: u64) {
        let mut map = self.lock();
        if map.get(id).map(|(g, _)| *g == gen).unwrap_or(false) {
            map.remove(id);
        }
    }

    fn cancel(&self, id: &str) -> bool {
        match self.lock().get(id) {
            Some((_, t)) => {
                t.cancel();
                true
            }
            None => false,
        }
    }
}

fn valid_request_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 128 && id.chars().all(|c| c.is_ascii_graphic())
}

fn valid_beta(b: &str) -> bool {
    !b.is_empty()
        && b.len() <= 64
        && b.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
}

#[tauri::command]
pub async fn llm_stream(
    app: AppHandle,
    request_id: String,
    body: Value,
    betas: Option<Vec<String>>,
    on_event: Channel<LlmEvent>,
) -> AppResult<()> {
    let send = |ev: LlmEvent| on_event.send(ev).is_ok();
    if !valid_request_id(&request_id) {
        send(error_event("bad_request", "Invalid requestId", None, false));
        return Ok(());
    }
    let betas = betas.unwrap_or_default();
    if betas.len() > 16 || !betas.iter().all(|b| valid_beta(b)) {
        send(error_event(
            "bad_request",
            "Invalid anthropic-beta value",
            None,
            false,
        ));
        return Ok(());
    }
    let key = match app.state::<Secrets>().get(Provider::Anthropic) {
        Ok(Some(k)) => k,
        Ok(None) => {
            send(error_event(
                "key_missing",
                "Add your Anthropic key in Settings",
                None,
                false,
            ));
            return Ok(());
        }
        Err(e) => {
            send(error_event("keyring_error", e.message, None, false));
            return Ok(());
        }
    };
    let store = app.state::<ScreenshotStore>();
    let lookup = |id: &str| {
        store.get(id).map(|s| {
            (
                s.meta.mime.clone(),
                base64::engine::general_purpose::STANDARD.encode(s.data.as_slice()),
            )
        })
    };
    let body = match anthropic::prepare_body(body, &lookup) {
        Ok(b) => b,
        Err(ev) => {
            send(ev);
            return Ok(());
        }
    };

    let (gen, token) = app.state::<LlmState>().register(&request_id);

    let model = body
        .get("model")
        .and_then(Value::as_str)
        .unwrap_or("?")
        .to_string();
    tracing::info!(request_id = %request_id, %model, betas = betas.len(), "llm stream start");
    let started = Instant::now();
    let mut first_text_ms: Option<u128> = None;
    let mut outcome = String::from("?");
    let mut usage = Usage::default();
    let client = app.state::<Http>().0.clone();
    anthropic::stream_messages(
        &client,
        anthropic::API_BASE,
        &key,
        &request_id,
        &body,
        &betas,
        token.clone(),
        |ev| {
            match &ev {
                LlmEvent::Text { .. } if first_text_ms.is_none() => {
                    first_text_ms = Some(started.elapsed().as_millis())
                }
                LlmEvent::Stop {
                    stop_reason,
                    usage: u,
                } => {
                    outcome = format!("stop:{}", stop_reason.as_deref().unwrap_or("none"));
                    usage = *u;
                }
                LlmEvent::Error { code, status, .. } => {
                    outcome = format!("error:{code}:{}", status.unwrap_or(0))
                }
                _ => {}
            }
            let ok = send(ev);
            if !ok {
                outcome = "receiver_gone".into();
            }
            ok
        },
    )
    .await;

    app.state::<LlmState>().finish(&request_id, gen);
    tracing::info!(
        request_id = %request_id,
        %outcome,
        ms = started.elapsed().as_millis() as u64,
        first_text_ms = first_text_ms.map(|v| v as u64),
        input_tokens = usage.input_tokens,
        output_tokens = usage.output_tokens,
        cache_read = usage.cache_read_input_tokens,
        cache_write = usage.cache_creation_input_tokens,
        "llm stream end"
    );
    Ok(())
}

#[tauri::command]
pub async fn llm_cancel(state: tauri::State<'_, LlmState>, request_id: String) -> AppResult<()> {
    state.cancel(&request_id);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn llm_event_wire_format() {
        let v = serde_json::to_value(LlmEvent::Start {
            request_id: "r".into(),
            model: "m".into(),
        })
        .unwrap();
        assert_eq!(v, json!({"type": "start", "requestId": "r", "model": "m"}));
        assert_eq!(
            serde_json::to_value(LlmEvent::Thinking).unwrap(),
            json!({"type": "thinking"})
        );
        assert_eq!(
            serde_json::to_value(LlmEvent::Text { delta: "x".into() }).unwrap(),
            json!({"type": "text", "delta": "x"})
        );
        let v = serde_json::to_value(LlmEvent::Stop {
            stop_reason: None,
            usage: Usage {
                input_tokens: 1,
                output_tokens: 2,
                cache_read_input_tokens: 3,
                cache_creation_input_tokens: 4,
            },
        })
        .unwrap();
        assert_eq!(
            v,
            json!({"type": "stop", "stopReason": null, "usage": {"inputTokens": 1, "outputTokens": 2, "cacheReadInputTokens": 3, "cacheCreationInputTokens": 4}})
        );
        let v = serde_json::to_value(error_event("rate_limited", "slow down", Some(429), true))
            .unwrap();
        assert_eq!(
            v,
            json!({"type": "error", "code": "rate_limited", "message": "slow down", "status": 429, "retryable": true})
        );
        let v = serde_json::to_value(error_event("cancelled", "Cancelled", None, false)).unwrap();
        assert!(v.get("status").is_none());
    }

    #[test]
    fn inflight_registry_generations() {
        let s = LlmState::default();
        let (g1, t1) = s.register("a");
        let (g2, t2) = s.register("a"); // reuse cancels the first
        assert!(t1.is_cancelled());
        assert!(!t2.is_cancelled());
        s.finish("a", g1); // stale finish must not remove the newer entry
        assert!(s.cancel("a"));
        assert!(t2.is_cancelled());
        s.finish("a", g2);
        assert!(!s.cancel("a"));
    }

    #[test]
    fn id_and_beta_validation() {
        assert!(valid_request_id("card-123_abc"));
        assert!(!valid_request_id(""));
        assert!(!valid_request_id("has space"));
        assert!(valid_beta("server-side-fallback-2026-07-01"));
        assert!(!valid_beta("a,b"));
        assert!(!valid_beta("x\r\ny"));
    }
}
