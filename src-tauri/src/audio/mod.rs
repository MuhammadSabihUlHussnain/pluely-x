//! Audio capture for the live session (docs/NATIVE_DESIGN.md "Audio pipeline").
//!
//! Pure, platform-independent pieces: [`frame`] (50 ms framing), [`level`] (meters),
//! [`timeline`] (session clock + per-connection sample→time map), [`resample`] (fallback
//! converter), [`pump`] (packet → frame → channel logic incl. tail-fill) and [`dump`]
//! (debug WAV writer). The WASAPI backend lives in [`win`], the Core Audio one in [`mac`].
//!
//! The supervisor only talks to capture through [`CaptureBackend`], so tests inject fakes.

pub mod dump;
pub mod frame;
pub mod level;
pub mod pump;
pub mod resample;
pub mod timeline;
#[cfg(target_os = "macos")]
pub mod mac;
#[cfg(windows)]
pub mod win;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::AppError;
use level::LevelMeter;
use timeline::SessionClock;

/// Which side of the conversation a stream carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    /// The user's microphone.
    Me,
    /// Loopback of the default (console) output: the other people.
    Them,
}

impl Source {
    pub const ALL: [Source; 2] = [Source::Me, Source::Them];

    pub fn as_str(self) -> &'static str {
        match self {
            Source::Me => "me",
            Source::Them => "them",
        }
    }

    pub fn index(self) -> usize {
        match self {
            Source::Me => 0,
            Source::Them => 1,
        }
    }
}

impl std::fmt::Display for Source {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One frame of 16 kHz mono PCM, i16 little-endian, stamped on the session clock.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioFrame {
    /// `samples * 2` bytes of i16 LE PCM.
    pub pcm: Vec<u8>,
    /// Session time (ms since session start) of the first sample.
    pub t_ms: i64,
    /// Number of samples (800 for a full 50 ms frame).
    pub samples: u32,
    /// True when the frame holds only tail-fill zeros, not captured audio.
    pub synthetic: bool,
}

impl AudioFrame {
    pub fn duration_ms(&self) -> u64 {
        (self.samples as u64 * 1000) / frame::SAMPLE_RATE as u64
    }
}

/// Which endpoint a source should capture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSelector {
    /// Follow the default: Communications capture device for `me`, Console render device
    /// (loopback) for `them`.
    Default,
    /// A specific endpoint id (falls back to the default if it's missing).
    Id(String),
}

impl DeviceSelector {
    pub fn from_option(id: Option<&str>) -> Self {
        match id.map(str::trim) {
            None | Some("") | Some("default") => DeviceSelector::Default,
            Some(id) => DeviceSelector::Id(id.to_string()),
        }
    }
}

/// The `aec` session option.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum AecMode {
    #[default]
    Auto,
    On,
    Off,
}

/// What the mic stream actually got.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AecState {
    On,
    Off,
    Unsupported,
}

/// The endpoint a capture opened.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenedDevice {
    pub device_id: String,
    pub name: String,
    /// Human-readable format, e.g. "48 kHz 2 ch → 16 kHz mono".
    pub format: String,
    /// Mic only.
    pub aec: Option<AecState>,
    /// True when the chosen device was missing and the default was opened instead.
    pub fallback: bool,
}

/// Why a running capture thread ended.
#[derive(Debug, Clone, PartialEq)]
pub enum CaptureExit {
    /// Stop was requested.
    Stopped,
    /// The endpoint went away or the stream broke; the supervisor restarts with backoff.
    DeviceLost(String),
    /// Mic access was revoked (Windows privacy setting / macOS Privacy & Security).
    PermissionDenied,
}

/// Why a capture couldn't start.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum CaptureError {
    #[error("Microphone access is turned off in the system privacy settings")]
    PermissionDenied,
    #[error("No audio device found: {0}")]
    DeviceNotFound(String),
    #[error("Couldn't start audio capture: {0}")]
    InitFailed(String),
}

