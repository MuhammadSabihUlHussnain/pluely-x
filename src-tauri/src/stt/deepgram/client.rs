//! One Deepgram streaming socket per source (docs/NATIVE_DESIGN.md "Deepgram client").
//!
//! `run_client` owns the socket for a source for the whole session:
//! - biased `select!` over (a) audio frames → Binary + timeline, (b) server messages,
//!   (c) a 1 s tick (KeepAlive after ≥ 4 s without audio; half-open watchdog), (d) control
//!   (Mute → Finalize; Stop → CloseStream, then read results until close or 3 s);
//! - on failure: classify fatal vs retryable, wait with full-jitter backoff while buffering
//!   up to 10 s of frames, reconnect (`conn += 1`, fresh [`Timeline`]) and replay the backlog.
//!
//! Never logs the key, headers or transcript text (transcripts only at debug level).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::{sleep_until, Instant, MissedTickBehavior};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::{HeaderValue, AUTHORIZATION};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use super::backoff::{classify_close, classify_http_status, Backoff, BackoffConfig, Failure};
use super::protocol::{self, ServerMsg};
use crate::audio::dump::WavDump;
use crate::audio::timeline::Timeline;
use crate::audio::{AudioFrame, Source, SourceStats};

type Ws = WebSocketStream<MaybeTlsStream<TcpStream>>;

/// Timing knobs. Defaults are the design values; tests shrink some of them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DgTimings {
    /// Send KeepAlive after this long without audio (Deepgram closes after ~10 s).
    pub keepalive_after: Duration,
    pub tick: Duration,
    /// Audio sent since the last server message before the socket counts as half-open.
    pub watchdog_audio: Duration,
    /// How long Stop waits for the final results after CloseStream.
    pub close_flush: Duration,
    pub connect_timeout: Duration,
    pub backoff: BackoffConfig,
    pub rate_limit_min: Duration,
    /// Frames kept while disconnected.
    pub backlog: Duration,
    /// After real speech, send Finalize once the audio has stayed below `quiet_db` this
    /// long (or frames stopped for `QUIET_GAP`). Deepgram otherwise sometimes holds the
    /// last final for ~5 s on a quiet loopback stream. `None` disables it.
    pub quiet_finalize: Option<Duration>,
    /// Frame RMS (dBFS) at or below which audio counts as quiet for `quiet_finalize`.
    pub quiet_db: f32,
}

/// Fallback for `quiet_finalize`: frames stopped arriving altogether after speech.
const QUIET_GAP: Duration = Duration::from_secs(1);

impl Default for DgTimings {
    fn default() -> Self {
        Self {
            quiet_finalize: Some(Duration::from_millis(600)),
            quiet_db: -45.0,
            keepalive_after: Duration::from_secs(4),
            tick: Duration::from_secs(1),
            watchdog_audio: Duration::from_secs(10),
            close_flush: Duration::from_secs(3),
            connect_timeout: Duration::from_secs(10),
            backoff: BackoffConfig::default(),
            rate_limit_min: super::backoff::RATE_LIMIT_MIN,
            backlog: Duration::from_secs(10),
        }
    }
}

