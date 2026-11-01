//! The capture-thread side of the pipeline, shared by the WASAPI backend and test fakes:
//! packets → [`FrameAssembler`] → level meter → `try_send` into the source's channel, plus
//! tail-fill (docs/NATIVE_DESIGN.md "Silence").
//!
//! Tail-fill: when real packets stop arriving (loopback delivers nothing while nothing
//! plays), send up to 1.5 s of synthetic zeros in 100 ms chunks, stamped on the session
//! clock, so Deepgram's endpointing / UtteranceEnd fire for a sentence that just ended.
//! After that nothing is sent; the socket's KeepAlive holds the connection.

use std::sync::Arc;

use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::mpsc::Sender;

use super::frame::{ms_to_samples, samples_to_ms, FrameAssembler, FRAME_MS};
use super::level::LevelMeter;
use super::{AudioFrame, SourceStats};

/// Total synthetic audio after the last real packet.
pub const TAIL_FILL_MS: i64 = 1_500;
/// Tail-fill granularity.
pub const TAIL_CHUNK_MS: i64 = 100;
/// Quiet time before tail-fill starts.
pub const TAIL_START_AFTER_MS: i64 = 100;
/// Fill only up to `now - margin`, so a packet that's already in flight doesn't overlap.
pub const TAIL_MARGIN_MS: i64 = 50;

pub struct CapturePump {
    asm: FrameAssembler,
    tx: Sender<AudioFrame>,
    level: Arc<LevelMeter>,
    stats: Arc<SourceStats>,
    /// Session time just after the last real sample.
    last_real_end_ms: Option<i64>,
    /// Tail-fill has covered up to here.
    fill_until_ms: i64,
    packets: u64,
    frames_sent: u64,
}

impl CapturePump {
    pub fn new(tx: Sender<AudioFrame>, level: Arc<LevelMeter>, stats: Arc<SourceStats>) -> Self {
        Self {
            asm: FrameAssembler::new(),
            tx,
            level,
            stats,
            last_real_end_ms: None,
            fill_until_ms: 0,
            packets: 0,
            frames_sent: 0,
        }
    }

    pub fn frames_sent(&self) -> u64 {
        self.frames_sent
    }

    /// A captured packet of 16 kHz mono f32 starting at session time `t_ms`.
    pub fn on_packet(&mut self, samples: &[f32], t_ms: i64, silent: bool, discontinuity: bool) {
        if samples.is_empty() {
            return;
        }
        // The first packet after Start() always carries DATA_DISCONTINUITY; ignore it.
        if discontinuity && self.packets > 0 {
            SourceStats::add(&self.stats.discontinuities, 1);
        }
        self.packets += 1;
        let (tx, level, stats, sent) = (&self.tx, &self.level, &self.stats, &mut self.frames_sent);
        self.asm.push_f32(samples, t_ms, silent, false, &mut |frame| {
            send(tx, level, stats, sent, frame);
        });
        let end = t_ms + samples_to_ms(samples.len());
        self.last_real_end_ms = Some(end);
        self.fill_until_ms = end;
    }

    /// Called after every wait (timeout or not) with the current session time.
    pub fn on_idle(&mut self, now_ms: i64) {
        let Some(last_end) = self.last_real_end_ms else {
            return;
        };
        if now_ms - last_end < TAIL_START_AFTER_MS {
            return;
        }
        let limit = (now_ms - TAIL_MARGIN_MS).min(last_end + TAIL_FILL_MS);
        let (tx, level, stats, sent) = (&self.tx, &self.level, &self.stats, &mut self.frames_sent);
        while self.fill_until_ms + TAIL_CHUNK_MS <= limit {
            self.asm.push_zeros(
                ms_to_samples(TAIL_CHUNK_MS),
                self.fill_until_ms,
                true,
                &mut |frame| send(tx, level, stats, sent, frame),
            );
            self.fill_until_ms += TAIL_CHUNK_MS;
        }
        if self.fill_until_ms + TAIL_CHUNK_MS > last_end + TAIL_FILL_MS {
            // Done: push out the last partial frame so nothing lingers in the buffer.
            self.asm
                .flush(&mut |frame| send(tx, level, stats, sent, frame));
        }
    }
}

