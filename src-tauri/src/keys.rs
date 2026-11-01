//! Key commands (contract §1). No command ever returns a secret.

use std::time::{Duration, Instant};

use futures_util::{SinkExt, StreamExt};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tokio_tungstenite::tungstenite::{self, client::IntoClientRequest, http::HeaderValue, Message};

use crate::error::{AppError, AppResult};
use crate::llm::anthropic::{ANTHROPIC_VERSION, API_BASE};
use crate::llm::Http;
use crate::secrets::{key_hint, Provider, Secrets};
use crate::settings::SettingsState;

pub const EVENT_CHANGED: &str = "keys://changed";
pub const DEEPGRAM_LISTEN: &str = "wss://api.deepgram.com/v1/listen";
const TEST_TIMEOUT: Duration = Duration::from_secs(12);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyStatus {
    pub present: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeysStatus {
    pub anthropic: KeyStatus,
    pub deepgram: KeyStatus,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct KeyTestResult {
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub latency_ms: u64,
}

impl KeyTestResult {
    fn ok(latency: Duration) -> Self {
        Self {
            ok: true,
            code: None,
            message: None,
            latency_ms: latency.as_millis() as u64,
        }
    }
    fn fail(code: &str, message: impl Into<String>, latency: Duration) -> Self {
        Self {
            ok: false,
            code: Some(code.to_string()),
            message: Some(message.into()),
            latency_ms: latency.as_millis() as u64,
        }
    }
}

fn status_for(secrets: &Secrets, p: Provider) -> AppResult<KeyStatus> {
    Ok(match secrets.get(p)? {
        Some(k) if !k.is_empty() => KeyStatus {
            present: true,
            hint: Some(key_hint(&k)),
        },
        _ => KeyStatus {
            present: false,
            hint: None,
        },
    })
}

pub fn keys_status_of(secrets: &Secrets) -> AppResult<KeysStatus> {
    Ok(KeysStatus {
        anthropic: status_for(secrets, Provider::Anthropic)?,
        deepgram: status_for(secrets, Provider::Deepgram)?,
    })
}

/// Map an HTTP status from a key check (Anthropic GET or Deepgram handshake).
pub fn map_test_status(provider: Provider, status: u16) -> (&'static str, String) {
    let name = match provider {
        Provider::Anthropic => "Anthropic",
        Provider::Deepgram => "Deepgram",
    };
    match status {
        401 | 403 => ("key_invalid", format!("{name} rejected this key")),
        402 => (
            "key_no_credit",
            format!("The {name} account has no credit left"),
        ),
        400 => (
            "bad_request",
            format!("{name} rejected the test request (HTTP 400)"),
        ),
        429 => (
            "rate_limited",
            format!("{name} is rate limiting this key; try again shortly"),
        ),
        500..=599 => (
            "server",
            format!("{name} had a server error (HTTP {status})"),
        ),
        _ => (
            "server",
            format!("Unexpected response from {name} (HTTP {status})"),
        ),
    }
}

/// Basic shape check before any network call.
pub fn sanitize_key(raw: &str) -> Result<String, &'static str> {
    let k = raw.trim();
    if k.is_empty() {
        return Err("The key is empty");
    }
    if k.len() > 512 {
        return Err("The key is too long");
    }
    if !k.chars().all(|c| c.is_ascii_graphic()) {
        return Err("The key contains spaces or invalid characters");
    }
    Ok(k.to_string())
}

pub fn deepgram_test_url(language: &str) -> String {
    format!(
        "{DEEPGRAM_LISTEN}?model=nova-3&language={}&encoding=linear16&sample_rate=16000&channels=1\
&interim_results=true&smart_format=true&punctuate=true&endpointing=300&utterance_end_ms=1000&vad_events=true&tag=pluelyx-keytest",
        crate::screen::encode_query_value(language)
    )
}

async fn test_anthropic(client: &reqwest::Client, base: &str, key: &str) -> KeyTestResult {
    let t0 = Instant::now();
    let res = client
        .get(format!("{}/v1/models?limit=1", base.trim_end_matches('/')))
        .header("x-api-key", key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .timeout(TEST_TIMEOUT)
        .send()
        .await;
    match res {
        Ok(r) if r.status().is_success() => KeyTestResult::ok(t0.elapsed()),
        Ok(r) => {
            let (code, msg) = map_test_status(Provider::Anthropic, r.status().as_u16());
            KeyTestResult::fail(code, msg, t0.elapsed())
        }
        Err(e) => KeyTestResult::fail(
            "network",
            if e.is_timeout() {
                "Timed out reaching api.anthropic.com"
            } else {
                "Couldn't reach api.anthropic.com"
            },
            t0.elapsed(),
        ),
    }
}

async fn test_deepgram(url: &str, key: &str) -> KeyTestResult {
    let t0 = Instant::now();
    let mut req = match url.into_client_request() {
        Ok(r) => r,
        Err(e) => {
            return KeyTestResult::fail("network", format!("Bad Deepgram URL: {e}"), t0.elapsed())
        }
    };
    match HeaderValue::from_str(&format!("Token {key}")) {
        Ok(v) => {
            req.headers_mut().insert("Authorization", v);
        }
        Err(_) => {
            return KeyTestResult::fail(
                "key_invalid",
                "The key contains invalid characters",
                t0.elapsed(),
            )
        }
    }
    let connect = tokio_tungstenite::connect_async_tls_with_config(req, None, true, None);
    match tokio::time::timeout(TEST_TIMEOUT, connect).await {
        Err(_) => KeyTestResult::fail(
            "network",
            "Timed out reaching api.deepgram.com",
            t0.elapsed(),
        ),
        Ok(Err(tungstenite::Error::Http(resp))) => {
            let (code, msg) = map_test_status(Provider::Deepgram, resp.status().as_u16());
            KeyTestResult::fail(code, msg, t0.elapsed())
        }
        Ok(Err(e)) => {
            tracing::debug!(error = %e, "deepgram handshake failed");
            KeyTestResult::fail("network", "Couldn't reach api.deepgram.com", t0.elapsed())
        }
        Ok(Ok((mut ws, resp))) => {
            let latency = t0.elapsed();
            if let Some(id) = resp
                .headers()
                .get("dg-request-id")
                .and_then(|v| v.to_str().ok())
            {
                tracing::info!(dg_request_id = id, "deepgram key test handshake ok");
            }
            let _ = ws
                .send(Message::Text(r#"{"type":"CloseStream"}"#.into()))
                .await;
            // drain until the server closes (it sends final Metadata), max 3 s
            let _ = tokio::time::timeout(Duration::from_secs(3), async {
                while let Some(m) = ws.next().await {
                    if matches!(m, Ok(Message::Close(_)) | Err(_)) {
                        break;
                    }
                }
            })
            .await;
            let _ = ws.close(None).await;
            KeyTestResult::ok(latency)
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

fn broadcast(app: &AppHandle) {
    if let Ok(s) = keys_status_of(&app.state::<Secrets>()) {
        let _ = app.emit(EVENT_CHANGED, s);
    }
}

#[tauri::command]
pub async fn keys_status(secrets: tauri::State<'_, Secrets>) -> AppResult<KeysStatus> {
    let s = secrets.inner().clone();
    tauri::async_runtime::spawn_blocking(move || keys_status_of(&s)).await?
}

#[tauri::command]
pub async fn keys_set(app: AppHandle, provider: Provider, value: String) -> AppResult<()> {
    let key = sanitize_key(&value).map_err(|m| AppError::new("key_invalid", m))?;
    let s = app.state::<Secrets>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || s.set(provider, &key)).await??;
    tracing::info!(provider = provider.as_str(), "key saved");
    broadcast(&app);
    Ok(())
}

#[tauri::command]
pub async fn keys_delete(app: AppHandle, provider: Provider) -> AppResult<()> {
    let s = app.state::<Secrets>().inner().clone();
    tauri::async_runtime::spawn_blocking(move || s.delete(provider)).await??;
    tracing::info!(provider = provider.as_str(), "key deleted");
    broadcast(&app);
    Ok(())
}

#[tauri::command]
pub async fn keys_test(
    app: AppHandle,
    provider: Provider,
    candidate: Option<String>,
) -> AppResult<KeyTestResult> {
    let key = match candidate {
        Some(c) => match sanitize_key(&c) {
            Ok(k) => k,
            Err(m) => return Ok(KeyTestResult::fail("key_invalid", m, Duration::ZERO)),
        },
        None => {
            let s = app.state::<Secrets>().inner().clone();
            match tauri::async_runtime::spawn_blocking(move || s.get(provider)).await? {
                Ok(Some(k)) => k,
                Ok(None) => {
                    return Ok(KeyTestResult::fail(
                        "key_missing",
                        "No key saved yet",
                        Duration::ZERO,
                    ))
                }
                Err(e) => {
                    return Ok(KeyTestResult::fail(
                        "keyring_error",
                        e.message,
                        Duration::ZERO,
                    ))
                }
            }
        }
    };
    let result = match provider {
        Provider::Anthropic => test_anthropic(&app.state::<Http>().0, API_BASE, &key).await,
        Provider::Deepgram => {
            let lang = app.state::<SettingsState>().get().stt.language;
            test_deepgram(&deepgram_test_url(&lang), &key).await
        }
    };
    tracing::info!(provider = provider.as_str(), ok = result.ok, code = ?result.code, ms = result.latency_ms, "key test");
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::MemStore;
    use std::sync::Arc;

    #[test]
    fn status_never_contains_the_key() {
        let s = Secrets(Arc::new(MemStore::default()));
        assert_eq!(
            keys_status_of(&s).unwrap(),
            KeysStatus {
                anthropic: KeyStatus {
                    present: false,
                    hint: None
                },
                deepgram: KeyStatus {
                    present: false,
                    hint: None
                }
            }
        );
        s.set(Provider::Anthropic, "sk-ant-api03-secretsecret-a1b2")
            .unwrap();
        let st = keys_status_of(&s).unwrap();
        assert!(st.anthropic.present);
        assert_eq!(st.anthropic.hint.as_deref(), Some("\u{2026}a1b2"));
        let json = serde_json::to_string(&st).unwrap();
        assert!(!json.contains("secret"));
        assert_eq!(
            json,
            r#"{"anthropic":{"present":true,"hint":"…a1b2"},"deepgram":{"present":false}}"#
        );
    }

    #[test]
    fn status_mapping() {
        assert_eq!(map_test_status(Provider::Anthropic, 401).0, "key_invalid");
        assert_eq!(map_test_status(Provider::Deepgram, 403).0, "key_invalid");
        assert_eq!(map_test_status(Provider::Deepgram, 402).0, "key_no_credit");
        assert_eq!(map_test_status(Provider::Anthropic, 429).0, "rate_limited");
        assert_eq!(map_test_status(Provider::Anthropic, 503).0, "server");
        assert_eq!(map_test_status(Provider::Deepgram, 400).0, "bad_request");
    }

    #[test]
    fn key_sanitizing() {
        assert_eq!(sanitize_key("  sk-ant-x \n").unwrap(), "sk-ant-x");
        assert!(sanitize_key("   ").is_err());
        assert!(sanitize_key("two words").is_err());
        assert!(sanitize_key(&"x".repeat(600)).is_err());
    }

    #[test]
    fn deepgram_url_has_session_params() {
        let u = deepgram_test_url("en-US");
        assert!(u.starts_with("wss://api.deepgram.com/v1/listen?model=nova-3&language=en-US&"));
        for p in [
            "encoding=linear16",
            "sample_rate=16000",
            "channels=1",
            "interim_results=true",
            "utterance_end_ms=1000",
            "vad_events=true",
        ] {
            assert!(u.contains(p), "{p}");
        }
    }

    #[test]
    fn result_wire_format() {
        let v = serde_json::to_value(KeyTestResult::fail(
            "key_invalid",
            "no",
            Duration::from_millis(12),
        ))
        .unwrap();
        assert_eq!(
            v,
            serde_json::json!({"ok": false, "code": "key_invalid", "message": "no", "latencyMs": 12})
        );
        let v = serde_json::to_value(KeyTestResult::ok(Duration::from_millis(5))).unwrap();
        assert_eq!(v, serde_json::json!({"ok": true, "latencyMs": 5}));
    }

    /// Calls the real APIs with a bogus key; run manually (network).
    #[tokio::test]
    #[ignore]
    async fn bogus_keys_are_rejected_live() {
        let client = reqwest::Client::new();
        let r = test_anthropic(&client, API_BASE, "sk-ant-bogus-0000").await;
        assert_eq!(r.code.as_deref(), Some("key_invalid"), "{r:?}");
        let r = test_deepgram(&deepgram_test_url("en"), "0000bogus0000").await;
        assert_eq!(r.code.as_deref(), Some("key_invalid"), "{r:?}");
    }
}