/// Everything one source's socket needs.
pub struct DgClientConfig {
    pub source: Source,
    /// Full listen URL (see `config::build_url`).
    pub url: String,
    pub api_key: String,
    /// Connection number for the first socket (continues across client restarts).
    pub first_conn: u32,
    pub timings: DgTimings,
    pub dump: Option<WavDump>,
    pub rng_seed: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DgControl {
    /// Mute: send Finalize and drop frames until unmuted. Unmute: send frames again.
    Mute(bool),
    /// CloseStream, flush results (≤ close_flush), exit.
    Stop,
}

/// A transcript segment, already on the session clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Segment {
    pub seg_id: String,
    pub conn: u32,
    pub is_final: bool,
    pub speech_final: bool,
    pub from_finalize: bool,
    pub text: String,
    pub confidence: f64,
    pub t0_ms: i64,
    pub t1_ms: i64,
    pub speaker: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DgEvent {
    /// Opening socket `conn`; `attempt` > 0 means this is a retry.
    Connecting { conn: u32, attempt: u32 },
    Live { conn: u32 },
    /// The socket failed and will be retried after `retry_in`.
    Reconnecting {
        attempt: u32,
        retry_in: Duration,
        code: &'static str,
        message: String,
    },
    Transcript(Segment),
    SpeechStarted { t_ms: i64 },
    UtteranceEnd { last_word_end_ms: i64 },
    /// Unrecoverable (bad key, no credit, rejected request). The client has exited.
    Fatal { code: &'static str, message: String },
    /// The client task finished (after Stop, Fatal, or the frame channel closing).
    Closed,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DgOutcome {
    Stopped,
    Fatal { code: &'static str, message: String },
}

/// Events go to the supervisor tagged with the source and the client generation.
pub type DgEventTx = mpsc::UnboundedSender<(Source, u32, DgEvent)>;

struct Ctx {
    source: Source,
    gen: u32,
    events: DgEventTx,
    stats: Arc<SourceStats>,
    timings: DgTimings,
    muted: bool,
    dump: Option<WavDump>,
}

impl Ctx {
    fn emit(&self, ev: DgEvent) {
        let _ = self.events.send((self.source, self.gen, ev));
    }
}

/// Frames buffered while disconnected (oldest dropped past the cap).
struct Backlog {
    q: VecDeque<AudioFrame>,
    ms: u64,
    cap_ms: u64,
}

impl Backlog {
    fn new(cap: Duration) -> Self {
        Self {
            q: VecDeque::new(),
            ms: 0,
            cap_ms: cap.as_millis() as u64,
        }
    }

    fn push(&mut self, f: AudioFrame, stats: &SourceStats) {
        let d = f.duration_ms();
        self.ms += d;
        self.q.push_back(f);
        while self.ms > self.cap_ms {
            match self.q.pop_front() {
                Some(old) => {
                    let od = old.duration_ms();
                    self.ms -= od;
                    SourceStats::add(&stats.dropped_ms, od);
                }
                None => break,
            }
        }
    }

    fn push_front(&mut self, f: AudioFrame) {
        self.ms += f.duration_ms();
        self.q.push_front(f);
    }

    fn pop(&mut self) -> Option<AudioFrame> {
        let f = self.q.pop_front()?;
        self.ms -= f.duration_ms();
        Some(f)
    }
}

enum ConnEnd {
    Stopped,
    Failed(Failure),
}

/// Runs one source's Deepgram socket until Stop, a fatal error, or the frame channel closes.
pub async fn run_client(
    cfg: DgClientConfig,
    gen: u32,
    mut frames: mpsc::Receiver<AudioFrame>,
    mut control: mpsc::UnboundedReceiver<DgControl>,
    events: DgEventTx,
    stats: Arc<SourceStats>,
) -> DgOutcome {
    let DgClientConfig {
        source,
        url,
        api_key,
        first_conn,
        timings,
        dump,
        rng_seed,
    } = cfg;
    let mut ctx = Ctx {
        source,
        gen,
        events,
        stats,
        timings,
        muted: false,
        dump,
    };
    let mut backoff = match rng_seed {
        Some(seed) => Backoff::with_seed(timings.backoff, seed),
        None => Backoff::new(timings.backoff),
    };
    let mut backlog = Backlog::new(timings.backlog);
    let mut conn = first_conn.max(1) - 1;
    let mut attempt: u32 = 0;
    let mut ever_connected = false;

    let outcome = 'session: loop {
        conn += 1;
        ctx.emit(DgEvent::Connecting { conn, attempt });

        // Connect, buffering frames and honouring Stop meanwhile.
        let connected = {
            let fut = connect(&url, &api_key, &timings);
            tokio::pin!(fut);
            loop {
                tokio::select! {
                    biased;
                    res = &mut fut => break res,
                    f = frames.recv() => match f {
                        Some(f) => if !ctx.muted { backlog.push(f, &ctx.stats) },
                        None => break 'session DgOutcome::Stopped,
                    },
                    c = control.recv() => match c {
                        Some(DgControl::Mute(m)) => ctx.muted = m,
                        Some(DgControl::Stop) | None => break 'session DgOutcome::Stopped,
                    },
                }
            }
        };

        let failure = match connected {
            Ok(ws) => {
                if ever_connected {
                    SourceStats::add(&ctx.stats.reconnects, 1);
                }
                ever_connected = true;
                attempt = 0;
                ctx.emit(DgEvent::Live { conn });
                let started = Instant::now();
                match run_connection(ws, conn, &mut ctx, &mut frames, &mut control, &mut backlog).await {
                    ConnEnd::Stopped => break 'session DgOutcome::Stopped,
                    ConnEnd::Failed(f) => {
                        backoff.note_connection_lasted(started.elapsed());
                        f
                    }
                }
            }
            Err(f) => f,
        };

        let (code, message, min_delay) = match failure {
            Failure::Fatal { code, message } => {
                tracing::warn!(source = %source, code, "deepgram fatal: {message}");
                ctx.emit(DgEvent::Fatal {
                    code,
                    message: message.clone(),
                });
                break 'session DgOutcome::Fatal { code, message };
            }
            Failure::Retry {
                code,
                message,
                min_delay,
            } => (code, message, min_delay),
        };
        attempt += 1;
        let delay = backoff.next_delay().max(min_delay.unwrap_or_default());
        tracing::info!(source = %source, code, attempt, delay_ms = delay.as_millis() as u64, "deepgram reconnecting: {message}");
        ctx.emit(DgEvent::Reconnecting {
            attempt,
            retry_in: delay,
            code,
            message,
        });

        // Wait out the backoff, still buffering frames.
        let wake = Instant::now() + delay;
        loop {
            tokio::select! {
                biased;
                _ = sleep_until(wake) => break,
                f = frames.recv() => match f {
                    Some(f) => if !ctx.muted { backlog.push(f, &ctx.stats) },
                    None => break 'session DgOutcome::Stopped,
                },
                c = control.recv() => match c {
                    Some(DgControl::Mute(m)) => ctx.muted = m,
                    Some(DgControl::Stop) | None => break 'session DgOutcome::Stopped,
                },
            }
        }
    };

    if let Some(d) = ctx.dump.take() {
        let _ = d.finish();
    }
    ctx.emit(DgEvent::Closed);
    outcome
}

async fn connect(url: &str, key: &str, t: &DgTimings) -> Result<Ws, Failure> {
    let mut req = url
        .into_client_request()
        .map_err(|e| Failure::fatal("dg_bad_request", format!("Invalid Deepgram URL: {e}")))?;
    let auth = HeaderValue::from_str(&format!("Token {}", key.trim()))
        .map_err(|_| Failure::fatal("dg_auth", "The Deepgram key contains invalid characters."))?;
    req.headers_mut().insert(AUTHORIZATION, auth);
    let fut = tokio_tungstenite::connect_async_tls_with_config(req, None, true, None);
    match tokio::time::timeout(t.connect_timeout, fut).await {
        Err(_) => Err(Failure::retry("dg_timeout", "Connecting to Deepgram timed out.")),
        Ok(Err(e)) => Err(classify_ws_error(e, t)),
        Ok(Ok((ws, resp))) => {
            if let Some(id) = resp.headers().get("dg-request-id").and_then(|v| v.to_str().ok()) {
                tracing::info!(request_id = id, "deepgram connected");
            }
            Ok(ws)
        }
    }
}

fn classify_ws_error(e: WsError, t: &DgTimings) -> Failure {
    match e {
        WsError::Http(resp) => {
            let status = resp.status().as_u16();
            let mut f = classify_http_status(status, t.rate_limit_min);
            // Deepgram explains rejections in a JSON body (err_msg); never contains secrets.
            if let Some(body) = resp.body().as_ref() {
                if let Some(msg) = serde_json::from_slice::<serde_json::Value>(body)
                    .ok()
                    .and_then(|v| v.get("err_msg").and_then(|m| m.as_str()).map(str::to_string))
                {
                    tracing::warn!(status, "deepgram rejected the handshake: {msg}");
                    if let Failure::Fatal { code: "dg_bad_request", message } = &mut f {
                        *message = format!("{message} {msg}");
                    }
                }
            }
            f
        }
        WsError::Url(u) => Failure::fatal("dg_bad_request", format!("Invalid Deepgram URL: {u}")),
        WsError::HttpFormat(e) => Failure::fatal("dg_bad_request", format!("Bad request: {e}")),
        other => Failure::retry("dg_network", format!("Couldn't reach Deepgram: {other}")),
    }
}

async fn run_connection(
    ws: Ws,
    conn: u32,
    ctx: &mut Ctx,
    frames: &mut mpsc::Receiver<AudioFrame>,
    control: &mut mpsc::UnboundedReceiver<DgControl>,
    backlog: &mut Backlog,
) -> ConnEnd {
    let (mut sink, mut stream) = ws.split();
    let mut timeline = Timeline::new();
    let t = ctx.timings;

    // Replay what was buffered while disconnected.
    while let Some(f) = backlog.pop() {
        if let Err(e) = send_frame(&mut sink, &f, &mut timeline, ctx).await {
            backlog.push_front(f);
            return ConnEnd::Failed(Failure::retry("dg_network", format!("Send failed: {e}")));
        }
    }

    let mut last_audio = Instant::now();
    let mut last_keepalive = Instant::now();
    let mut audio_since_msg_ms: u64 = 0;
    let mut tick = tokio::time::interval_at(Instant::now() + t.tick, t.tick);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    // Quiet finalize: after speech, flush Deepgram once the audio goes quiet.
    let mut finalize_at: Option<Instant> = None;
    let mut voiced = false;
    let mut quiet_since: Option<Instant> = None;

    loop {
        tokio::select! {
            biased;
            f = frames.recv() => match f {
                Some(f) => {
                    if ctx.muted {
                        continue;
                    }
                    let d = f.duration_ms();
                    if let Err(e) = send_frame(&mut sink, &f, &mut timeline, ctx).await {
                        backlog.push(f, &ctx.stats);
                        return ConnEnd::Failed(Failure::retry("dg_network", format!("Send failed: {e}")));
                    }
                    last_audio = Instant::now();
                    audio_since_msg_ms += d;
                    if let Some(after) = t.quiet_finalize {
                        let (rms, _) = crate::audio::level::rms_peak_pcm(&f.pcm);
                        if crate::audio::level::to_db(rms) > t.quiet_db {
                            voiced = true;
                            quiet_since = None;
                            finalize_at = Some(last_audio + QUIET_GAP);
                        } else if voiced {
                            let since = *quiet_since.get_or_insert(last_audio);
                            finalize_at = Some(since + after);
                        }
                    }
                }
                None => {
                    // Capture side is gone for good: flush like a Stop.
                    close_and_flush(&mut sink, &mut stream, conn, &timeline, ctx).await;
                    return ConnEnd::Stopped;
                }
            },
            m = stream.next() => match m {
                Some(Ok(Message::Text(text))) => {
                    audio_since_msg_ms = 0;
                    handle_text(text.as_str(), conn, &timeline, ctx);
                }
                Some(Ok(Message::Close(frame))) => {
                    let (code, reason) = frame
                        .map(|f| (u16::from(f.code), f.reason.as_str().to_string()))
                        .unwrap_or((u16::from(CloseCode::Status), String::new()));
                    return ConnEnd::Failed(classify_close(code, &reason));
                }
                Some(Ok(_)) => {} // ping/pong/binary: tungstenite answers pings itself
                Some(Err(e)) => {
                    return ConnEnd::Failed(Failure::retry("dg_network", format!("Connection lost: {e}")));
                }
                None => {
                    return ConnEnd::Failed(Failure::retry("dg_network", "Connection closed."));
                }
            },
            _ = sleep_until(finalize_at.unwrap_or_else(Instant::now)), if finalize_at.is_some() && !ctx.muted => {
                finalize_at = None;
                voiced = false;
                quiet_since = None;
                if let Err(e) = sink.send(Message::text(protocol::FINALIZE)).await {
                    return ConnEnd::Failed(Failure::retry("dg_network", format!("Finalize failed: {e}")));
                }
            },
            _ = tick.tick() => {
                if audio_since_msg_ms >= t.watchdog_audio.as_millis() as u64 {
                    let _ = tokio::time::timeout(Duration::from_millis(500), sink.close()).await;
                    return ConnEnd::Failed(Failure::retry(
                        "dg_timeout",
                        "Deepgram stopped responding; reconnecting.",
                    ));
                }
                let quiet_since = last_audio.max(last_keepalive);
                if quiet_since.elapsed() >= t.keepalive_after {
                    if let Err(e) = sink.send(Message::text(protocol::KEEP_ALIVE)).await {
                        return ConnEnd::Failed(Failure::retry("dg_network", format!("KeepAlive failed: {e}")));
                    }
                    last_keepalive = Instant::now();
                }
            },
            c = control.recv() => match c {
                Some(DgControl::Mute(m)) => {
                    let was = ctx.muted;
                    ctx.muted = m;
                    if m {
                        // Mute sends its own Finalize; don't fire a stale one on unmute.
                        finalize_at = None;
                        voiced = false;
                        quiet_since = None;
                    }
                    if m && !was {
                        if let Err(e) = sink.send(Message::text(protocol::FINALIZE)).await {
                            return ConnEnd::Failed(Failure::retry("dg_network", format!("Finalize failed: {e}")));
                        }
                    }
                }
                Some(DgControl::Stop) | None => {
                    close_and_flush(&mut sink, &mut stream, conn, &timeline, ctx).await;
                    return ConnEnd::Stopped;
                }
            },
        }
    }
}

async fn send_frame<S>(
    sink: &mut S,
    f: &AudioFrame,
    timeline: &mut Timeline,
    ctx: &mut Ctx,
) -> Result<(), WsError>
where
    S: futures_util::Sink<Message, Error = WsError> + Unpin,
{
    sink.send(Message::binary(f.pcm.clone())).await?;
    timeline.record(f.t_ms, f.samples);
    SourceStats::add(&ctx.stats.sent_ms, f.duration_ms());
    if let Some(d) = ctx.dump.as_mut() {
        if let Err(e) = d.write(&f.pcm) {
            tracing::warn!("debug dump write failed, disabling: {e}");
            ctx.dump = None;
        }
    }
    Ok(())
}

async fn close_and_flush<S, R>(sink: &mut S, stream: &mut R, conn: u32, timeline: &Timeline, ctx: &mut Ctx)
where
    S: futures_util::Sink<Message, Error = WsError> + Unpin,
    R: futures_util::Stream<Item = Result<Message, WsError>> + Unpin,
{
    if sink.send(Message::text(protocol::CLOSE_STREAM)).await.is_err() {
        return;
    }
    let deadline = Instant::now() + ctx.timings.close_flush;
    loop {
        tokio::select! {
            _ = sleep_until(deadline) => {
                tracing::info!(source = %ctx.source, "deepgram flush timed out");
                break;
            }
            m = stream.next() => match m {
                Some(Ok(Message::Text(text))) => handle_text(text.as_str(), conn, timeline, ctx),
                Some(Ok(Message::Close(_))) | Some(Err(_)) | None => break,
                Some(Ok(_)) => {}
            },
        }
    }
    let _ = tokio::time::timeout(Duration::from_millis(500), sink.close()).await;
}

fn handle_text(text: &str, conn: u32, timeline: &Timeline, ctx: &Ctx) {
    match protocol::parse(text) {
        Ok(ServerMsg::Results(r)) => {
            let seg = Segment {
                seg_id: r.seg_id(ctx.source.as_str(), conn),
                conn,
                is_final: r.is_final,
                speech_final: r.speech_final,
                from_finalize: r.from_finalize,
                text: r.transcript().to_string(),
                confidence: r.confidence(),
                t0_ms: timeline.to_session_ms(r.start),
                t1_ms: timeline.to_session_ms(r.start + r.duration),
                speaker: r.speaker(),
            };
            tracing::debug!(source = %ctx.source, seg = %seg.seg_id, is_final = seg.is_final, text = %seg.text, "transcript");
            ctx.emit(DgEvent::Transcript(seg));
        }
        Ok(ServerMsg::UtteranceEnd(u)) => ctx.emit(DgEvent::UtteranceEnd {
            last_word_end_ms: timeline.to_session_ms(u.last_word_end),
        }),
        Ok(ServerMsg::SpeechStarted(s)) => ctx.emit(DgEvent::SpeechStarted {
            t_ms: timeline.to_session_ms(s.timestamp),
        }),
        Ok(ServerMsg::Metadata(m)) => {
            tracing::info!(source = %ctx.source, request_id = ?m.request_id, duration = ?m.duration, "deepgram metadata");
        }
        Ok(ServerMsg::Unknown) => {
            tracing::debug!(source = %ctx.source, "unknown deepgram message type");
        }
        Err(e) => {
            tracing::warn!(source = %ctx.source, "unparseable deepgram message: {e}");
        }
    }
}

#[cfg(test)]
#[path = "client_tests.rs"]
mod tests;
