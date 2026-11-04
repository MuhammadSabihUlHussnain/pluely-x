//! Integration tests for the Deepgram client against the mock server (no network, no key).

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;
use tokio::task::JoinHandle;

use super::super::config::{build_url, ListenParams};
use super::super::mock::{self, Behavior, Got, MockDeepgram};
use super::*;
use crate::audio::frame::FRAME_SAMPLES;
use crate::audio::{AudioFrame, Source, SourceStats};

const KEY: &str = "test-key-123";

struct Harness {
    frames: mpsc::Sender<AudioFrame>,
    control: mpsc::UnboundedSender<DgControl>,
    events: mpsc::UnboundedReceiver<(Source, u32, DgEvent)>,
    seen: Vec<DgEvent>,
    stats: Arc<SourceStats>,
    task: JoinHandle<DgOutcome>,
}

fn params(source: Source) -> ListenParams {
    ListenParams {
        language: "en".into(),
        endpointing_ms: 300,
        utterance_end_ms: 1000,
        keyterms: vec!["Pluely".into(), "Sabih Ul Hussnain".into()],
        diarize: source == Source::Them,
        source,
    }
}

fn start_client(base: &str, timings: DgTimings) -> Harness {
    start_client_with_dump(base, timings, None)
}

fn start_client_with_dump(
    base: &str,
    timings: DgTimings,
    dump: Option<crate::audio::dump::WavDump>,
) -> Harness {
    let (frames, frx) = mpsc::channel(64);
    let (control, crx) = mpsc::unbounded_channel();
    let (etx, events) = mpsc::unbounded_channel();
    let stats = Arc::new(SourceStats::default());
    let cfg = DgClientConfig {
        source: Source::Them,
        url: build_url(base, &params(Source::Them)),
        api_key: KEY.into(),
        first_conn: 1,
        timings,
        dump,
        rng_seed: Some(7),
    };
    let task = tokio::spawn(run_client(cfg, 1, frx, crx, etx, stats.clone()));
    Harness {
        frames,
        control,
        events,
        seen: Vec::new(),
        stats,
        task,
    }
}

impl Harness {
    /// Wait for an event matching `pred` (all events are kept in `seen`).
    async fn wait_for(&mut self, what: &str, within: Duration, pred: impl Fn(&DgEvent) -> bool) -> DgEvent {
        let deadline = tokio::time::Instant::now() + within;
        loop {
            match tokio::time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some((src, _gen, ev))) => {
                    assert_eq!(src, Source::Them);
                    self.seen.push(ev.clone());
                    if pred(&ev) {
                        return ev;
                    }
                }
                Ok(None) => panic!("event channel closed while waiting for {what}; saw {:?}", self.seen),
                Err(_) => panic!("timed out waiting for {what}; saw {:?}", self.seen),
            }
        }
    }
}

fn frame(seq: u32) -> AudioFrame {
    let mut samples = vec![0i16; FRAME_SAMPLES];
    samples[0] = seq as i16;
    samples[1] = 1234;
    AudioFrame {
        pcm: samples.iter().flat_map(|s| s.to_le_bytes()).collect(),
        t_ms: seq as i64 * 50,
        samples: FRAME_SAMPLES as u32,
        synthetic: false,
    }
}

fn seq_of(pcm: &[u8]) -> u32 {
    i16::from_le_bytes([pcm[0], pcm[1]]) as u32
}

/// Feeds frames every 50 ms (real time) starting at `from`, `count` frames (or forever).
fn feeder(tx: mpsc::Sender<AudioFrame>, from: u32, count: Option<u32>) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut iv = tokio::time::interval(Duration::from_millis(50));
        let mut seq = from;
        loop {
            iv.tick().await;
            if count.is_some_and(|c| seq >= from + c) {
                return;
            }
            if tx.send(frame(seq)).await.is_err() {
                return;
            }
            seq += 1;
        }
    })
}

