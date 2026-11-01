//! Live session: docs/CONTRACT.md §3 commands + the single-slot [`SessionManager`].

pub mod hub;
pub mod supervisor;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use tauri::ipc::{Channel, InvokeBody, Request};
use tauri::{AppHandle, Emitter, Manager, Runtime, State};
use tokio::sync::Mutex;

use crate::audio::{AudioControl, CaptureBackend, DeviceLists, DeviceWatcher};
use crate::error::{AppError, AppResult};
use crate::secrets::{Provider, Secrets};
use crate::stt::deepgram::client::DgTimings;
use crate::stt::deepgram::config::DEFAULT_BASE_URL;
use hub::EventSink;
use supervisor::{start_session, SessionHandle, SupervisorDeps};
use types::*;

pub const STATUS_EVENT: &str = "session://status";
pub const MIC_PRIVACY_URI: &str = "ms-settings:privacy-microphone";

/// Managed state: at most one live session.
pub struct SessionManager {
    slot: Mutex<Option<SessionHandle>>,
    backend: Arc<dyn CaptureBackend>,
    watcher: Option<Arc<dyn DeviceWatcher>>,
    devices: Option<Arc<AudioControl>>,
    dg_base_url: String,
    timings: DgTimings,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new_default()
    }
}

impl SessionManager {
    /// Production wiring: WASAPI backend + the `audio-ctl` thread for listing/notifications.
    pub fn new_default() -> Self {
        let ctl = Arc::new(AudioControl::start());
        Self::new(
            crate::audio::default_backend(),
            Some(ctl.clone() as Arc<dyn DeviceWatcher>),
            Some(ctl),
        )
    }

    pub fn new(
        backend: Arc<dyn CaptureBackend>,
        watcher: Option<Arc<dyn DeviceWatcher>>,
        devices: Option<Arc<AudioControl>>,
    ) -> Self {
        Self {
            slot: Mutex::new(None),
            backend,
            watcher,
            devices,
            dg_base_url: DEFAULT_BASE_URL.to_string(),
            timings: DgTimings::default(),
        }
    }

    /// Point the Deepgram sockets somewhere else (tests use a local mock).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_dg_base_url(mut self, url: impl Into<String>) -> Self {
        self.dg_base_url = url.into();
        self
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn with_timings(mut self, timings: DgTimings) -> Self {
        self.timings = timings;
        self
    }

    pub async fn list_devices(&self) -> AppResult<DeviceLists> {
        let Some(ctl) = &self.devices else {
            return Ok(DeviceLists::default());
        };
        ctl.list()
            .await
            .map_err(|e| AppError::new("audio_init_failed", "Couldn't list audio devices.").with_detail(e))
    }

    pub async fn start(
        &self,
        opts: SessionOptions,
        api_key: String,
        sink: Box<dyn EventSink>,
        dump_dir: Option<PathBuf>,
    ) -> AppResult<SessionInfo> {
        let mut slot = self.slot.lock().await;
        if slot.is_some() {
            return Err(AppError::new(
                "session_active",
                "A listening session is already running.",
            ));
        }
        let deps = SupervisorDeps {
            backend: self.backend.clone(),
            device_events: self.watcher.as_ref().map(|w| w.subscribe()),
            dg_base_url: self.dg_base_url.clone(),
            api_key,
            timings: self.timings,
            dump_dir,
        };
        let (info, handle) = start_session(opts, deps, sink).await?;
        *slot = Some(handle);
        Ok(info)
    }

    pub async fn stop(&self) -> AppResult<SessionSummary> {
        let handle = self.slot.lock().await.take();
        match handle {
            Some(h) => Ok(h.stop().await),
            None => Err(AppError::new("no_session", "No listening session is running.")),
        }
    }

    pub async fn set_source(&self, args: SetSourceArgs) -> AppResult<()> {
        let slot = self.slot.lock().await;
        match slot.as_ref() {
            Some(h) => h.set_source(args).await,
            None => Err(AppError::new("no_session", "No listening session is running.")),
        }
    }

    pub async fn status(&self) -> SessionStatus {
        self.slot
            .lock()
            .await
            .as_ref()
            .map(|h| h.status())
            .unwrap_or_default()
    }

    /// Route events to a new channel; it gets a `snapshot` first.
    pub async fn reattach(&self, sink: Box<dyn EventSink>) {
        let slot = self.slot.lock().await;
        match slot.as_ref() {
            Some(h) => h.hub.reattach(sink),
            None => {
                sink.send(&SessionEvent::Snapshot {
                    status: SessionStatus::default(),
                    recent_finals: Vec::new(),
                });
            }
        }
    }
}

