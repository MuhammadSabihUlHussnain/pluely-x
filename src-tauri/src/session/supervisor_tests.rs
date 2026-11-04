//! Session pipeline tests: FakeCapture → supervisor → mock Deepgram → SessionEvents collected
//! through a real `tauri::ipc::Channel`. No audio hardware, no API key.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::sync::mpsc;

use super::super::types::*;
use super::super::SessionManager;
use crate::audio::pump::CapturePump;
use crate::audio::{
    AecState, CaptureBackend, CaptureError, CaptureExit, CaptureRequest, DeviceEvent,
    DeviceRole, DeviceSelector, DeviceWatcher, Flow, OpenedDevice, RunningCapture, Source,
};
use crate::stt::deepgram::client::DgTimings;
use crate::stt::deepgram::mock::{self, Got, MockDeepgram, ServerConn};

// ---- fakes --------------------------------------------------------------------------------

struct FakeCap {
    source: Source,
    device_id: String,
    stop: Arc<AtomicBool>,
    lose: Arc<AtomicBool>,
}

#[derive(Default)]
struct FakeBackend {
    /// Errors to return for the next opens of a source.
    fail_next: Mutex<Vec<(Source, CaptureError)>>,
    /// Current default endpoint per source.
    defaults: Mutex<[String; 2]>,
    /// Endpoints that exist besides the defaults.
    extra: Mutex<HashSet<String>>,
    started: Mutex<Vec<FakeCap>>,
}

impl FakeBackend {
    fn new() -> Arc<Self> {
        let b = FakeBackend::default();
        *b.defaults.lock().unwrap() = ["mic-default".into(), "speakers-default".into()];
        Arc::new(b)
    }

    fn running(&self, s: Source) -> Vec<(String, bool)> {
        self.started
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.source == s)
            .map(|c| (c.device_id.clone(), c.stop.load(Ordering::SeqCst)))
            .collect()
    }

    fn lose_current(&self, s: Source) {
        for c in self.started.lock().unwrap().iter() {
            if c.source == s && !c.stop.load(Ordering::SeqCst) {
                c.lose.store(true, Ordering::SeqCst);
            }
        }
    }
}

impl CaptureBackend for FakeBackend {
    fn start(&self, req: CaptureRequest) -> Result<RunningCapture, CaptureError> {
        {
            let mut f = self.fail_next.lock().unwrap();
            if let Some(pos) = f.iter().position(|(s, _)| *s == req.source) {
                return Err(f.remove(pos).1);
            }
        }
        let default = self.defaults.lock().unwrap()[req.source.index()].clone();
        let (device_id, fallback) = match &req.device {
            DeviceSelector::Default => (default, false),
            DeviceSelector::Id(id) if self.extra.lock().unwrap().contains(id) => (id.clone(), false),
            DeviceSelector::Id(_) => (default, true),
        };
        let device = OpenedDevice {
            name: format!("Fake {}", device_id),
            device_id: device_id.clone(),
            format: "48 kHz 2 ch → 16 kHz mono".into(),
            aec: (req.source == Source::Me).then_some(AecState::Unsupported),
            fallback,
        };
        let stop = Arc::new(AtomicBool::new(false));
        let lose = Arc::new(AtomicBool::new(false));
        self.started.lock().unwrap().push(FakeCap {
            source: req.source,
            device_id,
            stop: stop.clone(),
            lose: lose.clone(),
        });
        let amplitude = if req.source == Source::Them { 0.3 } else { 0.05 };
        let (s2, l2) = (stop.clone(), lose);
        std::thread::spawn(move || fake_capture(req, s2, l2, amplitude));
        Ok(RunningCapture::new(device, stop))
    }
}

