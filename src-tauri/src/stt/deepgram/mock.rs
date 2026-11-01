//! Test-only mock Deepgram server on 127.0.0.1 (`accept_hdr_async`).
//!
//! Each incoming connection takes the next [`Behavior`] (default: accept). Accepted sockets
//! are handed to the test as [`ServerConn`] so the test plays Deepgram by hand.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tokio_tungstenite::tungstenite::http;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::WebSocketStream;

#[derive(Debug, Clone)]
pub enum Behavior {
    Accept,
    /// Wait before answering the handshake.
    Delay(Duration),
    /// Answer the handshake with this HTTP status.
    Reject(u16),
}

#[derive(Debug, Clone)]
pub struct SeenRequest {
    /// Matches `ServerConn::index` of the same connection.
    pub index: usize,
    pub uri: String,
    pub authorization: Option<String>,
}

pub struct ServerConn {
    pub index: usize,
    pub ws: WebSocketStream<TcpStream>,
}

/// What the client sent, decoded.
#[derive(Debug, Clone, PartialEq)]
pub enum Got {
    Binary(Vec<u8>),
    Text(String),
    Close,
}

impl ServerConn {
    pub async fn next(&mut self) -> Option<Got> {
        loop {
            match self.ws.next().await? {
                Ok(Message::Binary(b)) => return Some(Got::Binary(b.to_vec())),
                Ok(Message::Text(t)) => return Some(Got::Text(t.as_str().to_string())),
                Ok(Message::Close(_)) => return Some(Got::Close),
                Ok(_) => continue,
                Err(_) => return None,
            }
        }
    }

    pub async fn next_timeout(&mut self, d: Duration) -> Option<Got> {
        tokio::time::timeout(d, self.next()).await.ok().flatten()
    }

    pub async fn send_json(&mut self, v: serde_json::Value) {
        let _ = self.ws.send(Message::text(v.to_string())).await;
    }
}

pub struct MockDeepgram {
    pub addr: SocketAddr,
    pub conns: mpsc::UnboundedReceiver<ServerConn>,
    pub requests: Arc<Mutex<Vec<SeenRequest>>>,
    pub attempts: Arc<AtomicUsize>,
}

impl MockDeepgram {
    pub async fn start(behaviors: Vec<Behavior>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let (tx, conns) = mpsc::unbounded_channel();
        let requests = Arc::new(Mutex::new(Vec::new()));
        let attempts = Arc::new(AtomicUsize::new(0));
        let (reqs, atts) = (requests.clone(), attempts.clone());
        tokio::spawn(async move {
            loop {
                let Ok((tcp, _)) = listener.accept().await else {
                    return;
                };
                let index = atts.fetch_add(1, Ordering::SeqCst);
                let behavior = behaviors.get(index).cloned().unwrap_or(Behavior::Accept);
                let (reqs, tx) = (reqs.clone(), tx.clone());
                tokio::spawn(async move {
                    if let Behavior::Delay(d) = behavior {
                        tokio::time::sleep(d).await;
                    }
                    let reject = match behavior {
                        Behavior::Reject(s) => Some(s),
                        _ => None,
                    };
                    let cb = move |req: &Request, resp: Response| -> Result<Response, ErrorResponse> {
                        reqs.lock().unwrap().push(SeenRequest {
                            index,
                            uri: req.uri().to_string(),
                            authorization: req
                                .headers()
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_string),
                        });
                        match reject {
                            Some(status) => Err(http::Response::builder()
                                .status(status)
                                .body(Some(
                                    r#"{"err_code":"TEST","err_msg":"rejected by mock"}"#.to_string(),
                                ))
                                .unwrap()),
                            None => Ok(resp),
                        }
                    };
                    if let Ok(ws) = tokio_tungstenite::accept_hdr_async(tcp, cb).await {
                        let _ = tx.send(ServerConn { index, ws });
                    }
                });
            }
        });
        Self {
            addr,
            conns,
            requests,
            attempts,
        }
    }

    pub fn base_url(&self) -> String {
        format!("ws://{}/v1/listen", self.addr)
    }

    pub async fn accept(&mut self, within: Duration) -> ServerConn {
        tokio::time::timeout(within, self.conns.recv())
            .await
            .expect("mock: no connection in time")
            .expect("mock closed")
    }

    pub fn request(&self, index: usize) -> SeenRequest {
        self.requests
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.index == index)
            .cloned()
            .expect("mock: no request with that index")
    }

    pub fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
}

/// A Deepgram `Results` message.
pub fn results(start: f64, duration: f64, text: &str, is_final: bool, speech_final: bool) -> serde_json::Value {
    serde_json::json!({
        "type": "Results",
        "channel_index": [0, 1],
        "start": start,
        "duration": duration,
        "is_final": is_final,
        "speech_final": speech_final,
        "from_finalize": false,
        "channel": { "alternatives": [ { "transcript": text, "confidence": 0.97, "words": [] } ] },
        "metadata": { "request_id": "mock" }
    })
}

pub fn utterance_end(last_word_end: f64) -> serde_json::Value {
    serde_json::json!({ "type": "UtteranceEnd", "channel": [0, 1], "last_word_end": last_word_end })
}

pub fn metadata() -> serde_json::Value {
    serde_json::json!({ "type": "Metadata", "request_id": "mock", "duration": 1.0, "channels": 1 })
}
