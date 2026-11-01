//! Debug aid: `PLUELYX_DEBUG_DUMP=1` writes the exact 16 kHz PCM sent to Deepgram per source
//! to `<app log dir>/pluelyx-<session>-<source>.wav`. A tiny hand-rolled WAV writer keeps
//! `hound` a dev-dependency.

use std::fs::File;
use std::io::{BufWriter, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use super::frame::SAMPLE_RATE;

pub const ENV_VAR: &str = "PLUELYX_DEBUG_DUMP";

/// True when the debug dump is switched on.
pub fn enabled() -> bool {
    std::env::var(ENV_VAR)
        .map(|v| {
            let v = v.trim();
            v == "1" || v.eq_ignore_ascii_case("true")
        })
        .unwrap_or(false)
}

/// The dump path for one source of a session.
pub fn dump_path(dir: &Path, session_id: &str, source: &str) -> PathBuf {
    dir.join(format!("pluelyx-{session_id}-{source}.wav"))
}

/// Streaming 16 kHz mono 16-bit WAV writer. Sizes are patched every few seconds and on drop.
pub struct WavDump {
    out: BufWriter<File>,
    data_bytes: u32,
    since_patch: u32,
}

impl WavDump {
    pub fn create(path: &Path) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = BufWriter::new(File::create(path)?);
        out.write_all(&header(0))?;
        Ok(Self {
            out,
            data_bytes: 0,
            since_patch: 0,
        })
    }

    /// Append i16 LE PCM.
    pub fn write(&mut self, pcm: &[u8]) -> std::io::Result<()> {
        self.out.write_all(pcm)?;
        self.data_bytes = self.data_bytes.saturating_add(pcm.len() as u32);
        self.since_patch = self.since_patch.saturating_add(pcm.len() as u32);
        // Patch the header about every 5 s of audio so a crash leaves a playable file.
        if self.since_patch >= SAMPLE_RATE * 2 * 5 {
            self.patch()?;
        }
        Ok(())
    }

    pub fn finish(mut self) -> std::io::Result<()> {
        self.patch()
    }

    fn patch(&mut self) -> std::io::Result<()> {
        self.since_patch = 0;
        self.out.flush()?;
        let file = self.out.get_mut();
        let pos = file.stream_position()?;
        file.seek(SeekFrom::Start(0))?;
        file.write_all(&header(self.data_bytes))?;
        file.seek(SeekFrom::Start(pos))?;
        file.flush()
    }
}

impl Drop for WavDump {
    fn drop(&mut self) {
        let _ = self.patch();
    }
}

fn header(data_bytes: u32) -> [u8; 44] {
    let channels: u16 = 1;
    let bits: u16 = 16;
    let block_align = channels * bits / 8;
    let byte_rate = SAMPLE_RATE * block_align as u32;
    let mut h = [0u8; 44];
    h[0..4].copy_from_slice(b"RIFF");
    h[4..8].copy_from_slice(&(36u32.saturating_add(data_bytes)).to_le_bytes());
    h[8..12].copy_from_slice(b"WAVE");
    h[12..16].copy_from_slice(b"fmt ");
    h[16..20].copy_from_slice(&16u32.to_le_bytes());
    h[20..22].copy_from_slice(&1u16.to_le_bytes()); // PCM
    h[22..24].copy_from_slice(&channels.to_le_bytes());
    h[24..28].copy_from_slice(&SAMPLE_RATE.to_le_bytes());
    h[28..32].copy_from_slice(&byte_rate.to_le_bytes());
    h[32..34].copy_from_slice(&block_align.to_le_bytes());
    h[34..36].copy_from_slice(&bits.to_le_bytes());
    h[36..40].copy_from_slice(b"data");
    h[40..44].copy_from_slice(&data_bytes.to_le_bytes());
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_a_wav_hound_can_read() {
        let dir = std::env::temp_dir().join(format!("pluelyx-dump-test-{}", uuid::Uuid::new_v4()));
        let path = dump_path(&dir, "s1", "me");
        let samples: Vec<i16> = (0..16_000 * 6).map(|i| (i % 2000) as i16 - 1000).collect();
        {
            let mut w = WavDump::create(&path).unwrap();
            for chunk in samples.chunks(800) {
                let pcm: Vec<u8> = chunk.iter().flat_map(|s| s.to_le_bytes()).collect();
                w.write(&pcm).unwrap();
            }
            w.finish().unwrap();
        }
        let mut r = hound::WavReader::open(&path).unwrap();
        let spec = r.spec();
        assert_eq!(spec.channels, 1);
        assert_eq!(spec.sample_rate, 16_000);
        assert_eq!(spec.bits_per_sample, 16);
        let read: Vec<i16> = r.samples::<i16>().map(|s| s.unwrap()).collect();
        assert_eq!(read, samples);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn env_flag() {
        // Only checks parsing of the current value; doesn't mutate the environment.
        let _ = enabled();
        assert!(dump_path(Path::new("C:/logs"), "abc", "them")
            .to_string_lossy()
            .ends_with("pluelyx-abc-them.wav"));
    }
}