#[tokio::test]
async fn auth_params_cadence_and_closestream_flush() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let mut h = start_client(&dg.base_url(), DgTimings::default());
    let mut server = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 1 })).await;

    // Auth header + query params.
    let req = dg.requests.lock().unwrap()[0].clone();
    assert_eq!(req.authorization.as_deref(), Some("Token test-key-123"));
    assert!(req.uri.starts_with("/v1/listen?"), "{}", req.uri);
    for p in [
        "model=nova-3",
        "language=en",
        "encoding=linear16",
        "sample_rate=16000",
        "channels=1",
        "interim_results=true",
        "smart_format=true",
        "punctuate=true",
        "endpointing=300",
        "utterance_end_ms=1000",
        "vad_events=true",
        "tag=pluelyx-them",
        "diarize=true",
        "keyterm=Pluely",
        "keyterm=Sabih%20Ul%20Hussnain",
    ] {
        assert!(req.uri.contains(p), "missing {p} in {}", req.uri);
    }

    // 20 frames at 50 ms cadence.
    let feed = feeder(h.frames.clone(), 0, Some(20));
    let mut arrivals = Vec::new();
    while arrivals.len() < 20 {
        match server.next_timeout(Duration::from_secs(3)).await {
            Some(Got::Binary(b)) => {
                assert_eq!(b.len(), FRAME_SAMPLES * 2, "800-sample i16 frames");
                assert_eq!(seq_of(&b), arrivals.len() as u32, "in order");
                arrivals.push(std::time::Instant::now());
            }
            other => panic!("expected audio, got {other:?}"),
        }
    }
    feed.await.unwrap();
    let span = arrivals[19].duration_since(arrivals[1]).as_secs_f64() * 1000.0 / 18.0;
    assert!((35.0..=65.0).contains(&span), "mean frame interval {span:.1} ms");

    // A final result maps to the session clock through the timeline.
    server.send_json(mock::results(0.5, 0.5, "hello there?", true, true)).await;
    let ev = h
        .wait_for("transcript", Duration::from_secs(3), |e| matches!(e, DgEvent::Transcript(_)))
        .await;
    let DgEvent::Transcript(seg) = ev else { unreachable!() };
    assert_eq!(seg.seg_id, "them-1-500");
    assert_eq!((seg.t0_ms, seg.t1_ms), (500, 1000));
    assert!(seg.is_final && seg.speech_final);
    assert_eq!(seg.text, "hello there?");

    // Stop: CloseStream, then the client keeps reading until Deepgram closes.
    h.control.send(DgControl::Stop).unwrap();
    assert_eq!(
        server.next_timeout(Duration::from_secs(3)).await,
        Some(Got::Text(r#"{"type":"CloseStream"}"#.into()))
    );
    server.send_json(mock::results(1.0, 0.3, "flushed words", true, false)).await;
    server.send_json(mock::metadata()).await;
    let _ = server.ws.close(None).await;
    let outcome = tokio::time::timeout(Duration::from_secs(4), h.task).await.unwrap().unwrap();
    assert_eq!(outcome, DgOutcome::Stopped);
    while let Ok((_, _, ev)) = h.events.try_recv() {
        h.seen.push(ev);
    }
    assert!(
        h.seen.iter().any(|e| matches!(e, DgEvent::Transcript(s) if s.text == "flushed words" && s.t0_ms == 1000)),
        "flushed result delivered: {:?}",
        h.seen
    );
    assert_eq!(h.seen.last(), Some(&DgEvent::Closed));
    assert_eq!(SourceStats::get(&h.stats.sent_ms), 1000);
}

#[tokio::test]
async fn stop_gives_up_after_close_flush_when_server_hangs() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let timings = DgTimings {
        close_flush: Duration::from_millis(300),
        ..DgTimings::default()
    };
    let mut h = start_client(&dg.base_url(), timings);
    let mut server = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { .. })).await;
    let t0 = std::time::Instant::now();
    h.control.send(DgControl::Stop).unwrap();
    assert_eq!(
        server.next_timeout(Duration::from_secs(2)).await,
        Some(Got::Text(r#"{"type":"CloseStream"}"#.into()))
    );
    // Server never answers.
    let outcome = tokio::time::timeout(Duration::from_secs(3), h.task).await.unwrap().unwrap();
    assert_eq!(outcome, DgOutcome::Stopped);
    assert!(t0.elapsed() >= Duration::from_millis(280), "waited for the flush window");
}

#[tokio::test]
async fn keepalive_after_4s_without_audio() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let mut h = start_client(&dg.base_url(), DgTimings::default());
    let mut server = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { .. })).await;

    // From here on the clock only moves when everything is idle.
    tokio::time::pause();
    let start = tokio::time::Instant::now();
    // Nothing may arrive during the first ~4 s of idle.
    assert_eq!(server.next_timeout(Duration::from_millis(3_900)).await, None);
    assert_eq!(
        server.next().await,
        Some(Got::Text(r#"{"type":"KeepAlive"}"#.into())),
        "a TEXT KeepAlive frame"
    );
    // (The 4 s count started at connect, a few ms before `start`.)
    assert!(start.elapsed() >= Duration::from_millis(3_900), "{:?}", start.elapsed());
    tokio::time::resume();

    h.control.send(DgControl::Stop).unwrap();
    let _ = server.next_timeout(Duration::from_secs(2)).await;
    let _ = server.ws.close(None).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), h.task).await;
}

