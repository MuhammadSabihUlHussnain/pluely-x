//! The session supervisor: one tokio task per live session that owns both sources.
//!
//! Per source it runs a capture (via [`CaptureBackend`], on its own thread) feeding a
//! `mpsc::channel(64)` that a Deepgram task ([`run_client`]) drains. The supervisor:
//! - restarts lost captures with the 100 ms → 5 s ladder; the Deepgram socket stays up;
//! - follows default-device changes (400 ms debounce) for sources without a chosen device,
//!   and switches back to a chosen device that reappears after a fallback;
//! - turns Deepgram output into `SessionEvent`s: transcripts (echo-flagged), turns,
//!   speechStarted/utteranceEnd, states and errors;
//! - emits `levels` at 10 Hz and `stats` every 5 s;
//! - on stop: stops captures, sends CloseStream, waits ≤ 3 s for the flush, flushes turns.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::{AbortHandle, JoinHandle};
use tokio::time::{interval, interval_at, sleep_until, Instant, MissedTickBehavior};

use super::hub::{EventHub, EventSink};
use super::types::*;
use crate::audio::dump::{dump_path, WavDump};
use crate::audio::level::LevelMeter;
use crate::audio::timeline::SessionClock;
use crate::audio::{
    default_role_for, AecState, AudioFrame, CaptureBackend, CaptureError, CaptureExit,
    CaptureRequest, DeviceEvent, DeviceSelector, OpenedDevice, RunningCapture, Source, SourceStats,
};
use crate::error::{AppError, AppResult};
use crate::stt::deepgram::backoff::Ladder;
use crate::stt::deepgram::client::{
    run_client, DgClientConfig, DgControl, DgEvent, DgEventTx, DgOutcome, DgTimings, Segment,
};
use crate::stt::deepgram::config::{build_url, ListenParams};
use crate::stt::echo::EchoDetector;
use crate::stt::turns::{Turn, TurnTracker};

pub const FRAME_CHANNEL: usize = 64;
pub const LEVELS_EVERY: Duration = Duration::from_millis(100);
pub const STATS_EVERY: Duration = Duration::from_secs(5);
pub const DEVICE_DEBOUNCE: Duration = Duration::from_millis(400);
/// A capture that ran this long before failing resets its restart ladder.
const HEALTHY_CAPTURE: Duration = Duration::from_secs(10);
/// How long the initial device opens may take.
const INITIAL_OPEN_TIMEOUT: Duration = Duration::from_secs(7);

/// What a session needs from the outside world (injectable for tests).
pub struct SupervisorDeps {
    pub backend: Arc<dyn CaptureBackend>,
    pub device_events: Option<mpsc::UnboundedReceiver<DeviceEvent>>,
    pub dg_base_url: String,
    pub api_key: String,
    pub timings: DgTimings,
    pub dump_dir: Option<PathBuf>,
}

pub enum SupCmd {
    SetSource(SetSourceArgs, oneshot::Sender<AppResult<()>>),
    Stop(oneshot::Sender<SessionSummary>),
}

/// The running session, held in the manager's single slot.
pub struct SessionHandle {
    pub session_id: String,
    pub started_at_ms: i64,
    pub hub: Arc<EventHub>,
    cmd: mpsc::UnboundedSender<SupCmd>,
    task: JoinHandle<()>,
}

impl SessionHandle {
    pub fn status(&self) -> SessionStatus {
        SessionStatus {
            active: true,
            session_id: Some(self.session_id.clone()),
            started_at_ms: Some(self.started_at_ms),
        }
    }

    pub async fn set_source(&self, args: SetSourceArgs) -> AppResult<()> {
        let (tx, rx) = oneshot::channel();
        self.cmd
            .send(SupCmd::SetSource(args, tx))
            .map_err(|_| AppError::new("session_ended", "The session has ended."))?;
        rx.await
            .map_err(|_| AppError::new("session_ended", "The session has ended."))?
    }

    /// Stop and wait for the flush (bounded by the Deepgram close window + capture stop).
    pub async fn stop(self) -> SessionSummary {
        let (tx, rx) = oneshot::channel();
        let fallback = SessionSummary {
            session_id: self.session_id.clone(),
            duration_ms: unix_ms() - self.started_at_ms,
            reconnects: 0,
            dropped_ms: 0,
        };
        if self.cmd.send(SupCmd::Stop(tx)).is_err() {
            return fallback;
        }
        let summary = rx.await.unwrap_or(fallback);
        let _ = self.task.await;
        summary
    }
}

pub fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