impl CaptureError {
    pub fn code(&self) -> &'static str {
        match self {
            CaptureError::PermissionDenied => "mic_permission_denied",
            CaptureError::DeviceNotFound(_) => "device_not_found",
            CaptureError::InitFailed(_) => "audio_init_failed",
        }
    }

    pub fn user_message(&self, source: Source) -> String {
        match (self, source) {
            (CaptureError::PermissionDenied, _) => MIC_BLOCKED_MESSAGE.to_string(),
            (CaptureError::DeviceNotFound(_), Source::Me) => "No microphone was found.".to_string(),
            (CaptureError::DeviceNotFound(_), Source::Them) => {
                "No speaker or headphone output was found.".to_string()
            }
            (CaptureError::InitFailed(_), Source::Me) => "Couldn't open the microphone.".to_string(),
            (CaptureError::InitFailed(_), Source::Them) => {
                "Couldn't capture system audio.".to_string()
            }
        }
    }

    pub fn to_app_error(&self, source: Source) -> AppError {
        let e = AppError::new(self.code(), self.user_message(source));
        match self {
            CaptureError::DeviceNotFound(d) | CaptureError::InitFailed(d) => e.with_detail(d.clone()),
            CaptureError::PermissionDenied => e,
        }
    }
}

#[cfg(target_os = "macos")]
const MIC_BLOCKED_MESSAGE: &str =
    "Microphone access is blocked. Turn on Pluely X in System Settings > Privacy & Security > Microphone.";
#[cfg(not(target_os = "macos"))]
const MIC_BLOCKED_MESSAGE: &str =
    "Microphone access is blocked. Turn it on in Windows Settings > Privacy > Microphone.";

/// Counters shared by a source's capture thread, its Deepgram task and the supervisor.
#[derive(Debug, Default)]
pub struct SourceStats {
    pub sent_ms: AtomicU64,
    pub dropped_ms: AtomicU64,
    pub reconnects: AtomicU64,
    pub discontinuities: AtomicU64,
}

impl SourceStats {
    pub fn add(counter: &AtomicU64, v: u64) {
        counter.fetch_add(v, Ordering::Relaxed);
    }

    pub fn get(counter: &AtomicU64) -> u64 {
        counter.load(Ordering::Relaxed)
    }
}

/// Everything a capture needs. Frames go out through `frames` with `try_send` (never block).
pub struct CaptureRequest {
    pub source: Source,
    pub device: DeviceSelector,
    pub aec: AecMode,
    pub clock: SessionClock,
    pub frames: tokio::sync::mpsc::Sender<AudioFrame>,
    pub level: Arc<LevelMeter>,
    pub stats: Arc<SourceStats>,
    /// Called once when a successfully started capture ends (not called if `start` fails).
    pub on_exit: Box<dyn FnOnce(CaptureExit) + Send>,
}

/// A started capture. Dropping it (or calling [`RunningCapture::stop`]) asks the capture
/// thread to stop; it notices within ~100 ms and then calls `on_exit(Stopped)`.
pub struct RunningCapture {
    pub device: OpenedDevice,
    stop: Arc<AtomicBool>,
}

impl RunningCapture {
    pub fn new(device: OpenedDevice, stop: Arc<AtomicBool>) -> Self {
        Self { device, stop }
    }

    pub fn stop(&self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl Drop for RunningCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Starts captures. The real one is [`win::capture::WasapiBackend`]; tests use fakes.
pub trait CaptureBackend: Send + Sync + 'static {
    /// Opens the device and starts capturing on its own thread. May block for a few
    /// seconds while the driver initialises, so call it from `spawn_blocking`.
    fn start(&self, req: CaptureRequest) -> Result<RunningCapture, CaptureError>;
}

/// Data-flow direction of an endpoint (mirrors `EDataFlow`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Render,
    Capture,
}

/// Default-device role (mirrors `ERole`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceRole {
    Console,
    Multimedia,
    Communications,
}

