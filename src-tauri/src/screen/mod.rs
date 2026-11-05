//! Screenshots (contract §5): full-monitor and region capture, downscaled + encoded in Rust,
//! kept in an in-memory store by id, optionally saved under `<appData>/screenshots`.

pub mod encode;
pub mod geometry;
pub mod store;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use base64::Engine;
use image::RgbaImage;
use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, Position, Runtime, Size,
    WebviewUrl, WebviewWindowBuilder,
};

use crate::error::{AppError, AppResult};
use crate::settings::SettingsState;
use encode::Format;
use geometry::{CssRect, MonitorGeom, Rect};
use store::{ScreenshotMeta, ScreenshotStore, Shot};

pub const EVENT_CAPTURED: &str = "screenshot://captured";
pub const EVENT_CANCELLED: &str = "screenshot://cancelled";
pub const OVERLAY_PREFIX: &str = "capture-overlay-";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Target {
    #[default]
    Cursor,
    Overlay,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ImageData {
    pub mime: String,
    pub data_base64: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedPath {
    pub path: String,
}

struct FrozenMonitor {
    image: RgbaImage,
    scale: f64,
}

/// A region capture in progress: every monitor frozen at start, cropped on commit.
#[derive(Default)]
pub struct RegionState {
    session: Mutex<Option<RegionSession>>,
}

struct RegionSession {
    frozen: HashMap<String, FrozenMonitor>,
    #[cfg_attr(not(windows), allow(dead_code))]
    prev_foreground: isize,
}

fn capture_err(e: impl std::fmt::Display) -> AppError {
    AppError::new("capture_failed", "Couldn't capture the screen").with_detail(e.to_string())
}

/// Capture one monitor by its device name (falls back to the monitor at `fallback_point`).
fn capture_monitor(name: &str, fallback_point: (i32, i32)) -> AppResult<RgbaImage> {
    let all = xcap::Monitor::all().map_err(capture_err)?;
    let mon = match all.into_iter().find(|m| m.name() == name) {
        Some(m) => m,
        None => {
            xcap::Monitor::from_point(fallback_point.0, fallback_point.1).map_err(capture_err)?
        }
    };
    mon.capture_image().map_err(capture_err)
}

fn new_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

/// Encode, store and announce a captured image.
async fn finish_capture<R: Runtime>(
    app: &AppHandle<R>,
    img: RgbaImage,
    monitor: String,
    max_edge: u32,
    format: Format,
    quality: u8,
) -> AppResult<ScreenshotMeta> {
    let (src_w, src_h) = img.dimensions();
    let (enc, thumb) = tauri::async_runtime::spawn_blocking(move || {
        let enc = encode::encode(&img, max_edge, format, quality)?;
        let thumb = encode::thumb_data_url(&img)?;
        Ok::<_, String>((enc, thumb))
    })
    .await
    .map_err(AppError::from)?
    .map_err(|e| AppError::new("encode_failed", "Couldn't encode the screenshot").with_detail(e))?;

    let meta = ScreenshotMeta {
        id: new_id(),
        mime: enc.mime.to_string(),
        width: enc.width,
        height: enc.height,
        source_width: src_w,
        source_height: src_h,
        monitor,
        bytes: enc.bytes.len(),
        thumb_data_url: thumb,
    };
    app.state::<ScreenshotStore>().insert(Shot {
        meta: meta.clone(),
        data: Arc::new(enc.bytes),
    });
    tracing::info!(id = %meta.id, mime = %meta.mime, w = meta.width, h = meta.height, bytes = meta.bytes, "screenshot stored");
    let _ = app.emit_to(crate::window::MAIN, EVENT_CAPTURED, &meta);
    Ok(meta)
}

fn pick_monitor<R: Runtime>(
    app: &AppHandle<R>,
    target: Target,
    mons: &[MonitorGeom],
) -> AppResult<usize> {
    let idx = match target {
        Target::Cursor => {
            let p = app.cursor_position()?;
            let (x, y) = (p.x.round() as i32, p.y.round() as i32);
            geometry::monitor_at_point(mons, x, y)
                .or_else(|| geometry::nearest_monitor(mons, (x, y)))
        }
        Target::Overlay => {
            let w = crate::window::main_window(app)?;
            let p = w.outer_position()?;
            let s = w.outer_size()?;
            geometry::monitor_for_rect(mons, &Rect::new(p.x, p.y, s.width as i32, s.height as i32))
        }
    };
    idx.or_else(|| geometry::primary_index(mons))
        .ok_or_else(|| AppError::new("capture_failed", "No monitors found"))
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn screenshot_full(
    app: AppHandle,
    target: Option<Target>,
    max_edge: Option<u32>,
    format: Option<Format>,
    quality: Option<u8>,
) -> AppResult<ScreenshotMeta> {
    let mons = crate::window::monitors(&app)?;
    let idx = pick_monitor(&app, target.unwrap_or_default(), &mons)?;
    let m = mons[idx].clone();

    // With content protection on, the overlay is already invisible to capture. Without it,
    // hide it briefly so it isn't in the shot.
    let protected = app
        .state::<SettingsState>()
        .get()
        .overlay
        .content_protection;
    let main = crate::window::main_window(&app).ok();
    let hide = !protected
        && main
            .as_ref()
            .map(|w| w.is_visible().unwrap_or(false))
            .unwrap_or(false);
    if hide {
        let _ = main.as_ref().map(|w| w.hide());
        tokio::time::sleep(Duration::from_millis(60)).await;
    }
    let name = m.name.clone();
    let center = m.bounds.center();
    let result = tauri::async_runtime::spawn_blocking(move || capture_monitor(&name, center)).await;
    if hide {
        let _ = crate::window::show(&app);
    }
    let img = result.map_err(AppError::from)??;
    finish_capture(
        &app,
        img,
        m.name,
        encode::clamp_max_edge(max_edge),
        format.unwrap_or_default(),
        encode::clamp_quality(quality),
    )
    .await
}

/// Percent-encode a monitor name for the capture overlay URL.
pub fn encode_query_value(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

fn close_overlays<R: Runtime>(app: &AppHandle<R>) {
    for (label, w) in app.webview_windows() {
        if label.starts_with(OVERLAY_PREFIX) {
            let _ = w.destroy();
        }
    }
}

fn end_region<R: Runtime>(app: &AppHandle<R>, session: Option<RegionSession>) {
    close_overlays(app);
    if let Some(s) = session {
        crate::window::restore_foreground(s.prev_foreground);
    }
}

#[tauri::command]
pub async fn screenshot_region_start(app: AppHandle) -> AppResult<()> {
    // replace any capture already in progress
    let old = app
        .state::<RegionState>()
        .session
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    if old.is_some() {
        close_overlays(&app);
    }

    let mons = crate::window::monitors(&app)?;
    if mons.is_empty() {
        return Err(AppError::new("capture_failed", "No monitors found"));
    }
    let prev_foreground = crate::window::foreground_for_restore();

    // freeze every monitor first so the overlays aren't in the picture
    let to_capture: Vec<(String, (i32, i32), f64)> = mons
        .iter()
        .map(|m| (m.name.clone(), m.bounds.center(), m.scale))
        .collect();
    let frozen = tauri::async_runtime::spawn_blocking(move || {
        let mut map = HashMap::new();
        for (name, center, scale) in to_capture {
            let image = capture_monitor(&name, center)?;
            map.insert(name, FrozenMonitor { image, scale });
        }
        Ok::<_, AppError>(map)
    })
    .await
    .map_err(AppError::from)??;

    *app.state::<RegionState>()
        .session
        .lock()
        .unwrap_or_else(|e| e.into_inner()) = Some(RegionSession {
        frozen,
        prev_foreground,
    });

    let protected = app
        .state::<SettingsState>()
        .get()
        .overlay
        .content_protection;
    let cursor_idx = app
        .cursor_position()
        .ok()
        .and_then(|p| geometry::monitor_at_point(&mons, p.x.round() as i32, p.y.round() as i32));
    for (i, m) in mons.iter().enumerate() {
        let url = format!("capture?monitor={}", encode_query_value(&m.name));
        let w = WebviewWindowBuilder::new(
            &app,
            format!("{OVERLAY_PREFIX}{i}"),
            WebviewUrl::App(url.into()),
        )
        .title("Pluely X capture")
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .content_protected(protected)
        .visible(false)
        .build();
        let w = match w {
            Ok(w) => w,
            Err(e) => {
                let s = app
                    .state::<RegionState>()
                    .session
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .take();
                end_region(&app, s);
                return Err(AppError::from(e));
            }
        };
        #[cfg(windows)]
        if let Ok(h) = w.hwnd() {
            // keep it out of Alt+Tab; the subclass must be installed on the UI thread
            let raw = crate::window::win32::hwnd_to_isize(h);
            let _ = app.run_on_main_thread(move || {
                crate::window::win32::install_tool_subclass(crate::window::win32::hwnd_from_isize(
                    raw,
                ));
            });
        }
        // position first (may change DPI), then the exact physical size of that monitor
        w.set_position(Position::Physical(PhysicalPosition::new(
            m.bounds.x, m.bounds.y,
        )))?;
        w.set_size(Size::Physical(PhysicalSize::new(
            m.bounds.w as u32,
            m.bounds.h as u32,
        )))?;
        w.show()?;
        if Some(i) == cursor_idx || (cursor_idx.is_none() && m.primary) {
            w.set_focus()?;
        }
    }
    tracing::info!(monitors = mons.len(), "region capture started");
    Ok(())
}

#[tauri::command]
pub async fn screenshot_region_commit(
    app: AppHandle,
    monitor: String,
    rect: CssRect,
) -> AppResult<ScreenshotMeta> {
    let session = app
        .state::<RegionState>()
        .session
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
        .ok_or_else(|| AppError::not_found("No region capture in progress"))?;
    let frozen = session
        .frozen
        .get(&monitor)
        .map(|f| (f.image.dimensions(), f.scale));
    let Some(((w, h), scale)) = frozen else {
        end_region(&app, Some(session));
        return Err(AppError::invalid_argument(format!(
            "Unknown monitor \"{monitor}\""
        )));
    };
    let Some((x, y, cw, ch)) = geometry::css_to_image_crop(rect, scale, w, h) else {
        end_region(&app, Some(session));
        let _ = app.emit_to(crate::window::MAIN, EVENT_CANCELLED, serde_json::json!({}));
        return Err(AppError::invalid_argument(
            "The selected region is too small",
        ));
    };
    let cropped =
        image::imageops::crop_imm(&session.frozen[&monitor].image, x, y, cw, ch).to_image();
    end_region(&app, Some(session));
    finish_capture(
        &app,
        cropped,
        monitor,
        encode::DEFAULT_MAX_EDGE,
        Format::Auto,
        encode::DEFAULT_QUALITY,
    )
    .await
}

#[tauri::command]
pub async fn screenshot_region_cancel(app: AppHandle) -> AppResult<()> {
    let s = app
        .state::<RegionState>()
        .session
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take();
    end_region(&app, s);
    let _ = app.emit_to(crate::window::MAIN, EVENT_CANCELLED, serde_json::json!({}));
    Ok(())
}

#[tauri::command]
pub async fn screenshot_get(
    store: tauri::State<'_, ScreenshotStore>,
    id: String,
) -> AppResult<ImageData> {
    let shot = store
        .get(&id)
        .ok_or_else(|| AppError::not_found("Screenshot not found (it may have expired)"))?;
    Ok(ImageData {
        mime: shot.meta.mime.clone(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(shot.data.as_slice()),
    })
}

#[tauri::command]
pub async fn screenshot_discard(
    store: tauri::State<'_, ScreenshotStore>,
    id: String,
) -> AppResult<()> {
    store.remove(&id);
    Ok(())
}

/// `[A-Za-z0-9_-]{1,80}` — ids that are safe as path components.
pub fn safe_component(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 80
        && s.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

pub fn ext_for_mime(mime: &str) -> &'static str {
    if mime == "image/png" {
        "png"
    } else {
        "jpg"
    }
}

pub fn mime_for_path(p: &Path) -> Option<&'static str> {
    match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        _ => None,
    }
}

pub fn screenshots_root<R: Runtime>(app: &AppHandle<R>) -> AppResult<PathBuf> {
    Ok(app.path().app_data_dir()?.join("screenshots"))
}

/// Resolve `path` and make sure it is a file inside `root` (both canonicalized).
pub fn resolve_inside(root: &Path, path: &Path) -> AppResult<PathBuf> {
    let root =
        std::fs::canonicalize(root).map_err(|_| AppError::not_found("No saved screenshots"))?;
    let p = std::fs::canonicalize(path)
        .map_err(|_| AppError::not_found("Screenshot file not found"))?;
    if !p.starts_with(&root) || !p.is_file() {
        return Err(AppError::new(
            "forbidden",
            "Only saved screenshots can be loaded",
        ));
    }
    Ok(p)
}

#[tauri::command]
pub async fn screenshot_save(
    app: AppHandle,
    id: String,
    session_id: String,
) -> AppResult<SavedPath> {
    if !safe_component(&id) || !safe_component(&session_id) {
        return Err(AppError::invalid_argument(
            "Invalid screenshot or session id",
        ));
    }
    let shot = app
        .state::<ScreenshotStore>()
        .get(&id)
        .ok_or_else(|| AppError::not_found("Screenshot not found (it may have expired)"))?;
    let dir = screenshots_root(&app)?.join(&session_id);
    let path = dir.join(format!("{id}.{}", ext_for_mime(&shot.meta.mime)));
    let data = shot.data.clone();
    let p2 = path.clone();
    tauri::async_runtime::spawn_blocking(move || {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(&p2, data.as_slice())
    })
    .await
    .map_err(AppError::from)??;
    Ok(SavedPath {
        path: path.to_string_lossy().into_owned(),
    })
}

#[tauri::command]
pub async fn screenshot_load(app: AppHandle, path: String) -> AppResult<ImageData> {
    let root = screenshots_root(&app)?;
    let p = resolve_inside(&root, Path::new(&path))?;
    let mime = mime_for_path(&p).ok_or_else(|| AppError::new("forbidden", "Not an image file"))?;
    let bytes = tokio::fs::read(&p).await?;
    Ok(ImageData {
        mime: mime.to_string(),
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_encoding_of_device_names() {
        assert_eq!(encode_query_value(r"\\.\DISPLAY1"), "%5C%5C.%5CDISPLAY1");
        assert_eq!(encode_query_value("a b&c"), "a%20b%26c");
    }

    #[test]
    fn ids_and_paths() {
        assert!(safe_component("3f2a9c_b-1"));
        for bad in ["", "..", "a/b", r"a\b", "a.b", &"x".repeat(81)] {
            assert!(!safe_component(bad), "{bad}");
        }
        assert_eq!(ext_for_mime("image/png"), "png");
        assert_eq!(ext_for_mime("image/jpeg"), "jpg");
        assert_eq!(mime_for_path(Path::new("a/b.JPG")), Some("image/jpeg"));
        assert_eq!(mime_for_path(Path::new("a/b.txt")), None);
    }

    #[test]
    fn load_is_confined_to_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("screenshots");
        std::fs::create_dir_all(root.join("s1")).unwrap();
        let inside = root.join("s1").join("a.png");
        std::fs::write(&inside, b"x").unwrap();
        let outside = dir.path().join("secret.png");
        std::fs::write(&outside, b"x").unwrap();
        assert!(resolve_inside(&root, &inside).is_ok());
        assert_eq!(
            resolve_inside(&root, &outside).unwrap_err().code,
            "forbidden"
        );
        let sneaky = root.join("s1").join("..").join("..").join("secret.png");
        assert_eq!(
            resolve_inside(&root, &sneaky).unwrap_err().code,
            "forbidden"
        );
        assert_eq!(
            resolve_inside(&root, &root.join("nope.png"))
                .unwrap_err()
                .code,
            "not_found"
        );
    }

    /// Captures the real primary monitor; run manually.
    #[test]
    #[ignore]
    fn capture_primary_hardware() {
        let m = xcap::Monitor::all()
            .unwrap()
            .into_iter()
            .find(|m| m.is_primary())
            .unwrap();
        let img = capture_monitor(m.name(), (m.x() + 1, m.y() + 1)).unwrap();
        let e = encode::encode(&img, 1568, Format::Auto, 82).unwrap();
        assert!(e.width.max(e.height) <= 1568);
        assert!(e.bytes.len() < 1_000_000);
    }
}