#[tokio::test]
async fn drop_reconnects_and_replays_backlog() {
    // Second connection's handshake is delayed so a backlog builds up.
    let mut dg = MockDeepgram::start(vec![Behavior::Accept, Behavior::Delay(Duration::from_millis(500))]).await;
    let mut h = start_client(&dg.base_url(), DgTimings::default());
    let mut s1 = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live 1", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 1 })).await;
    let feed = feeder(h.frames.clone(), 0, None);

    let mut conn1 = Vec::new();
    while conn1.len() < 10 {
        if let Some(Got::Binary(b)) = s1.next_timeout(Duration::from_secs(3)).await {
            conn1.push(seq_of(&b));
        }
    }
    drop(s1); // abrupt drop: no close frame

    let ev = h
        .wait_for("reconnecting", Duration::from_secs(5), |e| matches!(e, DgEvent::Reconnecting { .. }))
        .await;
    match ev {
        DgEvent::Reconnecting { attempt, code, retry_in, .. } => {
            assert_eq!(attempt, 1);
            assert_eq!(code, "dg_network");
            assert!(retry_in <= Duration::from_millis(250), "first retry within base: {retry_in:?}");
        }
        _ => unreachable!(),
    }
    let mut s2 = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live 2", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 2 })).await;
    assert!(h.seen.contains(&DgEvent::Connecting { conn: 2, attempt: 1 }), "{:?}", h.seen);

    // The backlog arrives as a burst, then live frames continue; no gaps, no repeats.
    let mut conn2 = Vec::new();
    let mut burst = 0;
    let first = s2.next_timeout(Duration::from_secs(3)).await;
    let Some(Got::Binary(b)) = first else { panic!("expected replay, got {first:?}") };
    conn2.push(seq_of(&b));
    let t_first = std::time::Instant::now();
    while conn2.len() < 25 {
        if let Some(Got::Binary(b)) = s2.next_timeout(Duration::from_secs(3)).await {
            conn2.push(seq_of(&b));
            if t_first.elapsed() < Duration::from_millis(25) {
                burst += 1;
            }
        }
    }
    assert!(burst >= 5, "backlog replayed as a burst ({burst} frames)");
    assert!(conn2[0] > *conn1.last().unwrap(), "{conn1:?} then {conn2:?}");
    assert!(conn2.windows(2).all(|w| w[1] == w[0] + 1), "contiguous: {conn2:?}");

    // Deepgram time 0 on the new socket = the first replayed frame on the session clock.
    s2.send_json(mock::results(0.0, 0.25, "after reconnect", true, false)).await;
    let ev = h
        .wait_for("transcript", Duration::from_secs(3), |e| matches!(e, DgEvent::Transcript(_)))
        .await;
    let DgEvent::Transcript(seg) = ev else { unreachable!() };
    assert_eq!(seg.conn, 2);
    assert_eq!(seg.seg_id, "them-2-0");
    assert_eq!(seg.t0_ms, conn2[0] as i64 * 50);
    assert_eq!(seg.t1_ms, conn2[0] as i64 * 50 + 250);
    assert_eq!(SourceStats::get(&h.stats.reconnects), 1);

    feed.abort();
    h.control.send(DgControl::Stop).unwrap();
    let _ = s2.ws.close(None).await;
    let _ = tokio::time::timeout(Duration::from_secs(5), h.task).await;
}

#[tokio::test]
async fn http_401_is_fatal_and_not_retried() {
    let dg = MockDeepgram::start(vec![Behavior::Reject(401)]).await;
    let mut h = start_client(&dg.base_url(), DgTimings::default());
    let ev = h.wait_for("fatal", Duration::from_secs(5), |e| matches!(e, DgEvent::Fatal { .. })).await;
    assert!(matches!(ev, DgEvent::Fatal { code: "dg_auth", .. }), "{ev:?}");
    let outcome = tokio::time::timeout(Duration::from_secs(2), h.task).await.unwrap().unwrap();
    assert!(matches!(outcome, DgOutcome::Fatal { code: "dg_auth", .. }));
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(dg.attempts(), 1, "no retry after 401");
    assert!(!h.seen.iter().any(|e| matches!(e, DgEvent::Reconnecting { .. })));
}