/// Endpoint notifications, forwarded from `IMMNotificationClient`.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceEvent {
    DefaultChanged {
        flow: Flow,
        role: DeviceRole,
        id: Option<String>,
    },
    Added(String),
    Removed(String),
    StateChanged {
        id: String,
        active: bool,
    },
}

/// Which default a source follows.
pub fn default_role_for(source: Source) -> (Flow, DeviceRole) {
    match source {
        Source::Me => (Flow::Capture, DeviceRole::Communications),
        Source::Them => (Flow::Render, DeviceRole::Console),
    }
}

/// Something that can deliver [`DeviceEvent`]s (the `audio-ctl` thread on Windows).
pub trait DeviceWatcher: Send + Sync + 'static {
    fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<DeviceEvent>;
}

/// One endpoint for `audio_list_devices`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Device {
    pub id: String,
    pub name: String,
    pub is_default: bool,
    pub is_default_comms: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DeviceLists {
    pub inputs: Vec<Device>,
    pub outputs: Vec<Device>,
}

/// The platform's capture backend.
pub fn default_backend() -> Arc<dyn CaptureBackend> {
    #[cfg(windows)]
    {
        Arc::new(win::capture::WasapiBackend)
    }
    #[cfg(target_os = "macos")]
    {
        Arc::new(mac::capture::MacBackend)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Arc::new(UnsupportedBackend)
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
struct UnsupportedBackend;

#[cfg(not(any(windows, target_os = "macos")))]
impl CaptureBackend for UnsupportedBackend {
    fn start(&self, _req: CaptureRequest) -> Result<RunningCapture, CaptureError> {
        Err(CaptureError::InitFailed(
            "audio capture is only implemented on Windows and macOS".into(),
        ))
    }
}

/// Device listing + notifications. On Windows this is the long-lived `audio-ctl` MTA thread;
/// on macOS a thread that polls Core Audio.
#[cfg(windows)]
pub use win::control::AudioControl;
#[cfg(target_os = "macos")]
pub use mac::control::AudioControl;

#[cfg(not(any(windows, target_os = "macos")))]
pub struct AudioControl;

#[cfg(not(any(windows, target_os = "macos")))]
impl AudioControl {
    pub fn start() -> Self {
        AudioControl
    }

    pub async fn list(&self) -> Result<DeviceLists, String> {
        Ok(DeviceLists::default())
    }
}

#[cfg(not(any(windows, target_os = "macos")))]
impl DeviceWatcher for AudioControl {
    fn subscribe(&self) -> tokio::sync::mpsc::UnboundedReceiver<DeviceEvent> {
        tokio::sync::mpsc::unbounded_channel().1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selector_from_option() {
        assert_eq!(DeviceSelector::from_option(None), DeviceSelector::Default);
        assert_eq!(DeviceSelector::from_option(Some("")), DeviceSelector::Default);
        assert_eq!(DeviceSelector::from_option(Some("default")), DeviceSelector::Default);
        assert_eq!(
            DeviceSelector::from_option(Some("{0.0.1.00000000}.{abc}")),
            DeviceSelector::Id("{0.0.1.00000000}.{abc}".into())
        );
    }

    #[test]
    fn source_serde() {
        assert_eq!(serde_json::to_string(&Source::Me).unwrap(), "\"me\"");
        assert_eq!(serde_json::from_str::<Source>("\"them\"").unwrap(), Source::Them);
    }

    #[test]
    fn capture_error_codes() {
        assert_eq!(CaptureError::PermissionDenied.code(), "mic_permission_denied");
        assert_eq!(CaptureError::DeviceNotFound("x".into()).code(), "device_not_found");
        assert_eq!(CaptureError::InitFailed("x".into()).code(), "audio_init_failed");
        let e = CaptureError::InitFailed("0x88890008".into()).to_app_error(Source::Them);
        assert_eq!(e.code, "audio_init_failed");
        assert_eq!(e.detail.as_deref(), Some("0x88890008"));
    }
}
