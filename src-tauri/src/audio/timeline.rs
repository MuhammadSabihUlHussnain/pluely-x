//! Session clock and per-connection timelines (pure, apart from reading QPC).
//!
//! - [`SessionClock`]: one origin for the whole session. WASAPI packet timestamps are QPC
//!   values in 100 ns units, so both sources land on the same ms-since-start scale.
//! - [`Timeline`]: Deepgram reports times in seconds of audio received *on that
//!   connection*. Each sent frame records (connection sample offset → session ms), so a
//!   Deepgram time maps back to the session clock even across gaps (silence that was never
//!   sent) and backlog replays. A reconnect starts a fresh Timeline.

use super::frame::SAMPLE_RATE;

/// Converts a raw QPC reading to 100 ns units.
pub fn qpc_to_100ns(counter: i64, frequency: i64) -> u64 {
    if counter <= 0 || frequency <= 0 {
        return 0;
    }
    ((counter as u128 * 10_000_000u128) / frequency as u128) as u64
}

/// The current QPC time in 100 ns units (the unit of WASAPI's `u64QPCPosition`).
#[cfg(windows)]
pub fn qpc_now_100ns() -> u64 {
    use std::sync::OnceLock;
    use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
    static FREQ: OnceLock<i64> = OnceLock::new();
    let freq = *FREQ.get_or_init(|| {
        let mut f = 0i64;
        // SAFETY: plain out-pointer; QueryPerformanceFrequency never fails on XP+.
        let _ = unsafe { QueryPerformanceFrequency(&mut f) };
        f
    });
    let mut c = 0i64;
    // SAFETY: plain out-pointer.
    let _ = unsafe { QueryPerformanceCounter(&mut c) };
    qpc_to_100ns(c, freq)
}

#[cfg(not(windows))]
pub fn qpc_now_100ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    let start = *START.get_or_init(Instant::now);
    (start.elapsed().as_nanos() / 100) as u64 + 1
}

/// The session's single time origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionClock {
    origin_100ns: u64,
}

impl SessionClock {
    pub fn from_origin(origin_100ns: u64) -> Self {
        Self { origin_100ns }
    }

    /// A clock whose zero is now.
    pub fn start() -> Self {
        Self::from_origin(qpc_now_100ns())
    }

    /// A QPC timestamp (100 ns units) → ms since session start (may be negative for audio
    /// captured just before the session started).
    pub fn ts_to_ms(&self, ts_100ns: u64) -> i64 {
        ((ts_100ns as i128 - self.origin_100ns as i128).div_euclid(10_000)) as i64
    }