fn send(
    tx: &Sender<AudioFrame>,
    level: &LevelMeter,
    stats: &SourceStats,
    sent: &mut u64,
    frame: AudioFrame,
) {
    if !frame.synthetic {
        level.update_pcm(&frame.pcm);
    }
    match tx.try_send(frame) {
        Ok(()) => *sent += 1,
        Err(TrySendError::Full(_)) | Err(TrySendError::Closed(_)) => {
            SourceStats::add(&stats.dropped_ms, FRAME_MS as u64);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::frame::FRAME_SAMPLES;
    use tokio::sync::mpsc;

    fn pump(cap: usize) -> (CapturePump, mpsc::Receiver<AudioFrame>, Arc<SourceStats>, Arc<LevelMeter>) {
        let (tx, rx) = mpsc::channel(cap);
        let stats = Arc::new(SourceStats::default());
        let level = Arc::new(LevelMeter::new());
        (CapturePump::new(tx, level.clone(), stats.clone()), rx, stats, level)
    }

    fn drain(rx: &mut mpsc::Receiver<AudioFrame>) -> Vec<AudioFrame> {
        let mut v = Vec::new();
        while let Ok(f) = rx.try_recv() {
            v.push(f);
        }
        v
    }

    #[test]
    fn packets_become_frames_and_levels() {
        let (mut p, mut rx, _stats, level) = pump(64);
        for i in 0..10 {
            p.on_packet(&vec![0.25; 160], i * 10, false, i == 0);
        }
        let frames = drain(&mut rx);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.samples as usize == FRAME_SAMPLES && !f.synthetic));
        let l = level.take();
        assert!((l.peak_db - (-12.04)).abs() < 0.1, "{l:?}");
    }

    #[test]
    fn tail_fill_is_bounded_and_timestamped() {
        let (mut p, mut rx, _stats, _level) = pump(64);
        // 200 ms of speech ending at t=1200.
        for i in 0..20 {
            p.on_packet(&vec![0.3; 160], 1000 + i * 10, false, false);
        }
        assert_eq!(drain(&mut rx).len(), 4);
        // Not yet quiet long enough.
        p.on_idle(1250);
        assert!(drain(&mut rx).is_empty());
        // Silence for a long time, observed in 100 ms idle ticks.
        let mut fill = Vec::new();
        let mut now = 1300;
        while now <= 6000 {
            p.on_idle(now);
            fill.extend(drain(&mut rx));
            now += 100;
        }
        assert!(fill.iter().all(|f| f.synthetic));
        let total_ms: u64 = fill.iter().map(|f| f.duration_ms()).sum();
        assert_eq!(total_ms, 1500, "exactly 1.5 s of tail-fill");
        assert_eq!(fill[0].t_ms, 1200);
        for w in fill.windows(2) {
            assert_eq!(w[1].t_ms - w[0].t_ms, 50);
        }
    }

    #[test]
    fn tail_fill_never_runs_ahead_of_now() {
        let (mut p, mut rx, _s, _l) = pump(64);
        p.on_packet(&vec![0.3; 800], 0, false, false);
        drain(&mut rx);
        p.on_idle(260); // limit = min(210, 1550) → fills 50..150 (one chunk 50..150? chunks of 100)
        let f = drain(&mut rx);
        let covered: i64 = f.iter().map(|f| f.duration_ms() as i64).sum();
        assert!(f.iter().all(|fr| fr.t_ms + 50 <= 210), "{f:?}");
        assert!(covered <= 160);
    }

    #[test]
    fn real_audio_after_fill_resets_it() {
        let (mut p, mut rx, _s, _l) = pump(64);
        p.on_packet(&vec![0.3; 800], 0, false, false);
        for now in (100..=3000).step_by(100) {
            p.on_idle(now);
        }
        drain(&mut rx);
        // Speech resumes at t=5000, then stops again: another 1.5 s of fill.
        p.on_packet(&vec![0.3; 800], 5_000, false, false);
        let resumed = drain(&mut rx);
        assert_eq!(resumed.len(), 1);
        assert_eq!(resumed[0].t_ms, 5_000);
        assert!(!resumed[0].synthetic);
        for now in (5_100..=9_000).step_by(100) {
            p.on_idle(now);
        }
        let fill: u64 = drain(&mut rx).iter().map(|f| f.duration_ms()).sum();
        assert_eq!(fill, 1500);
    }

    #[test]
    fn no_fill_before_any_audio() {
        let (mut p, mut rx, _s, _l) = pump(64);
        for now in (0..5000).step_by(100) {
            p.on_idle(now);
        }
        assert!(drain(&mut rx).is_empty());
    }

    #[test]
    fn full_channel_counts_dropped_ms() {
        let (mut p, mut rx, stats, _l) = pump(2);
        p.on_packet(&vec![0.1; 800 * 5], 0, false, false);
        assert_eq!(drain(&mut rx).len(), 2);
        assert_eq!(SourceStats::get(&stats.dropped_ms), 150);
    }

    #[test]
    fn discontinuities_counted_after_first_packet() {
        let (mut p, _rx, stats, _l) = pump(64);
        p.on_packet(&[0.0; 160], 0, false, true);
        p.on_packet(&[0.0; 160], 10, false, true);
        assert_eq!(SourceStats::get(&stats.discontinuities), 1);
    }
}
