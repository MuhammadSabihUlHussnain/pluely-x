//! Per-source WASAPI capture threads (`cap-me`, `cap-them`).
//!
//! Init order (docs/NATIVE_DESIGN.md "Mic init order"):
//! resolve device → `get_iaudioclient` → (mic, AEC not off) Communications category →
//! `initialize_client` (16 kHz mono f32, autoconvert, 200 ms, event-driven) → (mic) AEC with
//! `set_echo_cancellation_render_endpoint(None)` → event handle → capture client →
//! session-disconnect notification → `start_stream`.
//!
//! Drain loop: wait ≤100 ms (a timeout is normal during silence, never an exit), then read
//! *every* queued packet with `read_from_device` into a preallocated buffer, honour SILENT,
//! stamp with QPC on the session clock, and hand off to the [`CapturePump`].
//!
//! wasapi 0.19 workarounds: never `read_from_device_to_deque` (unwraps ReleaseBuffer), pass
//! `None` as the AEC endpoint (use-after-free with `Some`), close the event handle ourselves.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use wasapi::{
    AudioCaptureClient, AudioClient, Direction, EventCallbacks, EventRegistration, Handle,
    SampleType, StreamMode, WaveFormat,
};
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::Media::Audio::AudioCategory_Communications;

use super::com::ComGuard;
use super::{describe, devices, hr, hresult_of};
use crate::audio::frame::{le_bytes_to_f32, SAMPLE_RATE};
use crate::audio::pump::CapturePump;
use crate::audio::resample::{decode_interleaved, Resampler, SampleFormat};
use crate::audio::timeline::SessionClock;
use crate::audio::{
    AecMode, AecState, CaptureBackend, CaptureError, CaptureExit, CaptureRequest, OpenedDevice,
    RunningCapture, Source,
};

/// 200 ms shared-mode buffer (event mode still wakes every ~10 ms).
const BUFFER_HNS: i64 = 2_000_000;
/// Event wait; a timeout just means "no packet yet".
const WAIT_MS: u32 = 100;
/// How long `start` waits for the thread to open the device.
const INIT_TIMEOUT: Duration = Duration::from_secs(5);

/// The real backend.
pub struct WasapiBackend;

impl CaptureBackend for WasapiBackend {
    fn start(&self, req: CaptureRequest) -> Result<RunningCapture, CaptureError> {
        let stop = Arc::new(AtomicBool::new(false));
        let (init_tx, init_rx) = mpsc::sync_channel::<Result<OpenedDevice, CaptureError>>(1);
        let thread_stop = stop.clone();
        let name = match req.source {
            Source::Me => "cap-me",
            Source::Them => "cap-them",
        };
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || capture_thread(req, init_tx, thread_stop))
            .map_err(|e| CaptureError::InitFailed(format!("spawn capture thread: {e}")))?;
        match init_rx.recv_timeout(INIT_TIMEOUT) {
            Ok(Ok(device)) => Ok(RunningCapture::new(device, stop)),
            Ok(Err(e)) => Err(e),
            Err(_) => {
                stop.store(true, Ordering::SeqCst);
                Err(CaptureError::InitFailed(
                    "audio driver didn't respond within 5 s".into(),
                ))
            }
        }
    }
}

/// Owns the event handle wasapi creates but never closes.
struct EventHandle(Handle);

impl EventHandle {
    fn wait(&self, ms: u32) -> bool {
        self.0.wait_for_event(ms).is_ok()
    }

    fn raw(&self) -> HANDLE {
        // `wasapi::Handle` is a single-field struct around a HANDLE with no accessor. The
        // size assertion below guards the layout assumption at compile time.
        const _: () = assert!(std::mem::size_of::<Handle>() == std::mem::size_of::<HANDLE>());
        // SAFETY: same size, single field; reading the pointer value doesn't move it.
        unsafe { *(&self.0 as *const Handle as *const HANDLE) }
    }
}

impl Drop for EventHandle {
    fn drop(&mut self) {
        let h = self.raw();
        if !h.is_invalid() && !h.0.is_null() {
            // SAFETY: the handle came from CreateEventA in set_get_eventhandle and is closed
            // exactly once, after the audio client that signalled it has been released.
            let _ = unsafe { CloseHandle(h) };
        }
    }
}

enum Convert {
    /// The engine delivers 16 kHz mono f32.
    Native,
    /// Mix format fallback: decode + downmix + resample.
    Resample {
        format: SampleFormat,
        resampler: Resampler,
    },
}

