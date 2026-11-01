//! Packet → 50 ms frame assembly (pure).
//!
//! WASAPI hands us packets of arbitrary length (usually 10 ms) with a QPC timestamp. Deepgram
//! wants a steady stream, so we cut it into 800-sample (50 ms) i16 LE frames, each stamped
//! with the session time of its first sample.

use super::AudioFrame;

pub const SAMPLE_RATE: u32 = 16_000;
pub const FRAME_MS: u32 = 50;
pub const FRAME_SAMPLES: usize = (SAMPLE_RATE * FRAME_MS / 1000) as usize; // 800
/// A new packet more than this far ahead of where the buffer expects it starts a new run.
pub const GAP_TOLERANCE_MS: i64 = 30;

/// f32 [-1, 1] → i16 with clamping and rounding.
#[inline]
pub fn f32_to_i16(s: f32) -> i16 {
    if s.is_nan() {
        return 0;
    }
    let v = (s.clamp(-1.0, 1.0) * 32767.0).round();
    v as i16
}

/// i16 samples → little-endian bytes.
pub fn i16_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for s in samples {
        out.extend_from_slice(&s.to_le_bytes());
    }
    out
}

/// Little-endian f32 bytes → f32 samples (appended to `out`).
pub fn le_bytes_to_f32(bytes: &[u8], out: &mut Vec<f32>) {
    out.extend(
        bytes
            .chunks_exact(4)
            .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
    );
}

/// At a frame boundary (empty buffer), a packet further than this from the expected time
/// re-anchors the run to the packet's own timestamp (absorbs clock drift and real gaps
/// without inserting padding).
pub const ANCHOR_TOLERANCE_MS: i64 = 20;

/// Converts samples (session-timed) into fixed 50 ms frames.
#[derive(Debug, Default)]
pub struct FrameAssembler {
    buf: Vec<i16>,
    /// Session time of `buf[0]` (or of the next sample when `buf` is empty).
    buf_t_ms: i64,
    /// Whether a run is in progress (false until the first push and after `flush`).
    running: bool,
    /// `buf[..real_until]` may hold captured samples; everything after is tail-fill.
    real_until: usize,
}

impl FrameAssembler {
    pub fn new() -> Self {
        Self {
            buf: Vec::with_capacity(FRAME_SAMPLES * 2),
            buf_t_ms: 0,
            running: false,
            real_until: 0,
        }
    }

    /// Session time the next sample is expected at, if a run is in progress.
    pub fn expected_t_ms(&self) -> Option<i64> {
        if self.running {
            Some(self.buf_t_ms + samples_to_ms(self.buf.len()))
        } else {
            None
        }
    }

    #[cfg(test)]
    pub fn buffered_samples(&self) -> usize {
        self.buf.len()
    }

    /// Push f32 samples starting at `t_ms`. `silent` zeroes them (the driver's SILENT flag
    /// means the buffer content is undefined). `synthetic` marks tail-fill.
    pub fn push_f32(
        &mut self,
        samples: &[f32],
        t_ms: i64,
        silent: bool,
        synthetic: bool,
        emit: &mut dyn FnMut(AudioFrame),
    ) {
        if samples.is_empty() {
            return;
        }
        self.begin(t_ms, emit);
        if silent {
            self.buf.resize(self.buf.len() + samples.len(), 0);
        } else {
            self.buf.extend(samples.iter().map(|&s| f32_to_i16(s)));
        }
        if !synthetic {
            self.real_until = self.buf.len();
        }
        self.drain_full(emit);
    }

    /// Push `n` zero samples starting at `t_ms` (tail-fill when `synthetic`).
    pub fn push_zeros(
        &mut self,
        n: usize,
        t_ms: i64,
        synthetic: bool,
        emit: &mut dyn FnMut(AudioFrame),
    ) {
        if n == 0 {
            return;
        }
        self.begin(t_ms, emit);
        self.buf.resize(self.buf.len() + n, 0);
        if !synthetic {
            self.real_until = self.buf.len();
        }
        self.drain_full(emit);
    }