enum CapMsg {
    Opened {
        source: Source,
        gen: u64,
        result: Result<RunningCapture, CaptureError>,
    },
    Exited {
        source: Source,
        gen: u64,
        exit: CaptureExit,
    },
}

struct DgHandle {
    gen: u32,
    ctl: mpsc::UnboundedSender<DgControl>,
    task: JoinHandle<DgOutcome>,
}

struct SourceRt {
    source: Source,
    enabled: bool,
    muted: bool,
    /// The chosen endpoint id (None = follow the default).
    device_pref: Option<String>,
    /// Bumped whenever a capture is (re)started or stopped, so stale messages are ignored.
    cap_gen: u64,
    capture: Option<RunningCapture>,
    opening: bool,
    opened_at: Option<Instant>,
    restart_at: Option<Instant>,
    /// The `device.reason` for the next successful open.
    pending_reason: Option<DeviceReason>,
    ladder: Ladder,
    last_cap_error: Option<&'static str>,
    last_device: Option<OpenedDevice>,
    /// Frame channel: the capture's sender and, until the Deepgram task starts, its receiver.
    frames_tx: Option<mpsc::Sender<AudioFrame>>,
    frames_rx: Option<mpsc::Receiver<AudioFrame>>,
    dg: Option<DgHandle>,
    dg_gen: u32,
    next_conn: u32,
    errored: bool,
    level: Arc<LevelMeter>,
    stats: Arc<SourceStats>,
    turns: TurnTracker,
    shown_interims: HashSet<String>,
}

impl SourceRt {
    fn new(source: Source, opts: &SourceOptions) -> Self {
        Self {
            source,
            enabled: opts.enabled,
            muted: false,
            device_pref: opts
                .device_id
                .clone()
                .filter(|d| !d.trim().is_empty() && d != "default"),
            cap_gen: 0,
            capture: None,
            opening: false,
            opened_at: None,
            restart_at: None,
            pending_reason: None,
            ladder: Ladder::default(),
            last_cap_error: None,
            last_device: None,
            frames_tx: None,
            frames_rx: None,
            dg: None,
            dg_gen: 0,
            next_conn: 1,
            errored: false,
            level: Arc::new(LevelMeter::new()),
            stats: Arc::new(SourceStats::default()),
            turns: TurnTracker::new(source),
            shown_interims: HashSet::new(),
        }
    }

    fn ensure_channel(&mut self) {
        if self.frames_tx.is_none() {
            let (tx, rx) = mpsc::channel(FRAME_CHANNEL);
            self.frames_tx = Some(tx);
            self.frames_rx = Some(rx);
        }
    }

    fn noun(&self) -> &'static str {
        match self.source {
            Source::Me => "microphone",
            Source::Them => "audio output",
        }
    }
}

struct Supervisor {
    session_id: String,
    opts: SessionOptions,
    clock: SessionClock,
    hub: Arc<EventHub>,
    backend: Arc<dyn CaptureBackend>,
    dg_base_url: String,
    api_key: String,
    timings: DgTimings,
    dump_dir: Option<PathBuf>,
    src: [SourceRt; 2],
    echo: EchoDetector,
    dg_tx: DgEventTx,
    dg_rx: mpsc::UnboundedReceiver<(Source, u32, DgEvent)>,
    cap_tx: mpsc::UnboundedSender<CapMsg>,
    cap_rx: mpsc::UnboundedReceiver<CapMsg>,
    dev_rx: Option<mpsc::UnboundedReceiver<DeviceEvent>>,
    /// Latest default endpoint per source role (from notifications).
    defaults: [Option<Option<String>>; 2],
    available: HashSet<String>,
    dev_debounce_at: Option<Instant>,
    session_live: bool,
    retiring: Vec<JoinHandle<DgOutcome>>,
}