/// Declared so that drop order is: session registration, capture client, audio client,
/// then the event handle (after nothing can signal it any more).
struct Stream {
    _session: Option<EventRegistration>,
    capture: AudioCaptureClient,
    client: AudioClient,
    event: EventHandle,
    disconnected: Arc<Mutex<Option<String>>>,
    convert: Convert,
    block_align: usize,
    buf: Vec<u8>,
    rate: u32,
}

impl Drop for Stream {
    fn drop(&mut self) {
        let _ = self.client.stop_stream();
    }
}

fn is_hr(e: &wasapi::WasapiError, code: windows::core::HRESULT) -> bool {
    hresult_of(e) == Some(code)
}

fn map_init_err(e: &wasapi::WasapiError) -> CaptureError {
    match hresult_of(e) {
        Some(h) if h == hr::E_ACCESSDENIED => CaptureError::PermissionDenied,
        Some(h) if h == hr::AUDCLNT_E_DEVICE_INVALIDATED || h == hr::E_NOTFOUND => {
            CaptureError::DeviceNotFound(describe(e))
        }
        _ => CaptureError::InitFailed(describe(e)),
    }
}

fn khz(rate: u32) -> String {
    if rate % 1000 == 0 {
        format!("{} kHz", rate / 1000)
    } else {
        format!("{:.1} kHz", rate as f64 / 1000.0)
    }
}

fn open_stream(req_source: Source, req: &CaptureRequest) -> Result<(Stream, OpenedDevice), CaptureError> {
    let (device, fallback) = devices::resolve(req_source, &req.device)?;
    let device_id = device.get_id().map_err(|e| map_init_err(&e))?;
    let name = device
        .get_friendlyname()
        .unwrap_or_else(|_| "Unknown device".to_string());

    let want_aec = req_source == Source::Me && req.aec != AecMode::Off;
    let new_client = || -> Result<AudioClient, CaptureError> {
        let client = device.get_iaudioclient().map_err(|e| map_init_err(&e))?;
        if want_aec {
            // Must precede Initialize; enables the system AEC effect where available.
            if let Err(e) = client.set_audio_stream_category(AudioCategory_Communications) {
                tracing::warn!("set Communications category failed: {}", describe(&e));
            }
        }
        Ok(client)
    };

    let mut client = new_client()?;
    let mix = client.get_mixformat().ok();
    let native = mix
        .as_ref()
        .map(|m| format!("{} {} ch", khz(m.get_samplespersec()), m.get_nchannels()))
        .unwrap_or_else(|| "unknown format".into());

    let want = WaveFormat::new(32, 32, &SampleType::Float, SAMPLE_RATE as usize, 1, None);
    let mode = StreamMode::EventsShared {
        autoconvert: true,
        buffer_duration_hns: BUFFER_HNS,
    };
    let (convert, format, block_align, rate) =
        match client.initialize_client(&want, &Direction::Capture, &mode) {
            Ok(()) => (
                Convert::Native,
                format!("{native} → 16 kHz mono"),
                want.get_blockalign() as usize,
                SAMPLE_RATE,
            ),
            Err(e) if is_hr(&e, hr::AUDCLNT_E_UNSUPPORTED_FORMAT) => {
                tracing::warn!("16 kHz autoconvert rejected; using the mix format + resampler");
                client = new_client()?;
                let mix = client.get_mixformat().map_err(|e| map_init_err(&e))?;
                let sample = match (mix.get_subformat(), mix.get_bitspersample()) {
                    (Ok(SampleType::Float), 32) => SampleFormat::F32,
                    (Ok(SampleType::Int), 16) => SampleFormat::I16,
                    (Ok(SampleType::Int), 32) => SampleFormat::I32,
                    other => {
                        return Err(CaptureError::InitFailed(format!(
                            "unsupported mix format {other:?}"
                        )))
                    }
                };
                let mode = StreamMode::EventsShared {
                    autoconvert: false,
                    buffer_duration_hns: BUFFER_HNS,
                };
                client
                    .initialize_client(&mix, &Direction::Capture, &mode)
                    .map_err(|e| map_init_err(&e))?;
                let rate = mix.get_samplespersec();
                let channels = mix.get_nchannels() as usize;
                (
                    Convert::Resample {
                        format: sample,
                        resampler: Resampler::new(rate, channels),
                    },
                    format!("{native} → 16 kHz mono (resampled)"),
                    mix.get_blockalign() as usize,
                    rate,
                )
            }
            Err(e) => return Err(map_init_err(&e)),
        };

    let aec = if req_source == Source::Me {
        Some(if !want_aec {
            AecState::Off
        } else {
            match client.is_aec_supported() {
                Ok(true) => match client
                    .get_aec_control()
                    .and_then(|c| c.set_echo_cancellation_render_endpoint(None))
                {
                    Ok(()) => AecState::On,
                    Err(e) => {
                        tracing::warn!("AEC control failed: {}", describe(&e));
                        AecState::Unsupported
                    }
                },
                Ok(false) => AecState::Unsupported,
                Err(e) => {
                    tracing::warn!("AEC query failed: {}", describe(&e));
                    AecState::Unsupported
                }
            }
        })
    } else {
        None
    };

    let event = EventHandle(client.set_get_eventhandle().map_err(|e| map_init_err(&e))?);
    let capture = client
        .get_audiocaptureclient()
        .map_err(|e| map_init_err(&e))?;

    let disconnected = Arc::new(Mutex::new(None::<String>));
    let session = match client.get_audiosessioncontrol() {
        Ok(ctl) => {
            let flag = disconnected.clone();
            let mut cb = EventCallbacks::new();
            cb.set_disconnected_callback(move |reason| {
                if let Ok(mut g) = flag.lock() {
                    *g = Some(format!("{reason:?}"));
                }
            });
            ctl.register_session_notification(cb)
                .map_err(|e| tracing::warn!("session notifications: {}", describe(&e)))
                .ok()
        }
        Err(e) => {
            tracing::warn!("no session control: {}", describe(&e));
            None
        }
    };

    let buffer_frames = client.get_buffer_size().map_err(|e| map_init_err(&e))? as usize;
    let buf = vec![0u8; buffer_frames.max(1) * block_align.max(1)];

    client.start_stream().map_err(|e| map_init_err(&e))?;

    let info = OpenedDevice {
        device_id,
        name,
        format,
        aec,
        fallback,
    };
    Ok((
        Stream {
            _session: session,
            capture,
            client,
            event,
            disconnected,
            convert,
            block_align,
            buf,
            rate,
        },
        info,
    ))
}

