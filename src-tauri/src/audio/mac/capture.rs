//! macOS captures. Each runs on its own thread, which owns the (non-`Send`) Core Audio
//! objects: the audio callback copies samples into a bounded channel, and the thread turns
//! them into 16 kHz mono frames through the shared [`CapturePump`].

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender};
use std::sync::Arc;
use std::time::{Duration, Instant};

use cidre::core_audio::aggregate_device_keys as agg_keys;
use cidre::{arc, av, cat, cf, core_audio as ca, ns, os};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use super::devices;
use crate::audio::pump::CapturePump;
use crate::audio::resample::Resampler;
use crate::audio::timeline::SessionClock;
use crate::audio::{
    AecState, CaptureBackend, CaptureError, CaptureExit, CaptureRequest, DeviceSelector,
    OpenedDevice, RunningCapture, Source, SourceStats,
};

/// How long `start` waits for the thread to open the device.
const INIT_TIMEOUT: Duration = Duration::from_secs(8);
/// Wait between checks of the stop flag when no audio arrives.
const WAIT: Duration = Duration::from_millis(100);
/// Callbacks normally arrive every ~10 ms, silence included. None for this long means the
/// device went away (unplugged, output switched): report it so the supervisor reopens.
const NO_CALLBACKS_LOST: Duration = Duration::from_secs(4);
/// Audio packets buffered between the callback and the capture thread (~2.5 s).
const PACKET_QUEUE: usize = 256;
/// Packet timestamps drifting further than this from the running sample count resync.
const RESYNC_MS: i64 = 250;

/// One callback's worth of interleaved f32 samples, stamped on the session clock when it
/// arrived.
struct Packet {
    samples: Vec<f32>,
    arrived_ms: i64,
}

/// The real macOS backend.
pub struct MacBackend;

impl CaptureBackend for MacBackend {
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
                Err(CaptureError::InitFailed("Core Audio didn't respond within 8 s".into()))
            }
        }
    }
}

/// An open stream: its packets, format and the objects that must stay alive.
struct Opened {
    info: OpenedDevice,
    packets: Receiver<Packet>,
    rate: u32,
    channels: usize,
    /// Set by the stream's error callback.
    failed: Arc<AtomicBool>,
    /// Keeps the Core Audio objects alive; dropping it stops the stream.
    _keep: Box<dyn std::any::Any>,
}