/// Start a session: open the enabled sources, start their Deepgram sockets, and spawn the
/// supervisor loop. Fails only if every enabled source failed to open.
pub async fn start_session(
    opts: SessionOptions,
    deps: SupervisorDeps,
    sink: Box<dyn EventSink>,
) -> AppResult<(SessionInfo, SessionHandle)> {
    let session_id = uuid::Uuid::new_v4().to_string();
    let started_at_ms = unix_ms();
    let clock = SessionClock::start();
    let status = SessionStatus {
        active: true,
        session_id: Some(session_id.clone()),
        started_at_ms: Some(started_at_ms),
    };
    let hub = Arc::new(EventHub::new(sink, status));
    let (dg_tx, dg_rx) = mpsc::unbounded_channel();
    let (cap_tx, cap_rx) = mpsc::unbounded_channel();
    let mut sup = Supervisor {
        session_id: session_id.clone(),
        src: [
            SourceRt::new(Source::Me, &opts.me),
            SourceRt::new(Source::Them, &opts.them),
        ],
        opts,
        clock,
        hub: hub.clone(),
        backend: deps.backend,
        dg_base_url: deps.dg_base_url,
        api_key: deps.api_key,
        timings: deps.timings,
        dump_dir: deps.dump_dir,
        echo: EchoDetector::new(),
        dg_tx,
        dg_rx,
        cap_tx,
        cap_rx,
        dev_rx: deps.device_events,
        defaults: [None, None],
        available: HashSet::new(),
        dev_debounce_at: None,
        session_live: false,
        retiring: Vec::new(),
    };

    tracing::info!(session = %session_id, me = sup.src[0].enabled, them = sup.src[1].enabled, "session starting");
    sup.hub.emit(SessionEvent::state(None, StreamState::Connecting));

    // Open the enabled sources in parallel and wait for both results.
    let mut pending = 0;
    for s in Source::ALL {
        if sup.src[s.index()].enabled {
            sup.src[s.index()].ensure_channel();
            sup.spawn_open(s);
            pending += 1;
        }
    }
    let deadline = Instant::now() + INITIAL_OPEN_TIMEOUT;
    let mut first_error: Option<AppError> = None;
    while pending > 0 {
        let msg = tokio::select! {
            m = sup.cap_rx.recv() => m,
            _ = sleep_until(deadline) => None,
        };
        let Some(msg) = msg else {
            break;
        };
        if let CapMsg::Opened { source, gen, result } = msg {
            if gen == sup.src[source.index()].cap_gen {
                pending -= 1;
                if let Err(e) = &result {
                    first_error.get_or_insert_with(|| e.to_app_error(source));
                }
                sup.on_opened(source, gen, result, true);
            }
        } else {
            sup.on_cap(msg);
        }
    }

    let enabled: Vec<Source> = Source::ALL
        .into_iter()
        .filter(|s| sup.src[s.index()].enabled)
        .collect();
    let any_ok = enabled.iter().any(|s| sup.src[s.index()].capture.is_some());
    if !enabled.is_empty() && !any_ok {
        let err = first_error.unwrap_or_else(|| {
            AppError::new("audio_init_failed", "Couldn't start audio capture.")
        });
        tracing::warn!(code = %err.code, "session start failed");
        sup.abort_all();
        return Err(err);
    }

    let device_info = |s: Source| {
        let rt = &sup.src[s.index()];
        rt.capture.as_ref().map(|c| DeviceInfo {
            device_id: c.device.device_id.clone(),
            name: c.device.name.clone(),
            format: c.device.format.clone(),
        })
    };
    let aec = sup.src[0]
        .capture
        .as_ref()
        .and_then(|c| c.device.aec)
        .unwrap_or(AecState::Off);
    let info = SessionInfo {
        session_id: session_id.clone(),
        started_at_ms,
        me: device_info(Source::Me),
        them: device_info(Source::Them),
        aec,
    };

    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let task = tokio::spawn(sup.run(cmd_rx));
    Ok((
        info,
        SessionHandle {
            session_id,
            started_at_ms,
            hub,
            cmd: cmd_tx,
            task,
        },
    ))
}

async fn recv_opt<T>(rx: &mut Option<mpsc::UnboundedReceiver<T>>) -> Option<T> {
    match rx {
        Some(r) => r.recv().await,
        None => std::future::pending().await,
    }
}

async fn sleep_until_opt(at: Option<Instant>) {
    match at {
        Some(at) => sleep_until(at).await,
        None => std::future::pending().await,
    }
}

impl Supervisor {
    fn emit(&self, ev: SessionEvent) {
        self.hub.emit(ev);
    }

    async fn run(mut self, mut cmd_rx: mpsc::UnboundedReceiver<SupCmd>) {
        let mut levels = interval(LEVELS_EVERY);
        levels.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut stats = interval_at(Instant::now() + STATS_EVERY, STATS_EVERY);
        stats.set_missed_tick_behavior(MissedTickBehavior::Skip);

        let reply = loop {
            let deadline = self.next_deadline();
            tokio::select! {
                cmd = cmd_rx.recv() => match cmd {
                    Some(SupCmd::SetSource(args, reply)) => {
                        let r = self.set_source(args);
                        let _ = reply.send(r);
                    }
                    Some(SupCmd::Stop(reply)) => break Some(reply),
                    None => break None,
                },
                Some((s, gen, ev)) = self.dg_rx.recv() => self.handle_dg(s, gen, ev),
                Some(msg) = self.cap_rx.recv() => self.on_cap(msg),
                Some(ev) = recv_opt(&mut self.dev_rx) => self.on_device_event(ev),
                _ = levels.tick() => self.emit_levels(),
                _ = stats.tick() => self.emit_stats(),
                _ = sleep_until_opt(deadline) => self.on_timer(),
            }
        };
        let summary = self.shutdown().await;
        if let Some(r) = reply {
            let _ = r.send(summary);
        }
    }