fn loop_exit(e: &wasapi::WasapiError) -> CaptureExit {
    match hresult_of(e) {
        Some(h) if h == hr::E_ACCESSDENIED => CaptureExit::PermissionDenied,
        _ => CaptureExit::DeviceLost(describe(e)),
    }
}

fn drain_loop(
    stream: &mut Stream,
    clock: SessionClock,
    pump: &mut CapturePump,
    stop: &AtomicBool,
) -> CaptureExit {
    let mut samples: Vec<f32> = Vec::with_capacity(4096);
    let mut decoded: Vec<f32> = Vec::with_capacity(4096);
    loop {
        if stop.load(Ordering::SeqCst) {
            return CaptureExit::Stopped;
        }
        if let Some(reason) = stream.disconnected.lock().ok().and_then(|g| g.clone()) {
            return CaptureExit::DeviceLost(format!("session disconnected: {reason}"));
        }
        // Timeout is normal during silence (loopback delivers nothing while nothing plays).
        let _signalled = stream.event.wait(WAIT_MS);
        loop {
            let n = match stream.capture.get_next_packet_size() {
                Ok(n) => n.unwrap_or(0),
                Err(e) => return loop_exit(&e),
            };
            if n == 0 {
                break;
            }
            let (frames, info) = match stream.capture.read_from_device(&mut stream.buf) {
                Ok(x) => x,
                Err(e) => return loop_exit(&e),
            };
            if frames == 0 {
                break;
            }
            let bytes = &stream.buf[..frames as usize * stream.block_align];
            samples.clear();
            match &mut stream.convert {
                Convert::Native => le_bytes_to_f32(bytes, &mut samples),
                Convert::Resample { format, resampler } => {
                    decoded.clear();
                    decode_interleaved(bytes, *format, &mut decoded);
                    resampler.process(&decoded, &mut samples);
                }
            }
            let dur_ms = frames as i64 * 1000 / stream.rate.max(1) as i64;
            let t_ms = if info.timestamp != 0 && !info.flags.timestamp_error {
                clock.ts_to_ms(info.timestamp)
            } else {
                clock.now_ms() - dur_ms
            };
            pump.on_packet(
                &samples,
                t_ms,
                info.flags.silent,
                info.flags.data_discontinuity,
            );
        }
        pump.on_idle(clock.now_ms());
    }
}