fn capture_thread(
    req: CaptureRequest,
    init_tx: SyncSender<Result<OpenedDevice, CaptureError>>,
    stop: Arc<AtomicBool>,
) {
    let source = req.source;
    let opened = match source {
        Source::Me => open_mic(&req.device, req.clock, req.stats.clone()),
        Source::Them => open_tap(&req.device, req.clock, req.stats.clone()),
    };
    let opened = match opened {
        Ok(o) => o,
        Err(e) => {
            tracing::warn!(source = %source, code = e.code(), "capture open failed: {e}");
            let _ = init_tx.send(Err(e));
            return;
        }
    };
    let info = opened.info.clone();
    tracing::info!(source = %source, device = %info.name, format = %info.format, aec = ?info.aec, "capture started");
    if init_tx.send(Ok(info)).is_err() {
        return; // `start` already gave up: don't run an orphan stream
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
    let exit = drain_loop(&opened, clock, &mut pump, &stop);
    drop(opened); // stops the stream
    tracing::info!(source = %source, ?exit, frames = pump.frames_sent(), "capture ended");
    on_exit(exit);
}

fn drain_loop(o: &Opened, clock: SessionClock, pump: &mut CapturePump, stop: &AtomicBool) -> CaptureExit {
    let mut rs = Resampler::new(o.rate, o.channels.max(1));
    let mut out = Vec::with_capacity(4096);
    // Session time of the next input frame, from the running count (resynced on drift).
    let mut next_ms: Option<f64> = None;
    let ms_per_frame = 1000.0 / o.rate.max(1) as f64;
    let mut last_packet = Instant::now();
    let mut got_any = false;
    loop {
        if stop.load(Ordering::SeqCst) {
            return CaptureExit::Stopped;
        }
        if o.failed.load(Ordering::SeqCst) {
            return CaptureExit::DeviceLost("the audio device stopped".into());
        }
        match o.packets.recv_timeout(WAIT) {
            Ok(p) => {
                last_packet = Instant::now();
                got_any = true;
                let frames_in = p.samples.len() / o.channels.max(1);
                let dur = frames_in as f64 * ms_per_frame;
                let arrived_start = p.arrived_ms as f64 - dur;
                let mut discontinuity = false;
                let t = match next_ms {
                    Some(t) if (t - arrived_start).abs() <= RESYNC_MS as f64 => t,
                    Some(_) => {
                        discontinuity = true;
                        arrived_start
                    }
                    None => arrived_start,
                };
                next_ms = Some(t + dur);
                out.clear();
                rs.process(&p.samples, &mut out);
                pump.on_packet(&out, t.round() as i64, false, discontinuity);
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => {
                return CaptureExit::DeviceLost("the audio stream closed".into());
            }
        }
        if got_any && last_packet.elapsed() >= NO_CALLBACKS_LOST {
            return CaptureExit::DeviceLost("no audio from the device for 4 s".into());
        }
        pump.on_idle(clock.now_ms());
    }
}

/// Hands a callback's samples to the capture thread without blocking the audio thread.
fn deliver(tx: &SyncSender<Packet>, clock: &SessionClock, dropped: &AtomicU64, samples: Vec<f32>) {
    let packet = Packet {
        samples,
        arrived_ms: clock.now_ms(),
    };
    if tx.try_send(packet).is_err() {
        dropped.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------------------
// Microphone (cpal / CoreAudio)
// ---------------------------------------------------------------------------------------

/// Microphone permission as macOS reports it. `NotDetermined` is fine: macOS asks the user
/// the first time the stream starts.
fn mic_permission_denied() -> bool {
    match av::CaptureDevice::authorization_status_for_media_type(av::MediaType::audio()) {
        Ok(av::AuthorizationStatus::Denied) | Ok(av::AuthorizationStatus::Restricted) => true,
        _ => false,
    }
}

fn open_mic(sel: &DeviceSelector, clock: SessionClock, stats: Arc<SourceStats>) -> Result<Opened, CaptureError> {
    if mic_permission_denied() {
        return Err(CaptureError::PermissionDenied);
    }
    let host = cpal::default_host();
    let mut fallback = false;
    let wanted = match sel {
        DeviceSelector::Id(uid) => devices::name_for_uid(uid),
        DeviceSelector::Default => None,
    };
    let device = match &wanted {
        Some(name) => {
            let found = host
                .input_devices()
                .ok()
                .and_then(|mut it| it.find(|d| d.name().ok().as_deref() == Some(name.as_str())));
            match found {
                Some(d) => Some(d),
                None => {
                    fallback = true;
                    host.default_input_device()
                }
            }
        }
        None => {
            fallback = matches!(sel, DeviceSelector::Id(_));
            host.default_input_device()
        }
    }
    .ok_or_else(|| CaptureError::DeviceNotFound("no microphone".into()))?;
    let name = device.name().unwrap_or_else(|_| "Microphone".into());
    let device_id = devices::list_all()
        .ok()
        .and_then(|l| l.inputs.into_iter().find(|d| d.name == name).map(|d| d.id))
        .unwrap_or_else(|| name.clone());
    let supported = device
        .default_input_config()
        .map_err(|e| CaptureError::InitFailed(format!("microphone format: {e}")))?;
    let config: cpal::StreamConfig = supported.config();
    let rate = config.sample_rate.0;
    let channels = config.channels as usize;

    let (tx, rx) = mpsc::sync_channel::<Packet>(PACKET_QUEUE);
    let failed = Arc::new(AtomicBool::new(false));
    let dropped = Arc::new(AtomicU64::new(0));
    let err_flag = failed.clone();
    let on_error = move |e: cpal::StreamError| {
        tracing::warn!("microphone stream error: {e}");
        err_flag.store(true, Ordering::SeqCst);
    };
    let stream = {
        use cpal::SampleFormat as F;
        let (tx, clock, d) = (tx.clone(), clock, dropped.clone());
        match supported.sample_format() {
            F::F32 => device.build_input_stream::<f32, _, _>(
                &config,
                move |data, _| deliver(&tx, &clock, &d, data.to_vec()),
                on_error,
                None,
            ),
            F::I16 => device.build_input_stream::<i16, _, _>(
                &config,
                move |data, _| deliver(&tx, &clock, &d, data.iter().map(|&s| s as f32 / 32768.0).collect()),
                on_error,
                None,
            ),
            F::I32 => device.build_input_stream::<i32, _, _>(
                &config,
                move |data, _| deliver(&tx, &clock, &d, data.iter().map(|&s| s as f32 / 2_147_483_648.0).collect()),
                on_error,
                None,
            ),
            other => {
                return Err(CaptureError::InitFailed(format!("unsupported microphone format {other:?}")));
            }
        }
    }
    .map_err(|e| CaptureError::InitFailed(format!("open microphone: {e}")))?;
    stream
        .play()
        .map_err(|e| CaptureError::InitFailed(format!("start microphone: {e}")))?;
    drop(tx);
    let _ = stats; // drops are counted by the pump once frames exist
    Ok(Opened {
        info: OpenedDevice {
            device_id,
            name,
            format: format!("{} kHz {channels} ch → 16 kHz mono", rate as f64 / 1000.0),
            aec: Some(AecState::Unsupported),
            fallback,
        },
        packets: rx,
        rate,
        channels,
        failed,
        _keep: Box::new((stream, dropped)),
    })
}

// ---------------------------------------------------------------------------------------
// System audio (Core Audio process tap, macOS 14.4+)
// ---------------------------------------------------------------------------------------

/// Read by the IO proc on Core Audio's real-time thread.
struct TapCtx {
    tx: SyncSender<Packet>,
    clock: SessionClock,
    dropped: Arc<AtomicU64>,
}

extern "C" fn tap_proc(
    _device: ca::Device,
    _now: &cat::AudioTimeStamp,
    input_data: &cat::AudioBufList<1>,
    _input_time: &cat::AudioTimeStamp,
    _output_data: &mut cat::AudioBufList<1>,
    _output_time: &cat::AudioTimeStamp,
    ctx: Option<&mut TapCtx>,
) -> os::Status {
    let Some(ctx) = ctx else {
        return os::Status::NO_ERR;
    };
    let buf = &input_data.buffers[0];
    let n = buf.data_bytes_size as usize / std::mem::size_of::<f32>();
    if n > 0 && !buf.data.is_null() {
        // SAFETY: the tap's format is 32-bit float (checked when the stream opened); Core
        // Audio guarantees `data_bytes_size` bytes at `data` for the duration of the call.
        let samples = unsafe { std::slice::from_raw_parts(buf.data as *const f32, n) };
        deliver(&ctx.tx, &ctx.clock, &ctx.dropped, samples.to_vec());
    }
    os::Status::NO_ERR
}

/// Owns the tap and the running aggregate device. Field order matters: the device stops
/// before the tap is destroyed, and the context outlives both.
struct TapStream {
    _device: ca::hardware::StartedDevice<ca::AggregateDevice>,
    _tap: ca::TapGuard,
    _ctx: Box<TapCtx>,
}

fn open_tap(sel: &DeviceSelector, clock: SessionClock, _stats: Arc<SourceStats>) -> Result<Opened, CaptureError> {
    // The tap hears every output; the aggregate device still needs a real output device as
    // its clock. Use the chosen one if it exists, else the default.
    let mut fallback = false;
    let output = match sel {
        DeviceSelector::Id(uid) => match devices::output_device(uid) {
            Some(d) => d,
            None => {
                fallback = true;
                ca::System::default_output_device()
                    .map_err(|e| CaptureError::DeviceNotFound(format!("default output: {e:?}")))?
            }
        },
        DeviceSelector::Default => ca::System::default_output_device()
            .map_err(|e| CaptureError::DeviceNotFound(format!("default output: {e:?}")))?,
    };
    let output_uid = output
        .uid()
        .map_err(|e| CaptureError::InitFailed(format!("output device id: {e:?}")))?;
    let output_name = output
        .name()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "Speakers".to_string());

    let tap_desc = ca::TapDesc::with_mono_global_tap_excluding_processes(&ns::Array::new());
    let tap = tap_desc.create_process_tap().map_err(|e| {
        CaptureError::InitFailed(format!(
            "system audio tap: {e:?} (needs macOS 14.4+ and System Audio Recording permission)"
        ))
    })?;
    let asbd = tap
        .asbd()
        .map_err(|e| CaptureError::InitFailed(format!("tap format: {e:?}")))?;
    let rate = asbd.sample_rate.round() as u32;
    let channels = asbd.channels_per_frame.max(1) as usize;
    if asbd.bits_per_channel != 32 {
        return Err(CaptureError::InitFailed(format!(
            "unexpected tap format: {} bits",
            asbd.bits_per_channel
        )));
    }
    let tap_uid = tap
        .uid()
        .map_err(|e| CaptureError::InitFailed(format!("tap id: {e:?}")))?;

    let sub_device = cf::DictionaryOf::with_keys_values(
        &[ca::sub_device_keys::uid()],
        &[output_uid.as_type_ref()],
    );
    let sub_tap = cf::DictionaryOf::with_keys_values(&[ca::sub_device_keys::uid()], &[tap_uid.as_type_ref()]);
    let agg_desc: arc::R<cf::DictionaryOf<cf::String, cf::Type>> = cf::DictionaryOf::with_keys_values(
        &[
            agg_keys::is_private(),
            agg_keys::is_stacked(),
            agg_keys::tap_auto_start(),
            agg_keys::name(),
            agg_keys::main_sub_device(),
            agg_keys::uid(),
            agg_keys::sub_device_list(),
            agg_keys::tap_list(),
        ],
        &[
            cf::Boolean::value_true().as_type_ref(),
            cf::Boolean::value_false(),
            cf::Boolean::value_true(),
            cf::str!(c"Pluely X system audio"),
            &output_uid,
            &cf::Uuid::new().to_cf_string(),
            &cf::ArrayOf::from_slice(&[sub_device.as_ref()]),
            &cf::ArrayOf::from_slice(&[sub_tap.as_ref()]),
        ],
    );

    let (tx, rx) = mpsc::sync_channel::<Packet>(PACKET_QUEUE);
    let dropped = Arc::new(AtomicU64::new(0));
    let mut ctx = Box::new(TapCtx {
        tx,
        clock,
        dropped,
    });
    let agg = ca::AggregateDevice::with_desc(&agg_desc)
        .map_err(|e| CaptureError::InitFailed(format!("aggregate device: {e:?}")))?;
    let proc_id = agg
        .create_io_proc_id(tap_proc, Some(&mut *ctx))
        .map_err(|e| CaptureError::InitFailed(format!("io proc: {e:?}")))?;
    let started = ca::device_start(agg, Some(proc_id))
        .map_err(|e| CaptureError::InitFailed(format!("start system audio: {e:?}")))?;

    Ok(Opened {
        info: OpenedDevice {
            device_id: output_uid.to_string(),
            name: output_name,
            format: format!("{} kHz {channels} ch → 16 kHz mono", rate as f64 / 1000.0),
            aec: None,
            fallback,
        },
        packets: rx,
        rate,
        channels,
        failed: Arc::new(AtomicBool::new(false)),
        _keep: Box::new(TapStream {
            _device: started,
            _tap: tap,
            _ctx: ctx,
        }),
    })
}