    // ---- capture lifecycle -------------------------------------------------------------

    fn spawn_open(&mut self, s: Source) {
        let aec = self.opts.aec;
        let clock = self.clock;
        let rt = &mut self.src[s.index()];
        rt.cap_gen += 1;
        let gen = rt.cap_gen;
        rt.opening = true;
        rt.restart_at = None;
        if let Some(old) = rt.capture.take() {
            old.stop();
        }
        rt.ensure_channel();
        let Some(frames) = rt.frames_tx.clone() else {
            return;
        };
        let exit_tx = self.cap_tx.clone();
        let req = CaptureRequest {
            source: s,
            device: DeviceSelector::from_option(rt.device_pref.as_deref()),
            aec,
            clock,
            frames,
            level: rt.level.clone(),
            stats: rt.stats.clone(),
            on_exit: Box::new(move |exit| {
                let _ = exit_tx.send(CapMsg::Exited {
                    source: s,
                    gen,
                    exit,
                });
            }),
        };
        let backend = self.backend.clone();
        let cap_tx = self.cap_tx.clone();
        tokio::spawn(async move {
            let result = match tokio::task::spawn_blocking(move || backend.start(req)).await {
                Ok(r) => r,
                Err(e) => Err(CaptureError::InitFailed(format!("capture start panicked: {e}"))),
            };
            // If the session is gone, dropping the RunningCapture stops it.
            let _ = cap_tx.send(CapMsg::Opened {
                source: s,
                gen,
                result,
            });
        });
    }

    fn on_cap(&mut self, msg: CapMsg) {
        match msg {
            CapMsg::Opened {
                source,
                gen,
                result,
            } => self.on_opened(source, gen, result, false),
            CapMsg::Exited { source, gen, exit } => self.on_exited(source, gen, exit),
        }
    }

    fn on_opened(
        &mut self,
        s: Source,
        gen: u64,
        result: Result<RunningCapture, CaptureError>,
        initial: bool,
    ) {
        let i = s.index();
        if gen != self.src[i].cap_gen || !self.src[i].enabled || self.src[i].errored {
            return; // stale: dropping `result` stops a capture that opened too late
        }
        self.src[i].opening = false;
        match result {
            Ok(cap) => {
                let dev = cap.device.clone();
                let rt = &mut self.src[i];
                let reason = if dev.fallback {
                    DeviceReason::Fallback
                } else {
                    rt.pending_reason.take().unwrap_or(DeviceReason::Opened)
                };
                rt.pending_reason = None;
                rt.capture = Some(cap);
                rt.opened_at = Some(Instant::now());
                rt.last_cap_error = None;
                rt.last_device = Some(dev.clone());
                tracing::info!(source = %s, ?reason, device = %dev.name, "capture opened");
                self.emit(SessionEvent::Device {
                    source: s,
                    reason,
                    device_id: dev.device_id,
                    name: dev.name,
                    format: dev.format,
                    aec: dev.aec,
                });
                if self.src[i].dg.is_none() {
                    self.start_dg(s);
                }
            }
            Err(e) if initial || e == CaptureError::PermissionDenied => {
                let app = e.to_app_error(s);
                self.source_failed(s, &app.code, &app.message);
            }
            Err(e) => {
                let rt = &mut self.src[i];
                let delay = rt.ladder.next_delay();
                rt.restart_at = Some(Instant::now() + delay);
                let first_of_streak = rt.last_cap_error != Some(e.code());
                rt.last_cap_error = Some(e.code());
                tracing::warn!(source = %s, code = e.code(), "capture open failed, retrying: {e}");
                if first_of_streak {
                    self.emit(SessionEvent::error(
                        Some(s),
                        e.code(),
                        e.user_message(s),
                        false,
                        Some(delay.as_millis() as u64),
                    ));
                }
            }
        }
    }