fn fake_capture(req: CaptureRequest, stop: Arc<AtomicBool>, lose: Arc<AtomicBool>, amplitude: f32) {
    let CaptureRequest {
        clock,
        frames,
        level,
        stats,
        on_exit,
        ..
    } = req;
    let mut pump = CapturePump::new(frames, level, stats);
    // Like a real device clock: packets are contiguous (t advances exactly 50 ms each) and
    // paced against an absolute schedule so sleep overshoot doesn't accumulate.
    let (t_start, i_start) = (clock.now_ms(), std::time::Instant::now());
    let mut k = 0u32;
    let exit = loop {
        if stop.load(Ordering::SeqCst) {
            break CaptureExit::Stopped;
        }
        if lose.load(Ordering::SeqCst) {
            break CaptureExit::DeviceLost("fake device unplugged".into());
        }
        let n = k as usize * 800;
        let pkt: Vec<f32> = (0..800)
            .map(|i| {
                amplitude * (2.0 * std::f32::consts::PI * 440.0 * (n + i) as f32 / 16_000.0).sin()
            })
            .collect();
        pump.on_packet(&pkt, t_start + k as i64 * 50, false, false);
        k += 1;
        let next = i_start + Duration::from_millis(50) * k;
        if let Some(d) = next.checked_duration_since(std::time::Instant::now()) {
            std::thread::sleep(d);
        }
    };
    on_exit(exit);
}

#[derive(Default)]
struct FakeWatcher {
    subs: Mutex<Vec<mpsc::UnboundedSender<DeviceEvent>>>,
}

impl FakeWatcher {
    fn send(&self, ev: DeviceEvent) {
        for s in self.subs.lock().unwrap().iter() {
            let _ = s.send(ev.clone());
        }
    }
}

impl DeviceWatcher for FakeWatcher {
    fn subscribe(&self) -> mpsc::UnboundedReceiver<DeviceEvent> {
        let (tx, rx) = mpsc::unbounded_channel();
        self.subs.lock().unwrap().push(tx);
        rx
    }
}

// ---- harness ------------------------------------------------------------------------------

type Events = Arc<Mutex<Vec<Value>>>;

fn channel() -> (tauri::ipc::Channel<SessionEvent>, Events) {
    let events: Events = Arc::new(Mutex::new(Vec::new()));
    let e2 = events.clone();
    let ch = tauri::ipc::Channel::new(move |body| {
        if let tauri::ipc::InvokeResponseBody::Json(s) = body {
            e2.lock().unwrap().push(serde_json::from_str(&s).unwrap());
        }
        Ok(())
    });
    (ch, events)
}

fn opts(me: bool, them: bool) -> SessionOptions {
    serde_json::from_value(json!({
        "me": { "enabled": me, "deviceId": null },
        "them": { "enabled": them },
        "stt": { "language": "en", "keyterms": ["Pluely"], "endpointingMs": 300, "utteranceEndMs": 1000, "diarizeThem": false },
        "aec": "auto",
        "echoDedup": true
    }))
    .unwrap()
}