fn broadcast_status<R: Runtime>(app: &AppHandle<R>, active: bool, session_id: Option<String>) {
    let _ = app.emit(STATUS_EVENT, StatusBroadcast { active, session_id });
}

fn dump_dir<R: Runtime>(app: &AppHandle<R>) -> Option<PathBuf> {
    if !crate::audio::dump::enabled() {
        return None;
    }
    app.path().app_log_dir().ok()
}

// ---- commands (docs/CONTRACT.md §3) -------------------------------------------------------

#[tauri::command]
pub async fn audio_list_devices(mgr: State<'_, SessionManager>) -> AppResult<DeviceLists> {
    mgr.list_devices().await
}

#[tauri::command]
pub async fn audio_open_mic_privacy_settings() -> AppResult<()> {
    tauri_plugin_opener::open_url(MIC_PRIVACY_URI, None::<&str>).map_err(|e| {
        AppError::new("open_failed", "Couldn't open Windows privacy settings.").with_detail(e.to_string())
    })
}

#[tauri::command]
pub async fn session_start<R: Runtime>(
    app: AppHandle<R>,
    mgr: State<'_, SessionManager>,
    secrets: State<'_, Secrets>,
    opts: SessionOptions,
    on_event: Channel<SessionEvent>,
) -> AppResult<SessionInfo> {
    let key = secrets
        .get(Provider::Deepgram)?
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| {
            AppError::new("key_missing", "Add a Deepgram key in Settings to start listening.")
        })?;
    let info = mgr
        .start(opts, key, Box::new(on_event), dump_dir(&app))
        .await?;
    broadcast_status(&app, true, Some(info.session_id.clone()));
    Ok(info)
}

#[tauri::command]
pub async fn session_stop<R: Runtime>(
    app: AppHandle<R>,
    mgr: State<'_, SessionManager>,
) -> AppResult<SessionSummary> {
    let summary = mgr.stop().await?;
    broadcast_status(&app, false, None);
    Ok(summary)
}

/// Args are read from the raw payload so `deviceId: null` (follow the default) can be told
/// apart from an absent `deviceId` (unchanged).
#[tauri::command]
pub async fn session_set_source(
    mgr: State<'_, SessionManager>,
    request: Request<'_>,
) -> AppResult<()> {
    let args: SetSourceArgs = match request.body() {
        InvokeBody::Json(v) => serde_json::from_value(v.clone()).map_err(|e| {
            AppError::new("invalid_argument", "Invalid session_set_source arguments.")
                .with_detail(e.to_string())
        })?,
        InvokeBody::Raw(_) => {
            return Err(AppError::new(
                "invalid_argument",
                "session_set_source expects JSON arguments.",
            ))
        }
    };
    mgr.set_source(args).await
}

#[tauri::command]
pub async fn session_status(mgr: State<'_, SessionManager>) -> AppResult<SessionStatus> {
    Ok(mgr.status().await)
}

#[tauri::command]
pub async fn session_reattach(
    mgr: State<'_, SessionManager>,
    on_event: Channel<SessionEvent>,
) -> AppResult<()> {
    mgr.reattach(Box::new(on_event)).await;
    Ok(())
}

/// Exit hook: stop the live session (CloseStream flush) before the process goes away.
/// Async session stop for the quit path (awaited on the async runtime; see `crate::quit`).
pub async fn stop_for_quit<R: Runtime>(app: &AppHandle<R>) {
    let Some(mgr) = app.try_state::<SessionManager>() else {
        return;
    };
    if let Ok(Ok(_)) = tokio::time::timeout(Duration::from_secs(4), mgr.stop()).await {
        broadcast_status(app, false, None);
    }
}

/// Blocking best-effort stop for `RunEvent::Exit` (main thread, outside the async runtime).
/// Must not be called from inside an async task or command.
pub fn stop_on_exit<R: Runtime>(app: &AppHandle<R>) {
    let Some(mgr) = app.try_state::<SessionManager>() else {
        return;
    };
    let stopped = tauri::async_runtime::block_on(async {
        tokio::time::timeout(Duration::from_secs(4), mgr.stop()).await
    });
    if let Ok(Ok(_)) = stopped {
        broadcast_status(app, false, None);
    }
}