    fn on_exited(&mut self, s: Source, gen: u64, exit: CaptureExit) {
        let i = s.index();
        if gen != self.src[i].cap_gen {
            return;
        }
        self.src[i].capture = None;
        match exit {
            CaptureExit::Stopped => {}
            CaptureExit::PermissionDenied => {
                let e = CaptureError::PermissionDenied.to_app_error(s);
                self.source_failed(s, &e.code, &e.message);
            }
            CaptureExit::DeviceLost(detail) => {
                tracing::warn!(source = %s, "capture lost: {detail}");
                let rt = &mut self.src[i];
                if rt.opened_at.is_some_and(|t| t.elapsed() >= HEALTHY_CAPTURE) {
                    rt.ladder.reset();
                }
                let delay = rt.ladder.next_delay();
                rt.restart_at = Some(Instant::now() + delay);
                rt.pending_reason = None;
                let noun = rt.noun();
                if let Some(dev) = rt.last_device.clone() {
                    self.emit(SessionEvent::Device {
                        source: s,
                        reason: DeviceReason::Lost,
                        device_id: dev.device_id,
                        name: dev.name,
                        format: dev.format,
                        aec: dev.aec,
                    });
                }
                self.emit(SessionEvent::error(
                    Some(s),
                    "device_lost",
                    format!("The {noun} was disconnected; reconnecting."),
                    false,
                    Some(delay.as_millis() as u64),
                ));
            }
        }
    }

    /// Stop a source for good (until re-enabled): fatal error event + state error.
    fn source_failed(&mut self, s: Source, code: &str, message: &str) {
        let i = s.index();
        {
            let rt = &mut self.src[i];
            rt.errored = true;
            rt.opening = false;
            rt.restart_at = None;
            rt.cap_gen += 1;
            if let Some(c) = rt.capture.take() {
                c.stop();
            }
        }
        self.stop_dg(s);
        self.emit(SessionEvent::error(Some(s), code, message, true, None));
        self.emit(SessionEvent::state(Some(s), StreamState::Error));
        let all_failed = Source::ALL
            .iter()
            .filter(|x| self.src[x.index()].enabled)
            .all(|x| self.src[x.index()].errored);
        if all_failed {
            self.emit(SessionEvent::state(None, StreamState::Error));
        }
    }

    // ---- deepgram lifecycle ------------------------------------------------------------

    fn start_dg(&mut self, s: Source) {
        let i = s.index();
        let stt = self.opts.stt.clone();
        let diarize = s == Source::Them && stt.diarize_them;
        let url = build_url(
            &self.dg_base_url,
            &ListenParams {
                language: stt.language,
                endpointing_ms: stt.endpointing_ms,
                utterance_end_ms: stt.utterance_end_ms,
                keyterms: stt.keyterms,
                diarize,
                source: s,
            },
        );
        let dump = self.dump_dir.as_ref().and_then(|dir| {
            let path = dump_path(dir, &self.session_id, s.as_str());
            match WavDump::create(&path) {
                Ok(d) => {
                    tracing::info!(path = %path.display(), "debug audio dump enabled");
                    Some(d)
                }
                Err(e) => {
                    tracing::warn!("debug dump unavailable: {e}");
                    None
                }
            }
        });
        let rt = &mut self.src[i];
        rt.ensure_channel();
        let Some(rx) = rt.frames_rx.take() else {
            tracing::error!(source = %s, "frame channel missing; not starting Deepgram");
            return;
        };
        rt.dg_gen += 1;
        let gen = rt.dg_gen;
        let (ctl, ctl_rx) = mpsc::unbounded_channel();
        if rt.muted {
            let _ = ctl.send(DgControl::Mute(true));
        }
        let cfg = DgClientConfig {
            source: s,
            url,
            api_key: self.api_key.clone(),
            first_conn: rt.next_conn,
            timings: self.timings,
            dump,
            rng_seed: None,
        };
        let task = tokio::spawn(run_client(
            cfg,
            gen,
            rx,
            ctl_rx,
            self.dg_tx.clone(),
            rt.stats.clone(),
        ));
        rt.dg = Some(DgHandle { gen, ctl, task });
    }

    /// Ask a source's Deepgram task to CloseStream + flush; it finishes in the background.
    fn stop_dg(&mut self, s: Source) {
        let rt = &mut self.src[s.index()];
        if let Some(dg) = rt.dg.take() {
            let _ = dg.ctl.send(DgControl::Stop);
            self.retiring.push(dg.task);
        }
        rt.frames_tx = None;
        rt.frames_rx = None;
    }

