//! Anthropic Messages API proxy: body preparation, SSE → `LlmEvent` mapping, error mapping,
//! and the streaming HTTP call. The key is added here and never reaches the webview.

use std::time::Duration;

use futures_util::StreamExt;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use super::sse::{SseEvent, SseParser};
use super::{LlmEvent, Usage};

pub const API_BASE: &str = "https://api.anthropic.com";
pub const ANTHROPIC_VERSION: &str = "2026-10-21";
pub const SCREENSHOT_PREFIX: &str = "@screenshot:";
/// Anthropic sends `ping` events while a response is being generated; this long without any
/// bytes means the connection is dead.
pub const STALL_TIMEOUT: Duration = Duration::from_secs(120);

pub fn error_event(
    code: &str,
    message: impl Into<String>,
    status: Option<u16>,
    retryable: bool,
) -> LlmEvent {
    LlmEvent::Error {
        code: code.to_string(),
        message: message.into(),
        status,
        retryable,
    }
}

pub fn cancelled() -> LlmEvent {
    error_event("cancelled", "Cancelled", None, false)
}

/// Map an API error `type` (from an error body or an SSE `error` event).
pub fn error_for_type(etype: &str, message: &str, status: Option<u16>) -> LlmEvent {
    let (code, retryable, default_msg) = match etype {
        "invalid_request_error" | "not_found_error" | "request_too_large" => {
            ("bad_request", false, "The request was rejected")
        }
        "authentication_error" | "permission_error" => {
            ("key_invalid", false, "The Anthropic key was rejected")
        }
        "billing_error" => (
            "key_no_credit",
            false,
            "The Anthropic account has no credit",
        ),
        "rate_limit_error" => ("rate_limited", true, "Rate limited by Anthropic"),
        "overloaded_error" => ("overloaded", true, "Claude is overloaded right now"),
        _ => ("server", true, "Anthropic server error"),
    };
    let msg = if message.trim().is_empty() || code != "bad_request" {
        default_msg.to_string()
    } else {
        message.to_string()
    };
    error_event(code, msg, status, retryable)
}

/// Map a non-200 HTTP response. 400s carry the API's own message.
pub fn error_for_status(status: u16, body: &str) -> LlmEvent {
    let parsed: Option<(String, String)> = serde_json::from_str::<Value>(body).ok().and_then(|v| {
        let e = v.get("error")?;
        Some((
            e.get("type")?.as_str()?.to_string(),
            e.get("message")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string(),
        ))
    });
    let api_msg = parsed.as_ref().map(|(_, m)| m.as_str()).unwrap_or("");
    match status {
        400 | 404 | 413 | 422 => {
            // billing problems come back as 400 invalid_request_error with a credit message
            if api_msg.to_ascii_lowercase().contains("credit balance") {
                return error_event("key_no_credit", api_msg, Some(status), false);
            }
            let msg = if api_msg.is_empty() {
                format!("The request was rejected (HTTP {status})")
            } else {
                api_msg.to_string()
            };
            error_event("bad_request", msg, Some(status), false)
        }
        401 | 403 => error_event(
            "key_invalid",
            "The Anthropic key was rejected",
            Some(status),
            false,
        ),
        402 => error_event(
            "key_no_credit",
            "The Anthropic account has no credit",
            Some(status),
            false,
        ),
        429 => error_event(
            "rate_limited",
            "Rate limited by Anthropic",
            Some(status),
            true,
        ),
        529 => error_event(
            "overloaded",
            "Claude is overloaded right now",
            Some(status),
            true,
        ),
        500..=599 => error_event("server", "Anthropic server error", Some(status), true),
        _ => match parsed {
            Some((t, m)) => error_for_type(&t, &m, Some(status)),
            None => error_event(
                "server",
                format!("Unexpected HTTP {status}"),
                Some(status),
                false,
            ),
        },
    }
}