#[tokio::test]
async fn http_402_and_400_are_fatal() {
    for (status, code) in [(402u16, "dg_payment"), (400, "dg_bad_request")] {
        let dg = MockDeepgram::start(vec![Behavior::Reject(status)]).await;
        let mut h = start_client(&dg.base_url(), DgTimings::default());
        let ev = h.wait_for("fatal", Duration::from_secs(5), |e| matches!(e, DgEvent::Fatal { .. })).await;
        match ev {
            DgEvent::Fatal { code: c, .. } => assert_eq!(c, code),
            _ => unreachable!(),
        }
        assert_eq!(dg.attempts(), 1);
    }
}

#[tokio::test]
async fn http_429_waits_at_least_5s_by_default() {
    let dg = MockDeepgram::start(vec![Behavior::Reject(429)]).await;
    let mut h = start_client(&dg.base_url(), DgTimings::default());
    let ev = h
        .wait_for("reconnecting", Duration::from_secs(5), |e| matches!(e, DgEvent::Reconnecting { .. }))
        .await;
    match ev {
        DgEvent::Reconnecting { code, retry_in, .. } => {
            assert_eq!(code, "dg_rate_limited");
            assert!(retry_in >= Duration::from_secs(5), "{retry_in:?}");
        }
        _ => unreachable!(),
    }
    // Stop during the wait returns promptly.
    h.control.send(DgControl::Stop).unwrap();
    let outcome = tokio::time::timeout(Duration::from_secs(1), h.task).await.unwrap().unwrap();
    assert_eq!(outcome, DgOutcome::Stopped);
    assert_eq!(dg.attempts(), 1);
}

#[tokio::test]
async fn http_429_is_retried_then_connects() {
    let mut dg = MockDeepgram::start(vec![Behavior::Reject(429), Behavior::Accept]).await;
    let timings = DgTimings {
        rate_limit_min: Duration::from_millis(300),
        ..DgTimings::default()
    };
    let mut h = start_client(&dg.base_url(), timings);
    let ev = h
        .wait_for("reconnecting", Duration::from_secs(5), |e| matches!(e, DgEvent::Reconnecting { .. }))
        .await;
    assert!(matches!(ev, DgEvent::Reconnecting { code: "dg_rate_limited", attempt: 1, retry_in, .. } if retry_in >= Duration::from_millis(300)));
    let _server = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live 2", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 2 })).await;
    assert_eq!(dg.attempts(), 2);
    assert_eq!(SourceStats::get(&h.stats.reconnects), 0, "first successful connection isn't a reconnect");
    h.control.send(DgControl::Stop).unwrap();
}

#[tokio::test]
async fn watchdog_reconnects_a_silent_half_open_socket() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let timings = DgTimings {
        watchdog_audio: Duration::from_millis(600),
        tick: Duration::from_millis(100),
        ..DgTimings::default()
    };
    let mut h = start_client(&dg.base_url(), timings);
    let mut s1 = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live 1", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 1 })).await;
    let feed = feeder(h.frames.clone(), 0, None);
    // The server reads audio but never answers.
    let reader = tokio::spawn(async move { while s1.next().await.is_some() {} });
    let ev = h
        .wait_for("watchdog", Duration::from_secs(5), |e| matches!(e, DgEvent::Reconnecting { .. }))
        .await;
    assert!(matches!(ev, DgEvent::Reconnecting { code: "dg_timeout", .. }), "{ev:?}");
    let _s2 = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live 2", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 2 })).await;
    feed.abort();
    reader.abort();
    h.control.send(DgControl::Stop).unwrap();
}