    fn handle_dg(&mut self, s: Source, gen: u32, ev: DgEvent) {
        let i = s.index();
        let current = self.src[i].dg.as_ref().is_some_and(|d| d.gen == gen);
        match ev {
            DgEvent::Connecting { conn, attempt } => {
                let rt = &mut self.src[i];
                rt.next_conn = rt.next_conn.max(conn + 1);
                if current && attempt == 0 {
                    self.emit(SessionEvent::state(Some(s), StreamState::Connecting));
                }
            }
            DgEvent::Live { .. } => {
                if current {
                    self.emit(SessionEvent::state(Some(s), StreamState::Live));
                    if !self.session_live {
                        self.session_live = true;
                        self.emit(SessionEvent::state(None, StreamState::Live));
                    }
                }
            }
            DgEvent::Reconnecting {
                attempt,
                retry_in,
                code,
                message,
            } => {
                if current {
                    let ms = retry_in.as_millis() as u64;
                    self.emit(SessionEvent::State {
                        source: Some(s),
                        state: StreamState::Reconnecting,
                        attempt: Some(attempt),
                        retry_in_ms: Some(ms),
                    });
                    self.emit(SessionEvent::error(Some(s), code, message, false, Some(ms)));
                }
            }
            DgEvent::Transcript(seg) => self.on_transcript(s, seg),
            DgEvent::SpeechStarted { t_ms } => {
                self.emit(SessionEvent::SpeechStarted { source: s, t_ms });
            }
            DgEvent::UtteranceEnd { last_word_end_ms } => {
                self.emit(SessionEvent::UtteranceEnd {
                    source: s,
                    last_word_end_ms,
                });
                if let Some(t) = self.src[i].turns.on_utterance_end() {
                    self.emit_turn(t);
                }
            }
            DgEvent::Fatal { code, message } => {
                if current {
                    self.src[i].dg = None;
                    self.source_failed(s, code, &message);
                }
            }
            DgEvent::Closed => {
                if current {
                    // Ended on its own (shouldn't happen while enabled); treat as stopped and
                    // drop its frame channel so a re-enable starts from a fresh one.
                    self.stop_dg(s);
                }
                let rt = &mut self.src[i];
                if !rt.enabled && rt.dg.is_none() {
                    if let Some(t) = rt.turns.flush() {
                        self.emit_turn(t);
                    }
                    self.emit(SessionEvent::state(Some(s), StreamState::Idle));
                }
            }
        }
    }

    fn on_transcript(&mut self, s: Source, seg: Segment) {
        let i = s.index();
        let empty = seg.text.trim().is_empty();
        {
            let rt = &mut self.src[i];
            if rt.shown_interims.len() > 256 {
                rt.shown_interims.clear();
            }
            if !seg.is_final {
                if empty {
                    // Only forward an empty interim if it clears one we showed.
                    if !rt.shown_interims.contains(&seg.seg_id) {
                        return;
                    }
                } else {
                    rt.shown_interims.insert(seg.seg_id.clone());
                }
            } else {
                let was_shown = rt.shown_interims.remove(&seg.seg_id);
                if empty && !was_shown {
                    // Silence final: nothing to show, but it can still end a turn.
                    if let Some(t) =
                        rt.turns.on_final("", seg.t0_ms, seg.t1_ms, seg.speech_final, seg.from_finalize)
                    {
                        self.emit_turn(t);
                    }
                    return;
                }
            }
        }

        let mut echo = None;
        let mut patches = Vec::new();
        if self.opts.echo_dedup && seg.is_final && !empty {
            match s {
                Source::Me => {
                    echo = Some(self.echo.on_me_final(&seg.seg_id, &seg.text, seg.t0_ms, seg.t1_ms));
                }
                Source::Them => {
                    patches = self.echo.on_them_final(&seg.seg_id, &seg.text, seg.t0_ms, seg.t1_ms);
                }
            }
        }
        self.emit(SessionEvent::Transcript {
            source: s,
            seg_id: seg.seg_id.clone(),
            conn: seg.conn,
            is_final: seg.is_final,
            speech_final: seg.speech_final,
            from_finalize: seg.from_finalize,
            text: seg.text.clone(),
            confidence: seg.confidence,
            t0_ms: seg.t0_ms,
            t1_ms: seg.t1_ms,
            speaker: seg.speaker,
            echo,
        });
        for seg_id in patches {
            self.emit(SessionEvent::TranscriptPatch { seg_id, echo: true });
        }
        if seg.is_final {
            // Echo text never forms a "me" turn.
            let text = if echo == Some(true) { "" } else { seg.text.as_str() };
            if let Some(t) =
                self.src[i].turns.on_final(text, seg.t0_ms, seg.t1_ms, seg.speech_final, seg.from_finalize)
            {
                self.emit_turn(t);
            }
        }
    }