async fn wait_event(events: &Events, what: &str, within: Duration, pred: impl Fn(&Value) -> bool) -> Value {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        if let Some(v) = events.lock().unwrap().iter().find(|v| pred(v)) {
            return v.clone();
        }
        if tokio::time::Instant::now() > deadline {
            let types: Vec<String> = events
                .lock()
                .unwrap()
                .iter()
                .filter(|v| v["type"] != "levels")
                .map(|v| v.to_string())
                .collect();
            panic!("timed out waiting for {what}; events: {types:#?}");
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn count(events: &Events, pred: impl Fn(&Value) -> bool) -> usize {
    events.lock().unwrap().iter().filter(|v| pred(v)).count()
}

fn index_of(events: &Events, pred: impl Fn(&Value) -> bool) -> Option<usize> {
    events.lock().unwrap().iter().position(|v| pred(v))
}

/// Accept both sockets and tell them apart by the `tag` query param.
async fn accept_pair(dg: &mut MockDeepgram) -> (ServerConn, ServerConn) {
    let a = dg.accept(Duration::from_secs(5)).await;
    let b = dg.accept(Duration::from_secs(5)).await;
    let tag_of = |c: &ServerConn| dg.request(c.index).uri;
    if tag_of(&a).contains("tag=pluelyx-me") {
        assert!(tag_of(&b).contains("tag=pluelyx-them"));
        (a, b)
    } else {
        assert!(tag_of(&b).contains("tag=pluelyx-me"));
        (b, a)
    }
}

async fn read_audio(conn: &mut ServerConn, frames: usize) {
    let mut n = 0;
    while n < frames {
        match conn.next_timeout(Duration::from_secs(3)).await {
            Some(Got::Binary(b)) => {
                assert_eq!(b.len(), 1600);
                n += 1;
            }
            Some(_) => {}
            None => panic!("no audio from client"),
        }
    }
}

/// Keep reading (and discarding audio) until a text frame arrives.
async fn next_text(conn: &mut ServerConn, within: Duration) -> Option<String> {
    let deadline = tokio::time::Instant::now() + within;
    loop {
        let left = deadline.saturating_duration_since(tokio::time::Instant::now());
        match conn.next_timeout(left).await? {
            Got::Text(t) => return Some(t),
            Got::Binary(_) => continue,
            Got::Close => return None,
        }
    }
}

fn manager(backend: Arc<FakeBackend>, watcher: Option<Arc<FakeWatcher>>, base: String) -> SessionManager {
    SessionManager::new(backend, watcher.map(|w| w as Arc<dyn DeviceWatcher>), None)
        .with_dg_base_url(base)
        .with_timings(DgTimings {
            close_flush: Duration::from_millis(1500),
            ..DgTimings::default()
        })
}

// ---- tests --------------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn full_pipeline_event_sequence() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let backend = FakeBackend::new();
    let mgr = manager(backend.clone(), None, dg.base_url());
    let (ch, events) = channel();

    let info = mgr.start(opts(true, true), "k".into(), Box::new(ch), None).await.unwrap();
    assert_eq!(info.me.as_ref().unwrap().device_id, "mic-default");
    assert_eq!(info.them.as_ref().unwrap().name, "Fake speakers-default");
    assert_eq!(info.aec, AecState::Unsupported);
    assert!(mgr.status().await.active);

    let (mut me, mut them) = accept_pair(&mut dg).await;
    assert!(dg.requests.lock().unwrap().iter().all(|r| r.authorization.as_deref() == Some("Token k")));
    wait_event(&events, "session live", Duration::from_secs(5), |v| v["type"] == "state" && v["source"].is_null() && v["state"] == "live").await;
    read_audio(&mut them, 10).await;
    read_audio(&mut me, 10).await;

    // "me" hears the speakers: its final arrives first, the matching "them" final later.
    me.send_json(mock::results(0.0, 1.0, "so what would you do", true, false)).await;
    wait_event(&events, "me final", Duration::from_secs(3), |v| v["type"] == "transcript" && v["source"] == "me").await;

    them.send_json(mock::results(0.0, 0.6, "so what", false, false)).await;
    them.send_json(mock::results(0.0, 1.2, "So what would you do?", true, true)).await;
    them.send_json(mock::utterance_end(1.2)).await;
    // Empty interim noise must not reach the UI.
    them.send_json(mock::results(1.2, 1.0, "", false, false)).await;

    let patch = wait_event(&events, "echo patch", Duration::from_secs(3), |v| v["type"] == "transcriptPatch").await;
    assert_eq!(patch, json!({ "type": "transcriptPatch", "segId": "me-1-0", "echo": true }));
    let turn = wait_event(&events, "them turn", Duration::from_secs(3), |v| v["type"] == "turn" && v["source"] == "them").await;
    assert_eq!(turn["text"], "So what would you do?");
    assert_eq!(turn["reason"], "speechFinal");
    assert_eq!(turn["question"], true);
    // Unique per session: `them-t1-<first 8 of the session id>`.
    let tag: String = info.session_id.chars().filter(|c| c.is_ascii_alphanumeric()).take(8).collect();
    assert_eq!(turn["turnId"], format!("them-t1-{tag}"));
    wait_event(&events, "utteranceEnd", Duration::from_secs(3), |v| v["type"] == "utteranceEnd").await;

    // Levels at 10 Hz: "them" plays a 0.3 tone (≈ -10.5 dBFS peak).
    let lv = wait_event(&events, "loud levels", Duration::from_secs(3), |v| {
        v["type"] == "levels" && v["them"]["peakDb"].as_f64().is_some_and(|p| p > -12.0)
    })
    .await;
    assert!(lv["me"].is_object());

    // Mute me → Finalize on its socket.
    mgr.set_source(SetSourceArgs { source: Source::Me, enabled: None, muted: Some(true), device_id: None })
        .await
        .unwrap();
    assert_eq!(next_text(&mut me, Duration::from_secs(3)).await.as_deref(), Some(r#"{"type":"Finalize"}"#));

    // Stop: CloseStream on both, flush, a trailing final becomes a "stop" turn.
    let stopper = tokio::spawn(async move {
        assert_eq!(next_text(&mut them, Duration::from_secs(5)).await.as_deref(), Some(r#"{"type":"CloseStream"}"#));
        them.send_json(mock::results(1.2, 0.8, "And one more thing.", true, false)).await;
        them.send_json(mock::metadata()).await;
        let _ = them.ws.close(None).await;
        assert_eq!(next_text(&mut me, Duration::from_secs(5)).await.as_deref(), Some(r#"{"type":"CloseStream"}"#));
        let _ = me.ws.close(None).await;
    });
    let summary = mgr.stop().await.unwrap();
    stopper.await.unwrap();
    assert_eq!(summary.session_id, info.session_id);
    assert!(summary.duration_ms > 500);
    assert_eq!(summary.reconnects, 0);
    assert!(!mgr.status().await.active);

    // ---- sequence checks ----
    let ev = events.lock().unwrap().clone();
    let non_level: Vec<&Value> = ev.iter().filter(|v| v["type"] != "levels").collect();
    assert_eq!(non_level[0], &json!({ "type": "state", "source": null, "state": "connecting" }));
    assert_eq!(
        non_level.last().unwrap(),
        &&json!({ "type": "state", "source": null, "state": "stopped" })
    );
    let first_transcript = index_of(&events, |v| v["type"] == "transcript").unwrap();
    for s in ["me", "them"] {
        let dev = index_of(&events, |v| v["type"] == "device" && v["source"] == s && v["reason"] == "opened").unwrap();
        assert!(dev < first_transcript, "device {s} opened before transcripts");
        let live = index_of(&events, |v| v["type"] == "state" && v["source"] == s && v["state"] == "live").unwrap();
        let connecting = index_of(&events, |v| v["type"] == "state" && v["source"] == s && v["state"] == "connecting").unwrap();
        assert!(connecting < live);
    }
    let me_final = ev.iter().find(|v| v["type"] == "transcript" && v["source"] == "me").unwrap();
    assert_eq!(me_final["echo"], false, "emitted immediately, patched later");
    assert_eq!(me_final["segId"], "me-1-0");
    assert_eq!(me_final["conn"], 1);
    // Interim then final for the same segment; the empty interim was filtered.
    let them_tr: Vec<&Value> = ev.iter().filter(|v| v["type"] == "transcript" && v["source"] == "them").collect();
    assert_eq!(them_tr[0]["isFinal"], false);
    assert_eq!(them_tr[0]["segId"], them_tr[1]["segId"]);
    assert_eq!(them_tr[1]["isFinal"], true);
    assert!(them_tr.iter().all(|v| v["text"] != ""));
    // Session clock: both sources' segment times are comparable (both started within ~0.5 s).
    let t0_me = me_final["t0Ms"].as_i64().unwrap();
    let t0_them = them_tr[1]["t0Ms"].as_i64().unwrap();
    assert!((t0_me - t0_them).abs() < 500, "me {t0_me} vs them {t0_them}");
    assert!(them_tr[1]["t1Ms"].as_i64().unwrap() - t0_them == 1200);
    // Exactly one turn for speech_final + UtteranceEnd; the flushed final made a "stop" turn.
    let them_turns: Vec<&Value> = ev.iter().filter(|v| v["type"] == "turn" && v["source"] == "them").collect();
    assert_eq!(them_turns.len(), 2, "{them_turns:?}");
    assert_eq!(them_turns[1]["reason"], "stop");
    assert_eq!(them_turns[1]["text"], "And one more thing.");
    assert!(count(&events, |v| v["type"] == "error") == 0, "no errors");
    let stopping = index_of(&events, |v| v["type"] == "state" && v["source"].is_null() && v["state"] == "stopping").unwrap();
    let flushed = index_of(&events, |v| v["type"] == "transcript" && v["text"] == "And one more thing.").unwrap();
    assert!(stopping < flushed, "flush happens during stopping");

    // A reattach after stop gets an inactive snapshot.
    let (ch2, ev2) = channel();
    mgr.reattach(Box::new(ch2)).await;
    assert_eq!(ev2.lock().unwrap()[0], json!({ "type": "snapshot", "status": { "active": false }, "recentFinals": [] }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reattach_mid_session_sends_snapshot_with_recent_finals() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let mgr = manager(FakeBackend::new(), None, dg.base_url());
    let (ch, events) = channel();
    let info = mgr.start(opts(false, true), "k".into(), Box::new(ch), None).await.unwrap();
    assert!(info.me.is_none());
    assert_eq!(info.aec, AecState::Off);
    let mut them = dg.accept(Duration::from_secs(5)).await;
    read_audio(&mut them, 4).await;
    them.send_json(mock::results(0.0, 0.5, "first", true, false)).await;
    them.send_json(mock::results(0.5, 0.5, "second", true, true)).await;
    wait_event(&events, "turn", Duration::from_secs(3), |v| v["type"] == "turn").await;

    let (ch2, ev2) = channel();
    mgr.reattach(Box::new(ch2)).await;
    let snap = ev2.lock().unwrap()[0].clone();
    assert_eq!(snap["type"], "snapshot");
    assert_eq!(snap["status"]["active"], true);
    assert_eq!(snap["status"]["sessionId"], info.session_id);
    let finals: Vec<&str> = snap["recentFinals"].as_array().unwrap().iter().map(|v| v["text"].as_str().unwrap()).collect();
    assert_eq!(finals, vec!["first", "second"]);
    // New events now flow to the new channel.
    wait_event(&ev2, "levels on new channel", Duration::from_secs(2), |v| v["type"] == "levels").await;

    let closer = tokio::spawn(async move {
        let _ = next_text(&mut them, Duration::from_secs(5)).await;
        let _ = them.ws.close(None).await;
    });
    mgr.stop().await.unwrap();
    closer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_fails_when_the_only_source_cant_open() {
    let dg = MockDeepgram::start(vec![]).await;
    let backend = FakeBackend::new();
    backend.fail_next.lock().unwrap().push((Source::Me, CaptureError::PermissionDenied));
    let mgr = manager(backend, None, dg.base_url());
    let (ch, events) = channel();
    let err = mgr.start(opts(true, false), "k".into(), Box::new(ch), None).await.unwrap_err();
    assert_eq!(err.code, "mic_permission_denied");
    assert!(!mgr.status().await.active);
    assert_eq!(dg.attempts(), 0, "no Deepgram socket for a session that never started");
    assert!(count(&events, |v| v["type"] == "error" && v["code"] == "mic_permission_denied" && v["fatal"] == true) == 1);
    assert!(mgr.stop().await.is_err(), "nothing to stop");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_start_reports_the_failed_source() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let backend = FakeBackend::new();
    backend
        .fail_next
        .lock()
        .unwrap()
        .push((Source::Me, CaptureError::DeviceNotFound("no mic".into())));
    let mgr = manager(backend, None, dg.base_url());
    let (ch, events) = channel();
    let info = mgr.start(opts(true, true), "k".into(), Box::new(ch), None).await.unwrap();
    assert!(info.me.is_none() && info.them.is_some());
    let e = wait_event(&events, "me error", Duration::from_secs(2), |v| v["type"] == "error").await;
    assert_eq!(e, json!({ "type": "error", "source": "me", "code": "device_not_found", "message": "No microphone was found.", "fatal": true }));
    let mut them = dg.accept(Duration::from_secs(5)).await;
    read_audio(&mut them, 2).await;
    assert_eq!(dg.attempts(), 1, "only the working source connects");
    // A second start while active is rejected.
    let (ch2, _) = channel();
    assert_eq!(mgr.start(opts(true, true), "k".into(), Box::new(ch2), None).await.unwrap_err().code, "session_active");
    let closer = tokio::spawn(async move {
        let _ = next_text(&mut them, Duration::from_secs(5)).await;
        let _ = them.ws.close(None).await;
    });
    mgr.stop().await.unwrap();
    closer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_lost_restarts_capture_and_keeps_the_socket() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let backend = FakeBackend::new();
    let mgr = manager(backend.clone(), None, dg.base_url());
    let (ch, events) = channel();
    mgr.start(opts(false, true), "k".into(), Box::new(ch), None).await.unwrap();
    let mut them = dg.accept(Duration::from_secs(5)).await;
    read_audio(&mut them, 3).await;

    backend.lose_current(Source::Them);
    let lost = wait_event(&events, "device lost", Duration::from_secs(3), |v| v["type"] == "device" && v["reason"] == "lost").await;
    assert_eq!(lost["deviceId"], "speakers-default");
    let err = wait_event(&events, "device_lost error", Duration::from_secs(3), |v| v["type"] == "error").await;
    assert_eq!(err["code"], "device_lost");
    assert_eq!(err["fatal"], false);
    assert_eq!(err["retryInMs"], 100);
    // Restarted (ladder: 100 ms) and audio flows on the same socket.
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    while count(&events, |v| v["type"] == "device" && v["reason"] == "opened") < 2 {
        assert!(std::time::Instant::now() < deadline, "capture was not reopened");
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let ev = events.lock().unwrap().clone();
    let devs: Vec<&str> = ev
        .iter()
        .filter(|v| v["type"] == "device")
        .map(|v| v["reason"].as_str().unwrap())
        .collect();
    assert_eq!(devs, vec!["opened", "lost", "opened"]);
    read_audio(&mut them, 3).await;
    assert_eq!(dg.attempts(), 1, "the Deepgram socket survived the device restart");
    assert_eq!(backend.running(Source::Them).len(), 2);

    let closer = tokio::spawn(async move {
        let _ = next_text(&mut them, Duration::from_secs(5)).await;
        let _ = them.ws.close(None).await;
    });
    mgr.stop().await.unwrap();
    closer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_output_change_is_debounced_and_followed() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let backend = FakeBackend::new();
    let watcher = Arc::new(FakeWatcher::default());
    let mgr = manager(backend.clone(), Some(watcher.clone()), dg.base_url());
    let (ch, events) = channel();
    mgr.start(opts(false, true), "k".into(), Box::new(ch), None).await.unwrap();
    let mut them = dg.accept(Duration::from_secs(5)).await;
    read_audio(&mut them, 2).await;

    // Headset plugged in: Windows fires a burst of default-changed notifications.
    backend.defaults.lock().unwrap()[1] = "headset".into();
    let t0 = std::time::Instant::now();
    for role in [DeviceRole::Console, DeviceRole::Multimedia, DeviceRole::Communications] {
        watcher.send(DeviceEvent::DefaultChanged { flow: Flow::Render, role, id: Some("headset".into()) });
    }
    // An unrelated capture-side change must not restart "them".
    watcher.send(DeviceEvent::DefaultChanged { flow: Flow::Capture, role: DeviceRole::Communications, id: Some("x".into()) });

    let ev = wait_event(&events, "defaultChanged", Duration::from_secs(3), |v| v["type"] == "device" && v["reason"] == "defaultChanged").await;
    assert!(t0.elapsed() >= Duration::from_millis(380), "debounced ({:?})", t0.elapsed());
    assert_eq!(ev["deviceId"], "headset");
    assert_eq!(backend.running(Source::Them).len(), 2, "exactly one restart for the burst");
    assert_eq!(backend.running(Source::Them)[0].1, true, "old capture stopped");
    read_audio(&mut them, 2).await;
    assert_eq!(dg.attempts(), 1);

    let closer = tokio::spawn(async move {
        let _ = next_text(&mut them, Duration::from_secs(5)).await;
        let _ = them.ws.close(None).await;
    });
    mgr.stop().await.unwrap();
    closer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disable_and_reenable_a_source() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let backend = FakeBackend::new();
    let mgr = manager(backend.clone(), None, dg.base_url());
    let (ch, events) = channel();
    mgr.start(opts(true, true), "k".into(), Box::new(ch), None).await.unwrap();
    let (mut me, mut them) = accept_pair(&mut dg).await;
    read_audio(&mut me, 2).await;
    me.send_json(mock::results(0.0, 0.5, "pending words", true, false)).await;
    wait_event(&events, "me final", Duration::from_secs(3), |v| v["type"] == "transcript" && v["source"] == "me").await;

    mgr.set_source(SetSourceArgs { source: Source::Me, enabled: Some(false), muted: None, device_id: None })
        .await
        .unwrap();
    assert_eq!(next_text(&mut me, Duration::from_secs(3)).await.as_deref(), Some(r#"{"type":"CloseStream"}"#));
    let _ = me.ws.close(None).await;
    wait_event(&events, "me idle", Duration::from_secs(3), |v| v["type"] == "state" && v["source"] == "me" && v["state"] == "idle").await;
    let turn = wait_event(&events, "me stop turn", Duration::from_secs(3), |v| v["type"] == "turn" && v["source"] == "me").await;
    assert_eq!(turn["reason"], "stop");
    assert_eq!(turn["text"], "pending words");

    // Re-enable with a chosen device that doesn't exist → fallback to default.
    mgr.set_source(SetSourceArgs {
        source: Source::Me,
        enabled: Some(true),
        muted: None,
        device_id: Some(Some("usb-mic".into())),
    })
    .await
    .unwrap();
    let fb = wait_event(&events, "fallback", Duration::from_secs(3), |v| v["type"] == "device" && v["reason"] == "fallback").await;
    assert_eq!(fb["deviceId"], "mic-default");
    let mut me2 = dg.accept(Duration::from_secs(5)).await;
    read_audio(&mut me2, 2).await;
    me2.send_json(mock::results(0.0, 0.5, "again", true, false)).await;
    let tr = wait_event(&events, "conn 2 transcript", Duration::from_secs(3), |v| v["type"] == "transcript" && v["text"] == "again").await;
    assert_eq!(tr["conn"], 2, "connection numbering continues per source");
    assert_eq!(tr["segId"], "me-2-0");

    let closer = tokio::spawn(async move {
        for c in [&mut me2, &mut them] {
            let _ = next_text(c, Duration::from_secs(5)).await;
            let _ = c.ws.close(None).await;
        }
    });
    mgr.stop().await.unwrap();
    closer.await.unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn deepgram_auth_failure_is_fatal_for_the_session() {
    let dg = MockDeepgram::start(vec![
        crate::stt::deepgram::mock::Behavior::Reject(401),
        crate::stt::deepgram::mock::Behavior::Reject(401),
    ])
    .await;
    let backend = FakeBackend::new();
    let mgr = manager(backend.clone(), None, dg.base_url());
    let (ch, events) = channel();
    mgr.start(opts(true, true), "bad".into(), Box::new(ch), None).await.unwrap();
    wait_event(&events, "session error", Duration::from_secs(5), |v| v["type"] == "state" && v["source"].is_null() && v["state"] == "error").await;
    assert_eq!(count(&events, |v| v["type"] == "error" && v["code"] == "dg_auth" && v["fatal"] == true), 2);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(dg.attempts(), 2, "no retries after 401");
    assert!(backend.running(Source::Me).iter().all(|(_, stopped)| *stopped), "captures stopped");
    let summary = mgr.stop().await.unwrap();
    assert_eq!(summary.reconnects, 0);
}