/// Replace every `{"type":"image","source":{"data":"@screenshot:<id>"}}` with the stored
/// screenshot. Returns the number of substitutions or the first missing id.
pub fn substitute_screenshots(
    v: &mut Value,
    lookup: &dyn Fn(&str) -> Option<(String, String)>,
) -> Result<usize, String> {
    match v {
        Value::Object(map) => {
            let is_image = map.get("type").and_then(Value::as_str) == Some("image");
            if is_image {
                if let Some(Value::Object(src)) = map.get_mut("source") {
                    let id = src
                        .get("data")
                        .and_then(Value::as_str)
                        .and_then(|d| d.strip_prefix(SCREENSHOT_PREFIX))
                        .map(str::to_string);
                    if let Some(id) = id {
                        let (mime, b64) = lookup(&id).ok_or(id)?;
                        src.insert("type".into(), Value::from("base64"));
                        src.insert("media_type".into(), Value::from(mime));
                        src.insert("data".into(), Value::from(b64));
                        return Ok(1);
                    }
                }
            }
            let mut n = 0;
            for child in map.values_mut() {
                n += substitute_screenshots(child, lookup)?;
            }
            Ok(n)
        }
        Value::Array(items) => {
            let mut n = 0;
            for child in items {
                n += substitute_screenshots(child, lookup)?;
            }
            Ok(n)
        }
        _ => Ok(0),
    }
}

/// Force `stream: true` and substitute screenshots; everything else passes through unchanged.
pub fn prepare_body(
    mut body: Value,
    lookup: &dyn Fn(&str) -> Option<(String, String)>,
) -> Result<Value, LlmEvent> {
    let Some(obj) = body.as_object_mut() else {
        return Err(error_event(
            "bad_request",
            "The request body must be a JSON object",
            None,
            false,
        ));
    };
    obj.insert("stream".into(), Value::Bool(true));
    substitute_screenshots(&mut body, lookup).map_err(|id| {
        error_event(
            "screenshot_missing",
            format!("Screenshot {id} is no longer available; capture it again"),
            None,
            false,
        )
    })?;
    Ok(body)
}

/// Turns SSE events into `LlmEvent`s and tracks usage/stop reason.
#[derive(Debug)]
pub struct StreamMapper {
    request_id: String,
    usage: Usage,
    stop_reason: Option<String>,
    thinking_blocks: Vec<u64>,
    pub done: bool,
    pub model: Option<String>,
}

fn u(v: &Value, k: &str) -> Option<u64> {
    v.get(k).and_then(Value::as_u64)
}