    /// Emit any partial frame, zero-padded to 800 samples, and end the run.
    pub fn flush(&mut self, emit: &mut dyn FnMut(AudioFrame)) {
        if !self.buf.is_empty() {
            self.buf.resize(FRAME_SAMPLES, 0);
            self.drain_full(emit);
        }
        self.buf.clear();
        self.real_until = 0;
        self.running = false;
    }

    /// Start or continue a run at `t_ms`.
    /// - Mid-frame, jitter and overlaps continue the run; a gap > 30 ms flushes the partial
    ///   frame (zero-padded) and starts a new run.
    /// - At a frame boundary, a packet > 20 ms off re-anchors the run (no padding needed).
    fn begin(&mut self, t_ms: i64, emit: &mut dyn FnMut(AudioFrame)) {
        match self.expected_t_ms() {
            None => {
                self.buf_t_ms = t_ms;
                self.running = true;
            }
            Some(expected) if self.buf.is_empty() => {
                if (t_ms - expected).abs() > ANCHOR_TOLERANCE_MS {
                    self.buf_t_ms = t_ms;
                }
            }
            Some(expected) if t_ms > expected + GAP_TOLERANCE_MS => {
                self.flush(emit);
                self.buf_t_ms = t_ms;
                self.running = true;
            }
            Some(_) => {}
        }
    }

    fn drain_full(&mut self, emit: &mut dyn FnMut(AudioFrame)) {
        let mut start = 0;
        while self.buf.len() - start >= FRAME_SAMPLES {
            let chunk = &self.buf[start..start + FRAME_SAMPLES];
            emit(AudioFrame {
                pcm: i16_to_le_bytes(chunk),
                t_ms: self.buf_t_ms,
                samples: FRAME_SAMPLES as u32,
                synthetic: self.real_until <= start,
            });
            start += FRAME_SAMPLES;
            self.buf_t_ms += FRAME_MS as i64;
        }
        if start > 0 {
            self.buf.drain(..start);
            self.real_until = self.real_until.saturating_sub(start);
        }
    }
}

pub fn samples_to_ms(n: usize) -> i64 {
    (n as i64 * 1000) / SAMPLE_RATE as i64
}

pub fn ms_to_samples(ms: i64) -> usize {
    ((ms.max(0) as u64 * SAMPLE_RATE as u64) / 1000) as usize
}

#[cfg(test)]
mod tests {
    use super::*;

    fn collect(asm: &mut FrameAssembler, f: impl FnOnce(&mut FrameAssembler, &mut dyn FnMut(AudioFrame))) -> Vec<AudioFrame> {
        let mut out = Vec::new();
        f(asm, &mut |fr| out.push(fr));
        out
    }

    fn pcm_i16(frame: &AudioFrame) -> Vec<i16> {
        frame
            .pcm
            .chunks_exact(2)
            .map(|c| i16::from_le_bytes([c[0], c[1]]))
            .collect()
    }

    #[test]
    fn f32_to_i16_clamps_and_rounds() {
        assert_eq!(f32_to_i16(0.0), 0);
        assert_eq!(f32_to_i16(1.0), 32767);
        assert_eq!(f32_to_i16(-1.0), -32767);
        assert_eq!(f32_to_i16(2.5), 32767);
        assert_eq!(f32_to_i16(-7.0), -32767);
        assert_eq!(f32_to_i16(0.5), 16384); // 16383.5 rounds away from zero
        assert_eq!(f32_to_i16(f32::NAN), 0);
    }

    #[test]
    fn le_encoding() {
        assert_eq!(i16_to_le_bytes(&[1, -2, 0x1234]), vec![1, 0, 0xFE, 0xFF, 0x34, 0x12]);
        let mut out = Vec::new();
        le_bytes_to_f32(&0.25f32.to_le_bytes(), &mut out);
        assert_eq!(out, vec![0.25]);
    }