    fn emit_turn(&self, t: Turn) {
        self.emit(SessionEvent::Turn {
            source: t.source,
            turn_id: t.turn_id,
            text: t.text,
            t0_ms: t.t0_ms,
            t1_ms: t.t1_ms,
            reason: t.reason,
            question: t.question,
        });
    }

    // ---- devices -----------------------------------------------------------------------

    fn on_device_event(&mut self, ev: DeviceEvent) {
        match &ev {
            DeviceEvent::DefaultChanged { flow, role, id } => {
                for s in Source::ALL {
                    if default_role_for(s) == (*flow, *role) {
                        self.defaults[s.index()] = Some(id.clone());
                    }
                }
            }
            DeviceEvent::Added(id) | DeviceEvent::StateChanged { id, active: true } => {
                self.available.insert(id.clone());
            }
            DeviceEvent::Removed(id) | DeviceEvent::StateChanged { id, active: false } => {
                self.available.remove(id);
            }
        }
        self.dev_debounce_at = Some(Instant::now() + DEVICE_DEBOUNCE);
    }

    fn evaluate_devices(&mut self) {
        for s in Source::ALL {
            let i = s.index();
            let rt = &self.src[i];
            if !rt.enabled || rt.errored || rt.opening {
                continue;
            }
            let reason = match &rt.device_pref {
                None => match (&self.defaults[i], &rt.last_device) {
                    (Some(Some(new_id)), Some(dev)) if *new_id != dev.device_id => {
                        Some(DeviceReason::DefaultChanged)
                    }
                    // Waiting to restart a lost device and a new default appeared: go now.
                    (Some(Some(_)), _) if rt.capture.is_none() && rt.restart_at.is_some() => {
                        Some(DeviceReason::DefaultChanged)
                    }
                    _ => None,
                },
                Some(pref) => {
                    let fell_back = rt.last_device.as_ref().is_some_and(|d| d.fallback);
                    if fell_back && self.available.contains(pref) {
                        Some(DeviceReason::Opened)
                    } else {
                        None
                    }
                }
            };
            if let Some(reason) = reason {
                tracing::info!(source = %s, ?reason, "device change: restarting capture");
                let rt = &mut self.src[i];
                rt.pending_reason = Some(reason);
                rt.ladder.reset();
                self.spawn_open(s);
            }
        }
    }

    // ---- timers, levels, stats ---------------------------------------------------------

    fn next_deadline(&self) -> Option<Instant> {
        self.src
            .iter()
            .filter_map(|r| r.restart_at)
            .chain(self.dev_debounce_at)
            .min()
    }

    fn on_timer(&mut self) {
        let now = Instant::now();
        if self.dev_debounce_at.is_some_and(|t| t <= now) {
            self.dev_debounce_at = None;
            self.evaluate_devices();
        }
        for s in Source::ALL {
            let rt = &mut self.src[s.index()];
            if rt.restart_at.is_some_and(|t| t <= now) {
                rt.restart_at = None;
                if rt.enabled && !rt.errored {
                    self.spawn_open(s);
                }
            }
        }
    }

    fn emit_levels(&self) {
        let level = |rt: &SourceRt| {
            if rt.enabled && rt.capture.is_some() {
                Some(rt.level.take())
            } else {
                None
            }
        };
        self.emit(SessionEvent::Levels {
            t_ms: self.clock.now_ms(),
            me: level(&self.src[0]),
            them: level(&self.src[1]),
        });
    }

    fn emit_stats(&self) {
        for rt in &self.src {
            if !rt.enabled {
                continue;
            }
            self.emit(SessionEvent::Stats {
                source: rt.source,
                sent_ms: SourceStats::get(&rt.stats.sent_ms),
                dropped_ms: SourceStats::get(&rt.stats.dropped_ms),
                reconnects: SourceStats::get(&rt.stats.reconnects),
                discontinuities: SourceStats::get(&rt.stats.discontinuities),
            });
        }
    }

    // ---- commands ----------------------------------------------------------------------