fn capture_thread(
    req: CaptureRequest,
    init_tx: mpsc::SyncSender<Result<OpenedDevice, CaptureError>>,
    stop: Arc<AtomicBool>,
) {
    let _com = match ComGuard::new() {
        Ok(g) => g,
        Err(e) => {
            let _ = init_tx.send(Err(CaptureError::InitFailed(e)));
            return;
        }
    };
    let source = req.source;
    let (mut stream, info) = match open_stream(source, &req) {
        Ok(x) => x,
        Err(e) => {
            tracing::warn!(source = %source, code = e.code(), "capture open failed: {e}");
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    tracing::info!(source = %source, device = %info.name, format = %info.format, aec = ?info.aec, "capture started");
    if init_tx.send(Ok(info)).is_err() {
        // `start` already gave up (timeout); don't run an orphan stream.
        return;
    }
    let CaptureRequest {
        clock,
        frames,
        level,
        stats,
        on_exit,
        ..
    } = req;
    let mut pump = CapturePump::new(frames, level, stats);
    let exit = drain_loop(&mut stream, clock, &mut pump, &stop);
    drop(stream); // stop + release COM objects, then close the event handle
    tracing::info!(source = %source, ?exit, frames = pump.frames_sent(), "capture ended");
    on_exit(exit);
}

#[cfg(test)]
mod hw_tests {
    //! Hardware tests: they open real audio devices. Run with
    //! `cargo test hw_ -- --ignored --nocapture --test-threads=1`.
    use super::*;
    use crate::audio::level::LevelMeter;
    use crate::audio::{AudioFrame, DeviceSelector, SourceStats};
    use std::time::Instant;
    use tokio::sync::mpsc as tmpsc;

    struct Started {
        cap: RunningCapture,
        rx: tmpsc::Receiver<AudioFrame>,
        exit_rx: mpsc::Receiver<CaptureExit>,
        stats: Arc<SourceStats>,
    }

    fn start(source: Source, aec: AecMode) -> Started {
        let (tx, rx) = tmpsc::channel(1024);
        let (exit_tx, exit_rx) = mpsc::channel();
        let stats = Arc::new(SourceStats::default());
        let req = CaptureRequest {
            source,
            device: DeviceSelector::Default,
            aec,
            clock: SessionClock::start(),
            frames: tx,
            level: Arc::new(LevelMeter::new()),
            stats: stats.clone(),
            on_exit: Box::new(move |e| {
                let _ = exit_tx.send(e);
            }),
        };
        let cap = WasapiBackend.start(req).expect("capture should start");
        Started {
            cap,
            rx,
            exit_rx,
            stats,
        }
    }

    fn drain(rx: &mut tmpsc::Receiver<AudioFrame>) -> Vec<AudioFrame> {
        let mut v = Vec::new();
        while let Ok(f) = rx.try_recv() {
            v.push(f);
        }
        v
    }

    fn peak(frames: &[AudioFrame]) -> f32 {
        frames
            .iter()
            .filter(|f| !f.synthetic)
            .map(|f| crate::audio::level::rms_peak_pcm(&f.pcm).1)
            .fold(0.0, f32::max)
    }

    /// Plays a quiet, short sine on the default render device through its own wasapi
    /// render stream (on this thread, MTA).
    fn play_tone(ms: u64, amplitude: f32) {
        let _com = ComGuard::new().unwrap();
        let dev = wasapi::get_default_device(&Direction::Render).unwrap();
        let mut client = dev.get_iaudioclient().unwrap();
        let fmt = WaveFormat::new(32, 32, &SampleType::Float, 48_000, 2, None);
        client
            .initialize_client(
                &fmt,
                &Direction::Render,
                &StreamMode::EventsShared {
                    autoconvert: true,
                    buffer_duration_hns: 1_000_000,
                },
            )
            .unwrap();
        let ev = EventHandle(client.set_get_eventhandle().unwrap());
        let render = client.get_audiorenderclient().unwrap();
        let total = (48_000 * ms / 1000) as usize;
        let mut written = 0usize;
        client.start_stream().unwrap();
        while written < total {
            if !ev.wait(200) {
                continue;
            }
            let space = client.get_available_space_in_frames().unwrap() as usize;
            let n = space.min(total - written);
            if n == 0 {
                continue;
            }
            let mut data = Vec::with_capacity(n * 8);
            for i in 0..n {
                let t = (written + i) as f32 / 48_000.0;
                // 10 ms fade in/out to avoid clicks.
                let fade = ((written + i).min(total - written - i) as f32 / 480.0).min(1.0);
                let s = amplitude * fade * (2.0 * std::f32::consts::PI * 660.0 * t).sin();
                data.extend_from_slice(&s.to_le_bytes());
                data.extend_from_slice(&s.to_le_bytes());
            }
            render.write_to_device(n, &data, None).unwrap();
            written += n;
        }
        std::thread::sleep(Duration::from_millis(150));
        client.stop_stream().unwrap();
    }

    #[test]
    #[ignore = "hardware: opens the default output's loopback and plays two short quiet tones"]
    fn hw_loopback_survives_15s_silence() {
        let mut s = start(Source::Them, AecMode::Off);
        println!("loopback device: {:?}", s.cap.device);
        std::thread::sleep(Duration::from_millis(300));
        drain(&mut s.rx);

        play_tone(400, 0.03);
        std::thread::sleep(Duration::from_millis(300));
        let first = drain(&mut s.rx);
        let p1 = peak(&first);
        println!("tone 1: {} frames, peak {:.4}", first.len(), p1);
        assert!(!first.is_empty(), "loopback produced frames while a tone played");

        // 15 s with nothing played by us. The capture thread must not exit.
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(15) {
            assert!(s.exit_rx.try_recv().is_err(), "capture exited during silence");
            std::thread::sleep(Duration::from_millis(250));
        }
        let during = drain(&mut s.rx);
        let synthetic = during.iter().filter(|f| f.synthetic).count();
        println!(
            "silence: {} frames ({} synthetic tail-fill)",
            during.len(),
            synthetic
        );

        play_tone(400, 0.03);
        std::thread::sleep(Duration::from_millis(300));
        let second = drain(&mut s.rx);
        let p2 = peak(&second);
        println!("tone 2: {} frames, peak {:.4}", second.len(), p2);
        assert!(!second.is_empty(), "loopback still delivers after 15 s of silence");
        assert!(p2 > 0.005, "the second tone was captured (peak {p2})");
        let ts: Vec<i64> = second.iter().map(|f| f.t_ms).collect();
        assert!(ts.windows(2).all(|w| w[1] >= w[0]), "monotonic timestamps");
        assert!(*ts.last().unwrap() > 15_000, "timestamps are on the session clock");

        s.cap.stop();
        let exit = s.exit_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(exit, CaptureExit::Stopped);
        println!(
            "dropped_ms={} discontinuities={}",
            SourceStats::get(&s.stats.dropped_ms),
            SourceStats::get(&s.stats.discontinuities)
        );
    }

    #[test]
    #[ignore = "hardware: opens the default communications microphone for ~1.5 s"]
    fn hw_mic_opens_with_aec_query() {
        let mut s = start(Source::Me, AecMode::Auto);
        println!("mic device: {:?}", s.cap.device);
        assert!(s.cap.device.aec.is_some(), "mic reports an AEC state");
        std::thread::sleep(Duration::from_millis(1500));
        let frames = drain(&mut s.rx);
        println!(
            "mic: {} frames in 1.5 s, real={}",
            frames.len(),
            frames.iter().filter(|f| !f.synthetic).count()
        );
        assert!(frames.len() >= 20, "mic delivers ~20 frames/s even when quiet");
        s.cap.stop();
        assert_eq!(
            s.exit_rx.recv_timeout(Duration::from_secs(2)).unwrap(),
            CaptureExit::Stopped
        );

        // AEC off: no Communications category, reports "off".
        let s2 = start(Source::Me, AecMode::Off);
        assert_eq!(s2.cap.device.aec, Some(AecState::Off));
        s2.cap.stop();
    }

    #[test]
    #[ignore = "hardware: lists audio endpoints via the audio-ctl thread"]
    fn hw_list_devices() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let ctl = super::super::control::AudioControl::start();
        let lists = rt.block_on(ctl.list()).unwrap();
        println!("inputs:");
        for d in &lists.inputs {
            println!("  {} default={} comms={}", d.name, d.is_default, d.is_default_comms);
        }
        println!("outputs:");
        for d in &lists.outputs {
            println!("  {} default={} comms={}", d.name, d.is_default, d.is_default_comms);
        }
        assert!(lists.outputs.iter().filter(|d| d.is_default).count() <= 1);
        drop(ctl);
    }
}