impl StreamMapper {
    pub fn new(request_id: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            usage: Usage::default(),
            stop_reason: None,
            thinking_blocks: Vec::new(),
            done: false,
            model: None,
        }
    }

    fn merge_usage(&mut self, u_: &Value) {
        if let Some(v) = u(u_, "input_tokens") {
            self.usage.input_tokens = v;
        }
        if let Some(v) = u(u_, "output_tokens") {
            self.usage.output_tokens = v;
        }
        if let Some(v) = u(u_, "cache_read_input_tokens") {
            self.usage.cache_read_input_tokens = v;
        }
        if let Some(v) = u(u_, "cache_creation_input_tokens") {
            self.usage.cache_creation_input_tokens = v;
        }
    }

    fn thinking(&mut self, index: u64) -> Option<LlmEvent> {
        if self.thinking_blocks.contains(&index) {
            None
        } else {
            self.thinking_blocks.push(index);
            Some(LlmEvent::Thinking)
        }
    }

    pub fn on_sse(&mut self, ev: &SseEvent) -> Vec<LlmEvent> {
        if self.done || ev.event.as_deref() == Some("ping") {
            return Vec::new();
        }
        let v: Value = match serde_json::from_str(&ev.data) {
            Ok(v) => v,
            Err(e) => {
                self.done = true;
                return vec![error_event(
                    "stream_parse",
                    format!("Malformed stream event: {e}"),
                    None,
                    false,
                )];
            }
        };
        let ty = v
            .get("type")
            .and_then(Value::as_str)
            .or(ev.event.as_deref())
            .unwrap_or("");
        let index = u(&v, "index").unwrap_or(0);
        match ty {
            "message_start" => {
                let msg = v.get("message").cloned().unwrap_or(Value::Null);
                if let Some(us) = msg.get("usage") {
                    self.merge_usage(us);
                }
                let model = msg
                    .get("model")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                self.model = Some(model.clone());
                vec![LlmEvent::Start {
                    request_id: self.request_id.clone(),
                    model,
                }]
            }
            "content_block_start" => {
                let block = v.get("content_block").cloned().unwrap_or(Value::Null);
                match block.get("type").and_then(Value::as_str) {
                    Some("thinking") | Some("redacted_thinking") => {
                        self.thinking(index).into_iter().collect()
                    }
                    Some("text") => match block.get("text").and_then(Value::as_str) {
                        Some(t) if !t.is_empty() => vec![LlmEvent::Text {
                            delta: t.to_string(),
                        }],
                        _ => Vec::new(),
                    },
                    _ => Vec::new(),
                }
            }
            "content_block_delta" => {
                let d = v.get("delta").cloned().unwrap_or(Value::Null);
                match d.get("type").and_then(Value::as_str) {
                    Some("text_delta") => match d.get("text").and_then(Value::as_str) {
                        Some(t) if !t.is_empty() => vec![LlmEvent::Text {
                            delta: t.to_string(),
                        }],
                        _ => Vec::new(),
                    },
                    Some("thinking_delta") | Some("signature_delta") => {
                        self.thinking(index).into_iter().collect()
                    }
                    _ => Vec::new(),
                }
            }
            "message_delta" => {
                if let Some(r) = v
                    .get("delta")
                    .and_then(|d| d.get("stop_reason"))
                    .and_then(Value::as_str)
                {
                    self.stop_reason = Some(r.to_string());
                }
                if let Some(us) = v.get("usage") {
                    self.merge_usage(us);
                }
                Vec::new()
            }
            "message_stop" => {
                self.done = true;
                vec![LlmEvent::Stop {
                    stop_reason: self.stop_reason.clone(),
                    usage: self.usage,
                }]
            }
            "error" => {
                self.done = true;
                let e = v.get("error").cloned().unwrap_or(Value::Null);
                let et = e.get("type").and_then(Value::as_str).unwrap_or("api_error");
                let m = e.get("message").and_then(Value::as_str).unwrap_or("");
                vec![error_for_type(et, m, None)]
            }
            _ => Vec::new(), // content_block_stop, unknown future events
        }
    }

    /// The HTTP body ended. If no terminal event was seen, report a retryable failure.
    pub fn finish(&mut self) -> Option<LlmEvent> {
        if self.done {
            return None;
        }
        self.done = true;
        Some(error_event(
            "network",
            "The response ended unexpectedly",
            None,
            true,
        ))
    }
}

/// Stream one Messages API call, emitting every event (the last one is always `stop` or
/// `error`). `emit` returns false when nobody is listening any more, which aborts the call.
pub async fn stream_messages(
    client: &reqwest::Client,
    base_url: &str,
    api_key: &str,
    request_id: &str,
    body: &Value,
    betas: &[String],
    cancel: CancellationToken,
    mut emit: impl FnMut(LlmEvent) -> bool,
) {
    let payload = match serde_json::to_vec(body) {
        Ok(p) => p,
        Err(e) => {
            emit(error_event(
                "bad_request",
                format!("Couldn't encode the request: {e}"),
                None,
                false,
            ));
            return;
        }
    };
    let mut req = client
        .post(format!("{}/v1/messages", base_url.trim_end_matches('/')))
        .header("x-api-key", api_key)
        .header("anthropic-version", ANTHROPIC_VERSION)
        .header("content-type", "application/json")
        .body(payload);
    if !betas.is_empty() {
        req = req.header("anthropic-beta", betas.join(","));
    }

    let sent = tokio::select! {
        biased;
        _ = cancel.cancelled() => { emit(cancelled()); return; }
        r = req.send() => r,
    };
    let resp = match sent {
        Ok(r) => r,
        Err(e) => {
            emit(error_event("network", network_message(&e), None, true));
            return;
        }
    };
    let status = resp.status().as_u16();
    if status != 200 {
        let text = tokio::select! {
            biased;
            _ = cancel.cancelled() => { emit(cancelled()); return; }
            t = resp.text() => t.unwrap_or_default(),
        };
        emit(error_for_status(status, &text));
        return;
    }

    let mut stream = resp.bytes_stream();
    let mut parser = SseParser::new();
    let mut mapper = StreamMapper::new(request_id);
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => { emit(cancelled()); return; }
            n = tokio::time::timeout(STALL_TIMEOUT, stream.next()) => n,
        };
        let chunk = match next {
            Err(_) => {
                emit(error_event("network", "The response stalled", None, true));
                return;
            }
            Ok(None) => {
                if let Some(ev) = parser.finish() {
                    for out in mapper.on_sse(&ev) {
                        emit(out);
                    }
                }
                if let Some(e) = mapper.finish() {
                    emit(e);
                }
                return;
            }
            Ok(Some(Err(e))) => {
                emit(error_event("network", network_message(&e), None, true));
                return;
            }
            Ok(Some(Ok(b))) => b,
        };
        let events = match parser.push(&chunk) {
            Ok(evs) => evs,
            Err(e) => {
                emit(error_event("stream_parse", e, None, false));
                return;
            }
        };
        for ev in events {
            for out in mapper.on_sse(&ev) {
                if !emit(out) {
                    return; // receiver gone (webview reloaded): drop the HTTP stream
                }
            }
            if mapper.done {
                return;
            }
        }
    }
}