    pub fn now_ms(&self) -> i64 {
        self.ts_to_ms(qpc_now_100ns())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Run {
    /// Connection sample offset of the run's first sample.
    offset: u64,
    /// Session time of that sample.
    t_ms: i64,
    /// Samples in the run.
    len: u64,
}

/// Connection sample offset → session ms for one Deepgram connection.
#[derive(Debug, Clone, Default)]
pub struct Timeline {
    runs: Vec<Run>,
    total: u64,
}

impl Timeline {
    pub fn new() -> Self {
        Self::default()
    }

    /// Samples sent on this connection so far.
    #[cfg(test)]
    pub fn samples_sent(&self) -> u64 {
        self.total
    }

    /// Record a sent frame that starts at session time `t_ms` and holds `samples` samples.
    /// Contiguous frames are merged into one run.
    pub fn record(&mut self, t_ms: i64, samples: u32) {
        let samples = samples as u64;
        if samples == 0 {
            return;
        }
        if let Some(last) = self.runs.last_mut() {
            let expected = last.t_ms + samples_to_ms_i(last.len);
            if (t_ms - expected).abs() <= 1 {
                last.len += samples;
                self.total += samples;
                return;
            }
        }
        self.runs.push(Run {
            offset: self.total,
            t_ms,
            len: samples,
        });
        self.total += samples;
    }

    /// Number of distinct runs (gaps + 1).
    #[cfg(test)]
    pub fn runs(&self) -> usize {
        self.runs.len()
    }

    /// Deepgram seconds (connection audio time) → session ms. Times past the end extrapolate
    /// from the last run; times before the start extrapolate from the first.
    pub fn to_session_ms(&self, secs: f64) -> i64 {
        let secs = if secs.is_finite() { secs.max(0.0) } else { 0.0 };
        let offset = (secs * SAMPLE_RATE as f64).round() as u64;
        if self.runs.is_empty() {
            return (secs * 1000.0).round() as i64;
        }
        let idx = match self.runs.binary_search_by(|r| r.offset.cmp(&offset)) {
            Ok(i) => i,
            Err(0) => 0,
            Err(i) => i - 1,
        };
        let run = self.runs[idx];
        let delta = offset as i64 - run.offset as i64;
        run.t_ms + (delta * 1000).div_euclid(SAMPLE_RATE as i64)
    }
}

fn samples_to_ms_i(n: u64) -> i64 {
    (n as i64 * 1000) / SAMPLE_RATE as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn qpc_conversion() {
        // 10 MHz QPC: units are already 100 ns.
        assert_eq!(qpc_to_100ns(123_456, 10_000_000), 123_456);
        // 3 MHz: 3 ticks = 1 µs = 10 units.
        assert_eq!(qpc_to_100ns(3, 3_000_000), 10);
        // Large counters don't overflow.
        assert_eq!(qpc_to_100ns(i64::MAX / 2, 10_000_000), (i64::MAX / 2) as u64);
        assert_eq!(qpc_to_100ns(0, 0), 0);
    }

    #[test]
    fn clock_maps_qpc_to_session_ms() {
        let clock = SessionClock::from_origin(50_000_000); // 5 s after boot
        assert_eq!(clock.ts_to_ms(50_000_000), 0);
        assert_eq!(clock.ts_to_ms(50_000_000 + 12_345 * 10_000), 12_345);
        assert_eq!(clock.ts_to_ms(50_000_000 - 10_000), -1);
        assert_eq!(clock.ts_to_ms(50_000_000 - 1), -1, "rounds towards -inf");
    }

    #[test]
    fn real_clock_advances() {
        let clock = SessionClock::start();
        let a = clock.now_ms();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let b = clock.now_ms();
        assert!(a >= 0 && b >= a + 15, "a={a} b={b}");
    }

    #[test]
    fn contiguous_frames_merge() {
        let mut tl = Timeline::new();
        for i in 0..100 {
            tl.record(1_000 + i * 50, 800);
        }
        assert_eq!(tl.runs(), 1);
        assert_eq!(tl.samples_sent(), 80_000);
        assert_eq!(tl.to_session_ms(0.0), 1_000);
        assert_eq!(tl.to_session_ms(2.5), 3_500);
        assert_eq!(tl.to_session_ms(4.999), 5_999);
    }

    #[test]
    fn gaps_map_to_the_right_side() {
        let mut tl = Timeline::new();
        // 1 s of audio at t=0, then nothing sent for 9 s, then 1 s at t=10_000.
        for i in 0..20 {
            tl.record(i * 50, 800);
        }
        for i in 0..20 {
            tl.record(10_000 + i * 50, 800);
        }
        assert_eq!(tl.runs(), 2);
        // Deepgram sees 2 s of audio. 0.5 s → session 500; 1.25 s → session 10_250.
        assert_eq!(tl.to_session_ms(0.5), 500);
        assert_eq!(tl.to_session_ms(1.0), 10_000);
        assert_eq!(tl.to_session_ms(1.25), 10_250);
        // Past the end extrapolates from the last run.
        assert_eq!(tl.to_session_ms(3.0), 12_000);
    }

    #[test]
    fn reconnect_starts_from_zero() {
        let mut old = Timeline::new();
        for i in 0..40 {
            old.record(i * 50, 800);
        }
        // New connection: replayed backlog starting at session t=5_000 is Deepgram t=0.
        let mut new = Timeline::new();
        for i in 0..10 {
            new.record(5_000 + i * 50, 800);
        }
        assert_eq!(old.to_session_ms(0.0), 0);
        assert_eq!(new.to_session_ms(0.0), 5_000);
        assert_eq!(new.to_session_ms(0.25), 5_250);
    }

    #[test]
    fn empty_timeline_is_identity() {
        let tl = Timeline::new();
        assert_eq!(tl.to_session_ms(1.5), 1_500);
        assert_eq!(tl.to_session_ms(f64::NAN), 0);
    }
}
