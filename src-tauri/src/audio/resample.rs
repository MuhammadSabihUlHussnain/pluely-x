//! Fallback converter: interleaved N-channel audio at any rate → 16 kHz mono f32 (pure).
//!
//! Only used when the driver rejects the 16 kHz mono autoconvert format
//! (`AUDCLNT_E_UNSUPPORTED_FORMAT`) and we have to open the mix format instead. Quality is
//! "good enough for speech recognition": downmix, a moving-average low-pass sized to the
//! decimation ratio, then linear interpolation. State carries across packets.

use super::frame::SAMPLE_RATE;

#[derive(Debug, Clone)]
pub struct Resampler {
    channels: usize,
    /// Input samples per output sample.
    step: f64,
    /// Position of the next output sample, in input samples relative to `hist[0]`.
    pos: f64,
    /// Low-passed mono input not yet fully consumed.
    hist: Vec<f32>,
    /// Moving-average window (in input samples) and its running state.
    win: usize,
    ring: Vec<f32>,
    ring_idx: usize,
    ring_sum: f64,
}

impl Resampler {
    pub fn new(in_rate: u32, channels: usize) -> Self {
        let in_rate = in_rate.max(1);
        let step = in_rate as f64 / SAMPLE_RATE as f64;
        let win = if step > 1.0 { step.round().max(1.0) as usize } else { 1 };
        Self {
            channels: channels.max(1),
            step,
            pos: 0.0,
            hist: Vec::new(),
            win,
            ring: vec![0.0; win],
            ring_idx: 0,
            ring_sum: 0.0,
        }
    }

    /// Feed interleaved samples; appends 16 kHz mono output to `out`.
    pub fn process(&mut self, interleaved: &[f32], out: &mut Vec<f32>) {
        for frame in interleaved.chunks_exact(self.channels) {
            let mono = frame.iter().sum::<f32>() / self.channels as f32;
            let filtered = self.lowpass(mono);
            self.hist.push(filtered);
        }
        // Emit while we have the two input samples around `pos`.
        while self.pos + 1.0 < self.hist.len() as f64 {
            let i = self.pos.floor() as usize;
            let frac = (self.pos - i as f64) as f32;
            let a = self.hist[i];
            let b = self.hist[i + 1];
            out.push(a + (b - a) * frac);
            self.pos += self.step;
        }
        // Drop consumed input, keeping the sample `pos` still interpolates from.
        let consumed = (self.pos.floor() as usize).min(self.hist.len());
        if consumed > 0 {
            self.hist.drain(..consumed);
            self.pos -= consumed as f64;
        }
    }

    fn lowpass(&mut self, x: f32) -> f32 {
        if self.win == 1 {
            return x;
        }
        self.ring_sum += x as f64 - self.ring[self.ring_idx] as f64;
        self.ring[self.ring_idx] = x;
        self.ring_idx = (self.ring_idx + 1) % self.win;
        (self.ring_sum / self.win as f64) as f32
    }
}

/// Decode one interleaved packet of the mix format to f32.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SampleFormat {
    F32,
    I16,
    I32,
}

pub fn decode_interleaved(bytes: &[u8], fmt: SampleFormat, out: &mut Vec<f32>) {
    match fmt {
        SampleFormat::F32 => out.extend(
            bytes
                .chunks_exact(4)
                .map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])),
        ),
        SampleFormat::I16 => out.extend(
            bytes
                .chunks_exact(2)
                .map(|c| i16::from_le_bytes([c[0], c[1]]) as f32 / 32768.0),
        ),
        SampleFormat::I32 => out.extend(
            bytes
                .chunks_exact(4)
                .map(|c| i32::from_le_bytes([c[0], c[1], c[2], c[3]]) as f32 / 2_147_483_648.0),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(rate: u32, channels: usize, hz: f32, secs: f32) -> Vec<f32> {
        let n = (rate as f32 * secs) as usize;
        let mut v = Vec::with_capacity(n * channels);
        for i in 0..n {
            let s = 0.5 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin();
            for _ in 0..channels {
                v.push(s);
            }
        }
        v
    }

    fn zero_crossings(v: &[f32]) -> usize {
        v.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count()
    }

    #[test]
    fn downsamples_48k_stereo_in_packets() {
        let input = tone(48_000, 2, 440.0, 1.0);
        let mut r = Resampler::new(48_000, 2);
        let mut out = Vec::new();
        // 10 ms packets (480 frames × 2 ch).
        for pkt in input.chunks(960) {
            r.process(pkt, &mut out);
        }
        assert!((out.len() as i64 - 16_000).abs() <= 2, "len {}", out.len());
        // 440 Hz → ~880 zero crossings per second.
        let zc = zero_crossings(&out[100..]);
        assert!((zc as i64 - 877).abs() <= 6, "zc {zc}");
        // Amplitude survives the low-pass.
        let peak = out[200..].iter().fold(0.0f32, |m, &s| m.max(s.abs()));
        assert!(peak > 0.45 && peak <= 0.51, "peak {peak}");
    }

    #[test]
    fn handles_44100_and_odd_packet_sizes() {
        let input = tone(44_100, 1, 300.0, 0.5);
        let mut r = Resampler::new(44_100, 1);
        let mut out = Vec::new();
        for pkt in input.chunks(441 + 7) {
            r.process(pkt, &mut out);
        }
        assert!((out.len() as i64 - 8_000).abs() <= 2, "len {}", out.len());
    }

    #[test]
    fn upsamples_8k() {
        let input = tone(8_000, 1, 200.0, 0.25);
        let mut r = Resampler::new(8_000, 1);
        let mut out = Vec::new();
        r.process(&input, &mut out);
        assert!((out.len() as i64 - 4_000).abs() <= 2, "len {}", out.len());
    }

    #[test]
    fn decodes_formats() {
        let mut out = Vec::new();
        decode_interleaved(&0.5f32.to_le_bytes(), SampleFormat::F32, &mut out);
        decode_interleaved(&(-16384i16).to_le_bytes(), SampleFormat::I16, &mut out);
        decode_interleaved(&(1_073_741_824i32).to_le_bytes(), SampleFormat::I32, &mut out);
        assert_eq!(out, vec![0.5, -0.5, 0.5]);
    }
}