fn network_message(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "Timed out talking to Anthropic".into()
    } else if e.is_connect() {
        "Couldn't reach api.anthropic.com — check your connection".into()
    } else {
        "Network error talking to Anthropic".into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn run_fixture(raw: &str, chunk: usize) -> Vec<LlmEvent> {
        let mut p = SseParser::new();
        let mut m = StreamMapper::new("req-1");
        let mut out = Vec::new();
        for c in raw.as_bytes().chunks(chunk) {
            for ev in p.push(c).unwrap() {
                out.extend(m.on_sse(&ev));
            }
        }
        if let Some(ev) = p.finish() {
            out.extend(m.on_sse(&ev));
        }
        out.extend(m.finish());
        out
    }

    #[test]
    fn thinking_then_text_fixture() {
        let raw = include_str!("fixtures/thinking_text.sse");
        for chunk in [1, 7, 64, 100_000] {
            let ev = run_fixture(raw, chunk);
            assert_eq!(
                ev,
                vec![
                    LlmEvent::Start {
                        request_id: "req-1".into(),
                        model: "claude-opus-5-5".into()
                    },
                    LlmEvent::Thinking,
                    LlmEvent::Text {
                        delta: "Use a hash map".into()
                    },
                    LlmEvent::Text {
                        delta: " \u{2014} one pass, O(n).\n\n```python\nseen = {}\n```".into()
                    },
                    LlmEvent::Stop {
                        stop_reason: Some("end_turn".into()),
                        usage: Usage {
                            input_tokens: 1520,
                            output_tokens: 87,
                            cache_read_input_tokens: 1408,
                            cache_creation_input_tokens: 0
                        },
                    },
                ],
                "chunk size {chunk}"
            );
        }
    }

    #[test]
    fn overloaded_mid_stream_is_retryable_error() {
        let ev = run_fixture(include_str!("fixtures/overloaded_midstream.sse"), 13);
        assert_eq!(ev.len(), 3);
        assert_eq!(
            ev[1],
            LlmEvent::Text {
                delta: "Hel".into()
            }
        );
        assert_eq!(
            ev[2],
            error_event("overloaded", "Claude is overloaded right now", None, true)
        );
    }

    #[test]
    fn refusal_stop_reason_and_cumulative_usage() {
        let ev = run_fixture(include_str!("fixtures/refusal.sse"), 50);
        match ev.last().unwrap() {
            LlmEvent::Stop { stop_reason, usage } => {
                assert_eq!(stop_reason.as_deref(), Some("refusal"));
                assert_eq!(usage.cache_creation_input_tokens, 2048);
                assert_eq!(usage.output_tokens, 5);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn truncated_stream_reports_network_error() {
        let ev = run_fixture(include_str!("fixtures/truncated.sse"), 32);
        assert_eq!(
            ev.last().unwrap(),
            &error_event("network", "The response ended unexpectedly", None, true)
        );
    }

    #[test]
    fn malformed_json_is_stream_parse() {
        let mut m = StreamMapper::new("r");
        let out = m.on_sse(&SseEvent {
            event: Some("content_block_delta".into()),
            data: "{not json".into(),
        });
        assert!(
            matches!(&out[0], LlmEvent::Error { code, retryable: false, .. } if code == "stream_parse")
        );
        assert!(m.done);
        assert!(m.finish().is_none());
    }

    #[test]
    fn status_mapping() {
        let body400 = r#"{"type":"error","error":{"type":"invalid_request_error","message":"max_tokens: must be at least 1"}}"#;
        assert_eq!(
            error_for_status(400, body400),
            error_event(
                "bad_request",
                "max_tokens: must be at least 1",
                Some(400),
                false
            )
        );
        let credit = r#"{"type":"error","error":{"type":"invalid_request_error","message":"Your credit balance is too low to access the Anthropic API."}}"#;
        assert!(
            matches!(error_for_status(400, credit), LlmEvent::Error { ref code, .. } if code == "key_no_credit")
        );
        let auth = r#"{"type":"error","error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
        assert_eq!(
            error_for_status(401, auth),
            error_event(
                "key_invalid",
                "The Anthropic key was rejected",
                Some(401),
                false
            )
        );
        assert!(
            matches!(error_for_status(403, ""), LlmEvent::Error { ref code, retryable: false, .. } if code == "key_invalid")
        );
        assert!(
            matches!(error_for_status(429, ""), LlmEvent::Error { ref code, retryable: true, .. } if code == "rate_limited")
        );
        assert!(
            matches!(error_for_status(529, ""), LlmEvent::Error { ref code, retryable: true, .. } if code == "overloaded")
        );
        assert!(
            matches!(error_for_status(500, "<html>"), LlmEvent::Error { ref code, retryable: true, .. } if code == "server")
        );
        assert!(
            matches!(error_for_status(503, ""), LlmEvent::Error { ref code, retryable: true, .. } if code == "server")
        );
        assert!(
            matches!(error_for_status(400, "garbage"), LlmEvent::Error { ref code, ref message, .. } if code == "bad_request" && message.contains("400"))
        );
        assert!(
            matches!(error_for_status(402, ""), LlmEvent::Error { ref code, .. } if code == "key_no_credit")
        );
    }

    #[test]
    fn sse_error_types() {
        for (t, code, retry) in [
            ("overloaded_error", "overloaded", true),
            ("rate_limit_error", "rate_limited", true),
            ("api_error", "server", true),
            ("invalid_request_error", "bad_request", false),
            ("authentication_error", "key_invalid", false),
            ("billing_error", "key_no_credit", false),
            ("something_new", "server", true),
        ] {
            match error_for_type(t, "m", None) {
                LlmEvent::Error {
                    code: c, retryable, ..
                } => {
                    assert_eq!((c.as_str(), retryable), (code, retry), "{t}");
                }
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn body_prep_forces_stream_and_substitutes_screenshots() {
        let lookup =
            |id: &str| (id == "abc").then(|| ("image/png".to_string(), "QUJD".to_string()));
        let body = json!({
            "model": "claude-opus-5-5", "max_tokens": 16000, "stream": false,
            "system": [{"type": "text", "text": "sys", "cache_control": {"type": "ephemeral"}}],
            "messages": [{"role": "user", "content": [
                {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "@screenshot:abc"}},
                {"type": "text", "text": "@screenshot:abc is not an image block"},
                {"type": "tool_result", "tool_use_id": "t", "content": [
                    {"type": "image", "source": {"data": "@screenshot:abc"}}
                ]}
            ]}]
        });
        let out = prepare_body(body, &lookup).unwrap();
        assert_eq!(out["stream"], true);
        let img = &out["messages"][0]["content"][0]["source"];
        assert_eq!(
            img,
            &json!({"type": "base64", "media_type": "image/png", "data": "QUJD"})
        );
        assert_eq!(
            out["messages"][0]["content"][1]["text"],
            "@screenshot:abc is not an image block"
        );
        assert_eq!(
            out["messages"][0]["content"][2]["content"][0]["source"]["data"],
            "QUJD"
        );
        assert_eq!(out["system"][0]["cache_control"]["type"], "ephemeral");
        assert_eq!(out["max_tokens"], 16000);
    }

    #[test]
    fn body_prep_errors() {
        let none = |_: &str| None;
        let missing = json!({"messages": [{"role": "user", "content": [{"type": "image", "source": {"type": "base64", "data": "@screenshot:gone"}}]}]});
        match prepare_body(missing, &none).unwrap_err() {
            LlmEvent::Error { code, message, .. } => {
                assert_eq!(code, "screenshot_missing");
                assert!(message.contains("gone"));
            }
            other => panic!("{other:?}"),
        }
        assert!(
            matches!(prepare_body(json!([1]), &none), Err(LlmEvent::Error { ref code, .. }) if code == "bad_request")
        );
        // ordinary base64 images pass through untouched
        let plain = json!({"messages": [{"role": "user", "content": [{"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": "AAAA"}}]}]});
        let out = prepare_body(plain.clone(), &none).unwrap();
        assert_eq!(out["messages"], plain["messages"]);
    }

    /// End-to-end against a local mock server: headers, forced stream:true, SSE relay, cancel.
    #[tokio::test]
    async fn mock_server_stream_and_cancel() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (req_tx, mut req_rx) = tokio::sync::mpsc::unbounded_channel::<String>();
        tokio::spawn(async move {
            for i in 0..2 {
                let (mut sock, _) = listener.accept().await.unwrap();
                let mut buf = Vec::new();
                let mut tmp = [0u8; 4096];
                // read headers + body (content-length)
                loop {
                    let n = sock.read(&mut tmp).await.unwrap();
                    buf.extend_from_slice(&tmp[..n]);
                    let s = String::from_utf8_lossy(&buf).to_string();
                    if let Some(h) = s.find("\r\n\r\n") {
                        let len = s[..h]
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if buf.len() >= h + 4 + len {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                req_tx
                    .send(String::from_utf8_lossy(&buf).to_string())
                    .unwrap();
                let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\nconnection: close\r\n\r\n";
                sock.write_all(head.as_bytes()).await.unwrap();
                if i == 0 {
                    sock.write_all(include_str!("fixtures/thinking_text.sse").as_bytes())
                        .await
                        .unwrap();
                    sock.shutdown().await.unwrap();
                } else {
                    // second call: send the start, then hang until the client cancels
                    sock.write_all(b"event: message_start\ndata: {\"type\":\"message_start\",\"message\":{\"model\":\"m\",\"usage\":{}}}\n\n").await.unwrap();
                    tokio::time::sleep(Duration::from_secs(30)).await;
                }
            }
        });

        let client = reqwest::Client::new();
        let base = format!("http://{addr}");
        let body = json!({"model": "claude-opus-5-5", "max_tokens": 10, "messages": []});
        let prepared = prepare_body(body.clone(), &|_| None).unwrap();
        let mut got = Vec::new();
        stream_messages(
            &client,
            &base,
            "sk-test",
            "r1",
            &prepared,
            &["beta-a".into(), "beta-b".into()],
            CancellationToken::new(),
            |e| {
                got.push(e);
                true
            },
        )
        .await;
        assert!(matches!(got.last(), Some(LlmEvent::Stop { .. })), "{got:?}");
        let raw = req_rx.recv().await.unwrap();
        let lower = raw.to_ascii_lowercase();
        assert!(raw.starts_with("POST /v1/messages HTTP/1.1"));
        assert!(lower.contains("x-api-key: sk-test"));
        assert!(lower.contains("anthropic-version: 2026-10-21"));
        assert!(lower.contains("anthropic-beta: beta-a,beta-b"));
        assert!(lower.contains("content-type: application/json"));
        let json_body: Value =
            serde_json::from_str(&raw[raw.find("\r\n\r\n").unwrap() + 4..]).unwrap();
        assert_eq!(json_body["stream"], true);

        // cancel while the server hangs
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(300)).await;
            t2.cancel();
        });
        let mut got = Vec::new();
        let started = std::time::Instant::now();
        stream_messages(
            &client,
            &base,
            "k",
            "r2",
            &prepare_body(body, &|_| None).unwrap(),
            &[],
            token,
            |e| {
                got.push(e);
                true
            },
        )
        .await;
        assert!(started.elapsed() < Duration::from_secs(5));
        assert!(
            matches!(got.first(), Some(LlmEvent::Start { .. })),
            "{got:?}"
        );
        assert_eq!(got.last(), Some(&cancelled()));
    }
}