    #[test]
    fn merges_small_packets_into_50ms_frames() {
        let mut asm = FrameAssembler::new();
        let mut frames = Vec::new();
        // 12 packets of 10 ms (160 samples) starting at t=1000 → 2 full frames + 40 ms left.
        for i in 0..12 {
            let pkt = vec![0.1f32; 160];
            asm.push_f32(&pkt, 1000 + i * 10, false, false, &mut |f| frames.push(f));
        }
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].t_ms, 1000);
        assert_eq!(frames[1].t_ms, 1050);
        for f in &frames {
            assert_eq!(f.samples, 800);
            assert_eq!(f.pcm.len(), 1600);
            assert!(!f.synthetic);
        }
        assert_eq!(asm.buffered_samples(), 320);
        assert_eq!(asm.expected_t_ms(), Some(1120));
    }

    #[test]
    fn splits_large_packet() {
        let mut asm = FrameAssembler::new();
        let pkt: Vec<f32> = (0..2000).map(|i| (i % 100) as f32 / 1000.0).collect();
        let frames = collect(&mut asm, |a, e| a.push_f32(&pkt, 0, false, false, e));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].t_ms, 0);
        assert_eq!(frames[1].t_ms, 50);
        assert_eq!(pcm_i16(&frames[1])[0], f32_to_i16(pkt[800]));
        assert_eq!(asm.buffered_samples(), 400);
    }

    #[test]
    fn silent_flag_zeroes() {
        let mut asm = FrameAssembler::new();
        let pkt = vec![0.9f32; 800];
        let frames = collect(&mut asm, |a, e| a.push_f32(&pkt, 0, true, false, e));
        assert_eq!(frames.len(), 1);
        assert!(pcm_i16(&frames[0]).iter().all(|&s| s == 0));
        assert!(!frames[0].synthetic, "silent packets are still captured audio");
    }

    #[test]
    fn gap_flushes_padded_partial_and_restarts_timing() {
        let mut asm = FrameAssembler::new();
        let mut frames = Vec::new();
        asm.push_f32(&vec![0.5; 300], 0, false, false, &mut |f| frames.push(f));
        assert!(frames.is_empty());
        // Next packet arrives 500 ms later: the 300-sample partial is flushed (padded).
        asm.push_f32(&vec![0.5; 800], 500, false, false, &mut |f| frames.push(f));
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].t_ms, 0);
        let first = pcm_i16(&frames[0]);
        assert!(first[..300].iter().all(|&s| s != 0));
        assert!(first[300..].iter().all(|&s| s == 0));
        assert_eq!(frames[1].t_ms, 500);
    }

    #[test]
    fn jitter_and_overlap_continue_the_run() {
        let mut asm = FrameAssembler::new();
        let mut frames = Vec::new();
        asm.push_f32(&vec![0.1; 400], 0, false, false, &mut |f| frames.push(f));
        // expected 25 ms: +15 ms jitter continues, then a -17 ms overlap continues too.
        asm.push_f32(&vec![0.1; 200], 40, false, false, &mut |f| frames.push(f));
        asm.push_f32(&vec![0.1; 200], 20, false, false, &mut |f| frames.push(f));
        // Frame boundary: 5 ms off continues at 50 ...
        asm.push_f32(&vec![0.1; 800], 55, false, false, &mut |f| frames.push(f));
        // ... 100 ms off re-anchors without padding.
        asm.push_f32(&vec![0.1; 800], 200, false, false, &mut |f| frames.push(f));
        let ts: Vec<i64> = frames.iter().map(|f| f.t_ms).collect();
        assert_eq!(ts, vec![0, 50, 200]);
        assert!(frames.iter().all(|f| pcm_i16(f).iter().all(|&s| s != 0)), "no padding");
    }

    #[test]
    fn synthetic_marking() {
        let mut asm = FrameAssembler::new();
        let mut frames = Vec::new();
        // 300 real samples then 2000 synthetic zeros: the first frame mixes, so it's real.
        asm.push_f32(&vec![0.2; 300], 0, false, false, &mut |f| frames.push(f));
        asm.push_zeros(2000, 18, true, &mut |f| frames.push(f));
        assert_eq!(frames.len(), 2);
        assert!(!frames[0].synthetic);
        assert!(frames[1].synthetic);
        asm.flush(&mut |f| frames.push(f));
        assert_eq!(frames.len(), 3);
        assert!(frames[2].synthetic);
        assert_eq!(frames[2].t_ms, 100);
    }

    #[test]
    fn sample_ms_conversions() {
        assert_eq!(samples_to_ms(800), 50);
        assert_eq!(samples_to_ms(16_000), 1000);
        assert_eq!(ms_to_samples(100), 1600);
        assert_eq!(ms_to_samples(-5), 0);
    }
}