    fn set_source(&mut self, args: SetSourceArgs) -> AppResult<()> {
        let s = args.source;
        let i = s.index();
        if let Some(dev) = args.device_id {
            let pref = dev.filter(|d| !d.trim().is_empty() && d != "default");
            let rt = &mut self.src[i];
            if rt.device_pref != pref {
                rt.device_pref = pref;
                if rt.enabled && !rt.errored {
                    rt.ladder.reset();
                    rt.pending_reason = None;
                    self.spawn_open(s);
                }
            }
        }
        if let Some(m) = args.muted {
            let rt = &mut self.src[i];
            if rt.muted != m {
                rt.muted = m;
                if let Some(dg) = &rt.dg {
                    let _ = dg.ctl.send(DgControl::Mute(m));
                }
            }
        }
        if let Some(e) = args.enabled {
            let rt = &mut self.src[i];
            if e {
                let running = rt.dg.is_some() || rt.capture.is_some() || rt.opening;
                if !rt.enabled || rt.errored || !running {
                    rt.enabled = true;
                    rt.errored = false;
                    rt.last_cap_error = None;
                    rt.ladder.reset();
                    rt.pending_reason = None;
                    rt.ensure_channel();
                    self.spawn_open(s);
                }
            } else if rt.enabled {
                rt.enabled = false;
                rt.errored = false;
                rt.opening = false;
                rt.restart_at = None;
                rt.cap_gen += 1;
                if let Some(c) = rt.capture.take() {
                    c.stop();
                }
                if rt.dg.is_some() {
                    self.emit(SessionEvent::state(Some(s), StreamState::Stopping));
                    self.stop_dg(s); // state idle follows when its flush completes
                } else {
                    self.stop_dg(s);
                    if let Some(t) = self.src[i].turns.flush() {
                        self.emit_turn(t);
                    }
                    self.emit(SessionEvent::state(Some(s), StreamState::Idle));
                }
            }
        }
        Ok(())
    }

    fn abort_all(&mut self) {
        for rt in &mut self.src {
            rt.cap_gen += 1;
            if let Some(c) = rt.capture.take() {
                c.stop();
            }
            if let Some(dg) = rt.dg.take() {
                dg.task.abort();
            }
        }
        for t in self.retiring.drain(..) {
            t.abort();
        }
    }

    async fn shutdown(mut self) -> SessionSummary {
        tracing::info!(session = %self.session_id, "session stopping");
        self.emit(SessionEvent::state(None, StreamState::Stopping));

        // 1. Stop captures and wait (briefly) for their exits so all audio is queued.
        let mut waiting = 0usize;
        for rt in &mut self.src {
            rt.restart_at = None;
            if let Some(c) = rt.capture.take() {
                c.stop();
                waiting += 1;
            }
        }
        let deadline = Instant::now() + Duration::from_millis(600);
        while waiting > 0 {
            tokio::select! {
                m = self.cap_rx.recv() => match m {
                    Some(CapMsg::Exited { .. }) => waiting -= 1,
                    Some(CapMsg::Opened { result, .. }) => drop(result),
                    None => break,
                },
                _ = sleep_until(deadline) => break,
            }
        }

        // 2. CloseStream on every socket; keep handling their flushed results.
        for s in Source::ALL {
            self.stop_dg(s);
        }
        let tasks = std::mem::take(&mut self.retiring);
        let aborts: Vec<AbortHandle> = tasks.iter().map(|t| t.abort_handle()).collect();
        let joined = futures_util::future::join_all(tasks);
        tokio::pin!(joined);
        let deadline = Instant::now() + self.timings.close_flush + Duration::from_millis(500);
        loop {
            tokio::select! {
                biased;
                ev = self.dg_rx.recv() => match ev {
                    Some((s, gen, ev)) => self.handle_dg(s, gen, ev),
                    None => break,
                },
                _ = &mut joined => break,
                _ = sleep_until(deadline) => {
                    tracing::warn!("deepgram flush timed out at session stop");
                    for a in &aborts { a.abort(); }
                    break;
                }
            }
        }
        while let Ok((s, gen, ev)) = self.dg_rx.try_recv() {
            self.handle_dg(s, gen, ev);
        }

        // 3. Whatever is still pending becomes a turn.
        for s in Source::ALL {
            if let Some(t) = self.src[s.index()].turns.flush() {
                self.emit_turn(t);
            }
        }

        let summary = SessionSummary {
            session_id: self.session_id.clone(),
            duration_ms: self.clock.now_ms().max(0),
            reconnects: self
                .src
                .iter()
                .map(|r| SourceStats::get(&r.stats.reconnects))
                .sum(),
            dropped_ms: self
                .src
                .iter()
                .map(|r| SourceStats::get(&r.stats.dropped_ms))
                .sum(),
        };
        self.hub.set_status(SessionStatus::default());
        self.emit(SessionEvent::state(None, StreamState::Stopped));
        tracing::info!(session = %self.session_id, duration_ms = summary.duration_ms, reconnects = summary.reconnects, dropped_ms = summary.dropped_ms, "session stopped");
        summary
    }
}

#[cfg(test)]
#[path = "supervisor_tests.rs"]
mod tests;
