//! Level meters (pure math + a lock-free peak-hold meter).
//!
//! The capture thread calls [`LevelMeter::update`] per frame; the 10 Hz ticker calls
//! [`LevelMeter::take`], which returns the loudest level since the last tick, or a value
//! decaying towards the floor when nothing (or only silence) arrived.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

/// The meter floor (dBFS).
pub const FLOOR_DB: f32 = -90.0;
/// How fast the shown level falls when the input goes quiet (per 100 ms tick).
pub const DECAY_DB_PER_TICK: f32 = 6.0;

/// `{ rmsDb, peakDb }` in dBFS, clamped to -90..0.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Level {
    pub rms_db: f32,
    pub peak_db: f32,
}

impl Level {
    pub const FLOOR: Level = Level {
        rms_db: FLOOR_DB,
        peak_db: FLOOR_DB,
    };
}

/// Linear amplitude (0..1 full scale) → dBFS, clamped to the meter range.
pub fn to_db(amplitude: f32) -> f32 {
    if !(amplitude > 0.0) {
        return FLOOR_DB;
    }
    (20.0 * amplitude.log10()).clamp(FLOOR_DB, 0.0)
}

/// (rms, peak) as linear amplitudes.
pub fn rms_peak_i16(samples: &[i16]) -> (f32, f32) {
    if samples.is_empty() {
        return (0.0, 0.0);
    }
    let mut sumsq = 0.0f64;
    let mut peak = 0.0f32;
    for &s in samples {
        let v = s as f32 / 32768.0;
        peak = peak.max(v.abs());
        sumsq += (v as f64) * (v as f64);
    }
    (((sumsq / samples.len() as f64).sqrt()) as f32, peak)
}

/// (rms, peak) of i16 LE PCM bytes.
pub fn rms_peak_pcm(bytes: &[u8]) -> (f32, f32) {
    let samples: Vec<i16> = bytes
        .chunks_exact(2)
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    rms_peak_i16(&samples)
}

#[cfg(test)]
pub fn measure_i16(samples: &[i16]) -> Level {
    let (rms, peak) = rms_peak_i16(samples);
    Level {
        rms_db: to_db(rms),
        peak_db: to_db(peak),
    }
}

/// Peak-hold meter shared between a capture thread (writer) and the level ticker (reader).
#[derive(Debug)]
pub struct LevelMeter {
    /// Max linear rms since the last `take`, as f32 bits (non-negative floats order like u32).
    rms: AtomicU32,
    peak: AtomicU32,
    shown: Mutex<Level>,
}

impl Default for LevelMeter {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelMeter {
    pub fn new() -> Self {
        Self {
            rms: AtomicU32::new(0),
            peak: AtomicU32::new(0),
            shown: Mutex::new(Level::FLOOR),
        }
    }

    /// Record a frame (i16 LE PCM). Lock-free; safe on the capture thread.
    pub fn update_pcm(&self, pcm: &[u8]) {
        let (rms, peak) = rms_peak_pcm(pcm);
        self.update_linear(rms, peak);
    }

    pub fn update_linear(&self, rms: f32, peak: f32) {
        let rms = if rms.is_finite() { rms.max(0.0) } else { 0.0 };
        let peak = if peak.is_finite() { peak.max(0.0) } else { 0.0 };
        self.rms.fetch_max(rms.to_bits(), Ordering::Relaxed);
        self.peak.fetch_max(peak.to_bits(), Ordering::Relaxed);
    }

    /// The level to show for this tick: the loudest since the last tick, but never falling
    /// faster than [`DECAY_DB_PER_TICK`].
    pub fn take(&self) -> Level {
        let rms = f32::from_bits(self.rms.swap(0, Ordering::Relaxed));
        let peak = f32::from_bits(self.peak.swap(0, Ordering::Relaxed));
        let measured = Level {
            rms_db: to_db(rms),
            peak_db: to_db(peak),
        };
        let mut shown = self.shown.lock().unwrap_or_else(|p| p.into_inner());
        let next = Level {
            rms_db: measured
                .rms_db
                .max(shown.rms_db - DECAY_DB_PER_TICK)
                .max(FLOOR_DB),
            peak_db: measured
                .peak_db
                .max(shown.peak_db - DECAY_DB_PER_TICK)
                .max(FLOOR_DB),
        };
        *shown = next;
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(amplitude: f32, n: usize) -> Vec<i16> {
        (0..n)
            .map(|i| {
                let v = amplitude * (2.0 * std::f32::consts::PI * 440.0 * i as f32 / 16_000.0).sin();
                (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16
            })
            .collect()
    }

    #[test]
    fn full_scale_sine_is_minus_3_db_rms() {
        let l = measure_i16(&sine(0.999, 16_000));
        assert!((l.rms_db - (-3.01)).abs() < 0.1, "rms {}", l.rms_db);
        assert!(l.peak_db > -0.1 && l.peak_db <= 0.0, "peak {}", l.peak_db);
    }

    #[test]
    fn half_scale_sine() {
        let l = measure_i16(&sine(0.5, 16_000));
        // 20*log10(0.5/sqrt(2)) = -9.03
        assert!((l.rms_db - (-9.03)).abs() < 0.1, "rms {}", l.rms_db);
        assert!((l.peak_db - (-6.02)).abs() < 0.1, "peak {}", l.peak_db);
    }

    #[test]
    fn silence_is_floor() {
        assert_eq!(measure_i16(&[0; 800]), Level::FLOOR);
        assert_eq!(measure_i16(&[]), Level::FLOOR);
        assert_eq!(to_db(f32::NAN), FLOOR_DB);
        assert_eq!(to_db(1e-9), FLOOR_DB);
        assert_eq!(to_db(4.0), 0.0);
    }

    #[test]
    fn meter_holds_peak_and_decays() {
        let m = LevelMeter::new();
        let loud: Vec<u8> = sine(0.5, 800).iter().flat_map(|s| s.to_le_bytes()).collect();
        let quiet: Vec<u8> = sine(0.01, 800).iter().flat_map(|s| s.to_le_bytes()).collect();
        m.update_pcm(&quiet);
        m.update_pcm(&loud);
        m.update_pcm(&quiet);
        let first = m.take();
        assert!((first.peak_db - (-6.0)).abs() < 0.2, "{first:?}");
        // Nothing new: decays by 6 dB per tick down to the floor.
        let second = m.take();
        assert!((second.peak_db - (first.peak_db - DECAY_DB_PER_TICK)).abs() < 1e-3);
        for _ in 0..30 {
            m.take();
        }
        assert_eq!(m.take(), Level::FLOOR);
    }

    #[test]
    fn level_serializes_camel_case() {
        let v = serde_json::to_value(Level::FLOOR).unwrap();
        assert_eq!(v, serde_json::json!({"rmsDb": -90.0, "peakDb": -90.0}));
    }
}