#[tokio::test]
async fn debug_dump_holds_exactly_the_sent_pcm() {
    let dir = std::env::temp_dir().join(format!("pluelyx-dump-{}", uuid::Uuid::new_v4()));
    let path = crate::audio::dump::dump_path(&dir, "sess", "them");
    let dump = crate::audio::dump::WavDump::create(&path).unwrap();
    let mut dg = MockDeepgram::start(vec![]).await;
    let mut h = start_client_with_dump(&dg.base_url(), DgTimings::default(), Some(dump));
    let mut server = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { .. })).await;
    for seq in 0..4 {
        h.frames.send(frame(seq)).await.unwrap();
    }
    for _ in 0..4 {
        assert!(matches!(server.next_timeout(Duration::from_secs(2)).await, Some(Got::Binary(_))));
    }
    h.control.send(DgControl::Stop).unwrap();
    let _ = server.next_timeout(Duration::from_secs(2)).await; // CloseStream
    let _ = server.ws.close(None).await;
    tokio::time::timeout(Duration::from_secs(5), h.task).await.unwrap().unwrap();

    let mut r = hound::WavReader::open(&path).unwrap();
    assert_eq!(r.spec().sample_rate, 16_000);
    let samples: Vec<i16> = r.samples::<i16>().map(|s| s.unwrap()).collect();
    assert_eq!(samples.len(), 4 * FRAME_SAMPLES);
    for seq in 0..4usize {
        assert_eq!(samples[seq * FRAME_SAMPLES], seq as i16);
        assert_eq!(samples[seq * FRAME_SAMPLES + 1], 1234);
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn mute_sends_finalize_and_drops_audio() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let mut h = start_client(&dg.base_url(), DgTimings::default());
    let mut server = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { .. })).await;
    h.frames.send(frame(0)).await.unwrap();
    assert!(matches!(server.next_timeout(Duration::from_secs(2)).await, Some(Got::Binary(_))));
    h.control.send(DgControl::Mute(true)).unwrap();
    assert_eq!(
        server.next_timeout(Duration::from_secs(2)).await,
        Some(Got::Text(r#"{"type":"Finalize"}"#.into()))
    );
    h.frames.send(frame(1)).await.unwrap();
    assert_eq!(server.next_timeout(Duration::from_millis(300)).await, None, "muted audio isn't sent");
    h.control.send(DgControl::Mute(false)).unwrap();
    // Give the control message a moment to land before the next frame.
    tokio::time::sleep(Duration::from_millis(50)).await;
    h.frames.send(frame(2)).await.unwrap();
    match server.next_timeout(Duration::from_secs(2)).await {
        Some(Got::Binary(b)) => assert_eq!(seq_of(&b), 2),
        other => panic!("{other:?}"),
    }
    h.control.send(DgControl::Stop).unwrap();
}

#[tokio::test]
async fn quiet_finalize_after_speech() {
    let mut dg = MockDeepgram::start(vec![]).await;
    let timings = DgTimings {
        quiet_finalize: Some(Duration::from_millis(200)),
        ..DgTimings::default()
    };
    let mut h = start_client(&dg.base_url(), timings);
    let mut server = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { .. })).await;
    let loud = |seq: u32| {
        let mut f = frame(seq);
        let mut samples: Vec<i16> = (0..FRAME_SAMPLES).map(|i| if i % 2 == 0 { 8000 } else { -8000 }).collect();
        samples[0] = seq as i16;
        f.pcm = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        f
    };
    // Quiet audio before any speech never finalizes.
    for seq in 0..4 {
        h.frames.send(frame(seq)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    // Speech, then a quiet stream that keeps flowing (like loopback after a sentence).
    for seq in 4..8 {
        h.frames.send(loud(seq)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    for seq in 8..20 {
        h.frames.send(frame(seq)).await.unwrap();
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let mut texts = Vec::new();
    let mut binaries = 0;
    while let Some(m) = server.next_timeout(Duration::from_millis(400)).await {
        match m {
            Got::Text(t) => texts.push((binaries, t)),
            Got::Binary(_) => binaries += 1,
            other => panic!("{other:?}"),
        }
    }
    assert_eq!(binaries, 20);
    assert_eq!(texts.len(), 1, "exactly one Finalize: {texts:?}");
    assert_eq!(texts[0].1, r#"{"type":"Finalize"}"#);
    assert!(texts[0].0 >= 10, "sent after the quiet period, not during speech: {texts:?}");
    h.control.send(DgControl::Stop).unwrap();
}

#[test]
fn backlog_holds_ninety_seconds_and_drops_the_oldest_beyond() {
    assert_eq!(DgTimings::default().backlog, Duration::from_secs(90));
    let stats = SourceStats::default();
    let mut b = Backlog::new(DgTimings::default().backlog);
    // 100 s of 50 ms frames while the network is down.
    for i in 0..2000 {
        b.push(frame(i), &stats);
    }
    assert_eq!(b.ms, 90_000, "keeps exactly the newest 90 s");
    assert_eq!(SourceStats::get(&stats.dropped_ms), 10_000);
    // On reconnect the live socket gets the newest 10 s; the other 80 s go to catch-up.
    let older = b.split_older(DgTimings::default().live_replay);
    assert_eq!(older.len(), 1600);
    assert_eq!(seq_of(&older[0].pcm), 200, "oldest kept frame is from t = 10 s");
    assert_eq!(b.ms, 10_000);
    assert_eq!(seq_of(&b.pop().unwrap().pcm), 1800);
}

#[test]
fn quiet_finalize_never_fires_before_the_endpoint() {
    let t = DgTimings::default();
    assert_eq!(t.with_endpointing(300).quiet_finalize, Some(Duration::from_millis(600)));
    assert_eq!(t.with_endpointing(700).quiet_finalize, Some(Duration::from_millis(850)));
    assert_eq!(t.with_endpointing(2000).quiet_finalize, Some(Duration::from_millis(2150)));
    let off = DgTimings { quiet_finalize: None, ..t };
    assert_eq!(off.with_endpointing(700).quiet_finalize, None);
}

#[tokio::test]
async fn long_outage_replays_the_newest_audio_live_and_catches_up_the_rest() {
    // The reconnect handshake takes 1.2 s, so ~1.2 s of audio is buffered; the live socket
    // replays only the newest 0.5 s and the older part goes to a catch-up socket.
    let mut dg = MockDeepgram::start(vec![Behavior::Accept, Behavior::Delay(Duration::from_millis(1200))]).await;
    let timings = DgTimings { live_replay: Duration::from_millis(500), ..DgTimings::default() };
    let mut h = start_client(&dg.base_url(), timings);
    let mut s1 = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live 1", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 1 })).await;
    let feed = feeder(h.frames.clone(), 0, None);
    let mut last1 = 0;
    for _ in 0..6 {
        if let Some(Got::Binary(b)) = s1.next_timeout(Duration::from_secs(3)).await {
            last1 = seq_of(&b);
        }
    }
    drop(s1);

    let mut live = dg.accept(Duration::from_secs(5)).await;
    h.wait_for("live 2", Duration::from_secs(5), |e| matches!(e, DgEvent::Live { conn: 2 })).await;
    let mut catch = dg.accept(Duration::from_secs(5)).await;
    assert!(dg.request(2).uri.contains("tag=pluelyx-them"), "same listen parameters");

    // Catch-up: the older frames, contiguous, then CloseStream.
    let mut older = Vec::new();
    loop {
        match catch.next_timeout(Duration::from_secs(3)).await {
            Some(Got::Binary(b)) => older.push(seq_of(&b)),
            Some(Got::Text(t)) => {
                assert!(t.contains("CloseStream"), "{t}");
                break;
            }
            other => panic!("catch-up socket: unexpected {other:?}"),
        }
    }
    // Live: the newest 0.5 s first, then real-time frames, picking up right after catch-up.
    let mut newer = Vec::new();
    while newer.len() < 15 {
        if let Some(Got::Binary(b)) = live.next_timeout(Duration::from_secs(3)).await {
            newer.push(seq_of(&b));
        }
    }
    feed.abort();
    assert!(older.len() >= 8, "older audio went to catch-up: {older:?}");
    assert_eq!(older[0], last1 + 1, "nothing lost between the dropped socket and catch-up");
    assert!(older.windows(2).all(|w| w[1] == w[0] + 1), "contiguous: {older:?}");
    assert_eq!(newer[0], older.last().unwrap() + 1, "live resumes right after catch-up: {older:?} {newer:?}");
    assert!(newer.windows(2).all(|w| w[1] == w[0] + 1), "contiguous: {newer:?}");
    assert_eq!(SourceStats::get(&h.stats.dropped_ms), 0);

    // Catch-up finals come back as CatchUp events on the session clock, with their own ids.
    catch.send_json(mock::results(0.0, 0.3, "what happened during the outage", true, true)).await;
    catch.send_json(mock::results(0.3, 0.1, "", true, false)).await; // empty finals are dropped
    let ev = h.wait_for("catch-up final", Duration::from_secs(3), |e| matches!(e, DgEvent::CatchUp(_))).await;
    let DgEvent::CatchUp(seg) = ev else { unreachable!() };
    assert_eq!(seg.seg_id, format!("them-{}-0", CATCH_UP_CONN_BASE + 2));
    assert_eq!(seg.text, "what happened during the outage");
    assert_eq!(seg.t0_ms, older[0] as i64 * 50);
    assert!(!h.seen.iter().any(|e| matches!(e, DgEvent::Transcript(_))), "catch-up never feeds the live transcript path");
    drop(catch);
    let _ = h.control.send(DgControl::Stop);
}
