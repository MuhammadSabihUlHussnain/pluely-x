//! The overlay window (`main`): built in code, non-activating, out of Alt+Tab, topmost,
//! hidden from capture, clamped on screen (contract §4).

pub mod dashboard;
pub mod placement;
#[cfg(windows)]
pub mod win32;

use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, Position, Runtime, Size,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

use crate::error::{AppError, AppResult};
use crate::screen::geometry::{MonitorGeom, Rect};
use crate::settings::{
    AppSettings, OverlaySettings, SettingsState, MIN_OVERLAY_HEIGHT, MIN_OVERLAY_WIDTH,
};
use placement::SavedPlacement;

pub const MAIN: &str = "main";
pub const EVENT_STATE: &str = "window://state";
pub const EVENT_TYPING: &str = "window://typing";
const PLACEMENT_FILE: &str = "placement.json";
const TOPMOST_INTERVAL: Duration = Duration::from_secs(3);

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Bounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WindowState {
    pub visible: bool,
    pub click_through: bool,
    pub content_protected: bool,
    pub always_on_top: bool,
    pub typing: bool,
    pub bounds: Bounds,
}

/// Managed runtime state for the overlay.
pub struct OverlayRuntime {
    placement_path: PathBuf,
    prev_foreground: AtomicIsize,
    typing_since: Mutex<Option<Instant>>,
    placement_gen: AtomicU64,
    state_gen: AtomicU64,
}

impl OverlayRuntime {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            placement_path: config_dir.join(PLACEMENT_FILE),
            prev_foreground: AtomicIsize::new(0),
            typing_since: Mutex::new(None),
            placement_gen: AtomicU64::new(0),
            state_gen: AtomicU64::new(0),
        }
    }
}

pub fn main_window<R: Runtime>(app: &AppHandle<R>) -> AppResult<WebviewWindow<R>> {
    app.get_webview_window(MAIN)
        .ok_or_else(|| AppError::new("window_missing", "The overlay window is not available"))
}

/// All monitors (physical bounds + work areas), primary flagged.
pub fn monitors<R: Runtime>(app: &AppHandle<R>) -> AppResult<Vec<MonitorGeom>> {
    let primary_name = app.primary_monitor()?.and_then(|m| m.name().cloned());
    let list = app.available_monitors()?;
    Ok(list
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let name = m.name().cloned().unwrap_or_else(|| format!("monitor-{i}"));
            let p = m.position();
            let s = m.size();
            let wa = m.work_area();
            MonitorGeom {
                primary: primary_name.as_deref() == Some(name.as_str()),
                name,
                bounds: Rect::new(p.x, p.y, s.width as i32, s.height as i32),
                work: Rect::new(
                    wa.position.x,
                    wa.position.y,
                    wa.size.width as i32,
                    wa.size.height as i32,
                ),
                scale: m.scale_factor(),
            }
        })
        .collect())
}

fn outer_rect<R: Runtime>(w: &WebviewWindow<R>) -> AppResult<Rect> {
    let p = w.outer_position()?;
    let s = w.outer_size()?;
    Ok(Rect::new(p.x, p.y, s.width as i32, s.height as i32))
}

pub fn main_scale<R: Runtime>(app: &AppHandle<R>) -> f64 {
    main_window(app)
        .and_then(|w| Ok(w.scale_factor()?))
        .unwrap_or(1.0)
}

// ---------------------------------------------------------------------------------------------
// Creation
// ---------------------------------------------------------------------------------------------

pub fn create_main<R: Runtime>(
    app: &AppHandle<R>,
    settings: &AppSettings,
) -> AppResult<WebviewWindow<R>> {
    let o = &settings.overlay;
    let window = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::App("index.html".into()))
        .title("Pluely X")
        .inner_size(o.width, o.height)
        .min_inner_size(MIN_OVERLAY_WIDTH, MIN_OVERLAY_HEIGHT)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(true)
        .maximizable(false)
        .minimizable(false)
        .skip_taskbar(true)
        .always_on_top(o.always_on_top)
        .content_protected(o.content_protection)
        .focusable(false)
        .focused(false)
        .visible(false)
        .build()?;

    #[cfg(windows)]
    {
        // setup runs on the main thread, which owns the window: safe to subclass here.
        let hwnd = window.hwnd()?;
        if !win32::install_overlay_subclass(hwnd) {
            tracing::warn!("overlay subclass install failed; Alt+Tab/activation fixes inactive");
        }
    }

    // placement: saved position (clamped) or top-centre of the primary work area
    let saved = std::fs::read(&app.state::<OverlayRuntime>().placement_path)
        .ok()
        .and_then(|b| serde_json::from_slice::<SavedPlacement>(&b).ok());
    if let Ok(mons) = monitors(app) {
        if let Some(r) = placement::restore(saved.as_ref(), (o.width, o.height), &mons) {
            window.set_position(Position::Physical(PhysicalPosition::new(r.x, r.y)))?;
        }
    }
    if o.click_through {
        window.set_ignore_cursor_events(true)?;
    }

    let app2 = app.clone();
    window.on_window_event(move |e| on_main_event(&app2, e));
    window.show()?;
    #[cfg(windows)]
    win32::ensure_topmost(window.hwnd()?);
    spawn_topmost_ticker(app.clone());
    tracing::info!(saved = saved.is_some(), "overlay created");
    Ok(window)
}

fn on_main_event<R: Runtime>(app: &AppHandle<R>, event: &WindowEvent) {
    match event {
        WindowEvent::Focused(false) => {
            #[cfg(windows)]
            if win32::is_typing() {
                let since = *app
                    .state::<OverlayRuntime>()
                    .typing_since
                    .lock()
                    .unwrap_or_else(|e| e.into_inner());
                // ignore the transient blur that can happen while focus is being taken
                if since
                    .map(|t| t.elapsed() > Duration::from_millis(300))
                    .unwrap_or(true)
                {
                    let _ = end_typing(app, false);
                }
            }
        }
        WindowEvent::Moved(_) => {
            schedule_placement_save(app);
            schedule_state_broadcast(app);
        }
        WindowEvent::Resized(_) => schedule_state_broadcast(app),
        WindowEvent::ScaleFactorChanged { .. } => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(200)).await;
                let _ = reclamp_main(&app);
            });
        }
        WindowEvent::CloseRequested { api, .. } => {
            // Alt+F4 while typing must not destroy the overlay
            api.prevent_close();
            let _ = hide(app);
        }
        _ => {}
    }
}

fn spawn_topmost_ticker<R: Runtime>(app: AppHandle<R>) {
    tauri::async_runtime::spawn(async move {
        let mut tick = tokio::time::interval(TOPMOST_INTERVAL);
        loop {
            tick.tick().await;
            let Some(w) = app.get_webview_window(MAIN) else {
                break;
            };
            let aot = app.state::<SettingsState>().get().overlay.always_on_top;
            if aot && w.is_visible().unwrap_or(false) {
                #[cfg(windows)]
                if let Ok(h) = w.hwnd() {
                    win32::ensure_topmost(h);
                }
            }
        }
    });
}

fn schedule_placement_save<R: Runtime>(app: &AppHandle<R>) {
    let rt = app.state::<OverlayRuntime>();
    let gen = rt.placement_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(500)).await;
        let rt = app.state::<OverlayRuntime>();
        if rt.placement_gen.load(Ordering::SeqCst) != gen {
            return;
        }
        if let Err(e) = save_placement(&app) {
            tracing::warn!(error = %e, "placement save failed");
        }
    });
}

fn save_placement<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let w = main_window(app)?;
    if !w.is_visible().unwrap_or(false) {
        return Ok(());
    }
    let r = outer_rect(&w)?;
    let mon = w.current_monitor()?;
    let saved = SavedPlacement {
        x: r.x,
        y: r.y,
        monitor: mon.as_ref().and_then(|m| m.name().cloned()),
        scale: mon.as_ref().map(|m| m.scale_factor()),
    };
    let path = &app.state::<OverlayRuntime>().placement_path;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, serde_json::to_vec(&saved)?)?;
    std::fs::rename(&tmp, path)?;
    Ok(())
}

fn schedule_state_broadcast<R: Runtime>(app: &AppHandle<R>) {
    let rt = app.state::<OverlayRuntime>();
    let gen = rt.state_gen.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(120)).await;
        if app
            .state::<OverlayRuntime>()
            .state_gen
            .load(Ordering::SeqCst)
            == gen
        {
            broadcast_state(&app);
        }
    });
}

// ---------------------------------------------------------------------------------------------
// State + operations (shared by commands, shortcuts, tray)
// ---------------------------------------------------------------------------------------------

pub fn current_state<R: Runtime>(app: &AppHandle<R>) -> AppResult<WindowState> {
    let w = main_window(app)?;
    let s = app.state::<SettingsState>().get();
    let scale = w.scale_factor().unwrap_or(1.0);
    let r = outer_rect(&w)?;
    #[cfg(windows)]
    let typing = win32::is_typing();
    #[cfg(not(windows))]
    let typing = false;
    Ok(WindowState {
        visible: w.is_visible()?,
        click_through: s.overlay.click_through,
        content_protected: s.overlay.content_protection,
        always_on_top: s.overlay.always_on_top,
        typing,
        bounds: Bounds {
            x: r.x as f64 / scale,
            y: r.y as f64 / scale,
            width: r.w as f64 / scale,
            height: r.h as f64 / scale,
        },
    })
}

pub fn broadcast_state<R: Runtime>(app: &AppHandle<R>) {
    match current_state(app) {
        Ok(s) => {
            let _ = app.emit(EVENT_STATE, s);
        }
        Err(e) => tracing::debug!(error = %e, "no window state to broadcast"),
    }
}

fn changed<R: Runtime>(app: &AppHandle<R>) -> AppResult<WindowState> {
    let s = current_state(app)?;
    let _ = app.emit(EVENT_STATE, &s);
    crate::tray::refresh(app);
    Ok(s)
}

/// Show without taking focus (the subclass strips activation), re-clamp, re-assert topmost.
pub fn show<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let w = main_window(app)?;
    w.show()?;
    let _ = reclamp_main(app);
    #[cfg(windows)]
    if app.state::<SettingsState>().get().overlay.always_on_top {
        win32::ensure_topmost(w.hwnd()?);
    }
    Ok(())
}

pub fn hide<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    #[cfg(windows)]
    if win32::is_typing() {
        let _ = end_typing(app, true);
    }
    main_window(app)?.hide()?;
    Ok(())
}

pub fn toggle<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    if main_window(app)?.is_visible()? {
        hide(app)?;
    } else {
        show(app)?;
    }
    let _ = changed(app);
    Ok(())
}

/// Apply click-through / always-on-top / content protection from settings. Content protection
/// covers every app window, including the dashboard and capture overlays.
pub fn apply_overlay_flags<R: Runtime>(app: &AppHandle<R>, o: &OverlaySettings) {
    if let Ok(w) = main_window(app) {
        if let Err(e) = w.set_ignore_cursor_events(o.click_through) {
            tracing::warn!(error = %e, "set click-through failed");
        }
        if let Err(e) = w.set_always_on_top(o.always_on_top) {
            tracing::warn!(error = %e, "set always-on-top failed");
        }
        #[cfg(windows)]
        if o.always_on_top {
            if let Ok(h) = w.hwnd() {
                win32::ensure_topmost(h);
            }
        }
    }
    for (label, w) in app.webview_windows() {
        if let Err(e) = w.set_content_protected(o.content_protection) {
            tracing::warn!(%label, error = %e, "set content protection failed");
        }
    }
}

/// Typing mode: the overlay becomes focusable and takes focus; the previous foreground
/// window is remembered so focus can be handed back (to Zoom/Teams) afterwards.
pub fn begin_typing<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let w = main_window(app)?;
    if !w.is_visible()? {
        show(app)?;
    }
    #[cfg(windows)]
    {
        let rt = app.state::<OverlayRuntime>();
        let own = w.hwnd().map(win32::hwnd_to_isize).unwrap_or(0);
        let fg = win32::foreground_window();
        let was_typing = win32::is_typing();
        if !was_typing && fg != 0 && fg != own && !is_own_window(app, fg) {
            rt.prev_foreground.store(fg, Ordering::SeqCst);
        }
        *rt.typing_since.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
        win32::set_typing(true);
        if w.set_ignore_cursor_events(false).is_err() {
            tracing::warn!("couldn't lift click-through for typing");
        }
        w.set_focusable(true)?;
        w.set_focus()?;
        if !was_typing {
            let _ = app.emit_to(MAIN, EVENT_TYPING, serde_json::json!({ "active": true }));
            let _ = changed(app);
        }
    }
    Ok(())
}

pub fn end_typing<R: Runtime>(app: &AppHandle<R>, restore_focus: bool) -> AppResult<()> {
    #[cfg(windows)]
    {
        let w = main_window(app)?;
        let rt = app.state::<OverlayRuntime>();
        let was_typing = win32::is_typing();
        win32::set_typing(false);
        *rt.typing_since.lock().unwrap_or_else(|e| e.into_inner()) = None;
        w.set_focusable(false)?;
        let click_through = app.state::<SettingsState>().get().overlay.click_through;
        if click_through {
            let _ = w.set_ignore_cursor_events(true);
        }
        let prev = rt.prev_foreground.swap(0, Ordering::SeqCst);
        if restore_focus && prev != 0 && !win32::restore_foreground(prev) {
            tracing::debug!("previous foreground window could not be restored");
        }
        if was_typing {
            let _ = app.emit_to(MAIN, EVENT_TYPING, serde_json::json!({ "active": false }));
            let _ = changed(app);
        }
    }
    #[cfg(not(windows))]
    let _ = (app, restore_focus);
    Ok(())
}

#[cfg(windows)]
fn is_own_window<R: Runtime>(app: &AppHandle<R>, raw: isize) -> bool {
    app.webview_windows()
        .values()
        .any(|w| w.hwnd().map(win32::hwnd_to_isize).ok() == Some(raw))
}

pub fn move_by_physical<R: Runtime>(app: &AppHandle<R>, dx: i32, dy: i32) -> AppResult<()> {
    let w = main_window(app)?;
    let r = outer_rect(&w)?;
    let mons = monitors(app)?;
    let n = placement::move_by(&r, dx, dy, &mons);
    if n != r {
        w.set_position(Position::Physical(PhysicalPosition::new(n.x, n.y)))?;
    }
    Ok(())
}

fn reclamp_main<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let w = main_window(app)?;
    let r = outer_rect(&w)?;
    let n = placement::reclamp(&r, &monitors(app)?);
    if n.x != r.x || n.y != r.y {
        w.set_position(Position::Physical(PhysicalPosition::new(n.x, n.y)))?;
    }
    if n.w != r.w || n.h != r.h {
        w.set_size(Size::Physical(tauri::PhysicalSize::new(
            n.w as u32, n.h as u32,
        )))?;
    }
    Ok(())
}

fn set_size_logical<R: Runtime>(
    app: &AppHandle<R>,
    width: Option<f64>,
    height: Option<f64>,
) -> AppResult<()> {
    let w = main_window(app)?;
    let scale = w.scale_factor()?;
    let cur = w.inner_size()?.to_logical::<f64>(scale);
    let mut nw = width.unwrap_or(cur.width);
    let mut nh = height.unwrap_or(cur.height);
    if !nw.is_finite() || !nh.is_finite() {
        return Err(AppError::invalid_argument("width/height must be numbers"));
    }
    // clamp to the current monitor's work area
    if let Some(m) = w.current_monitor()? {
        let wa = m.work_area();
        let s = m.scale_factor();
        nw = nw.min(wa.size.width as f64 / s);
        nh = nh.min(wa.size.height as f64 / s);
    }
    nw = nw.max(MIN_OVERLAY_WIDTH);
    nh = nh.max(MIN_OVERLAY_HEIGHT);
    w.set_size(Size::Logical(LogicalSize::new(nw.round(), nh.round())))?;
    let _ = reclamp_main(app);
    Ok(())
}

pub fn reset_position<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let w = main_window(app)?;
    let r = outer_rect(&w)?;
    let mons = monitors(app)?;
    let p = crate::screen::geometry::primary_index(&mons)
        .ok_or_else(|| AppError::internal("No monitors found"))?;
    let n = placement::top_center(&mons[p], r.w, r.h);
    w.set_position(Position::Physical(PhysicalPosition::new(n.x, n.y)))?;
    Ok(())
}

/// Second launch: show the existing overlay without activating it.
pub fn on_second_instance<R: Runtime>(app: &AppHandle<R>) {
    tracing::info!("second instance started; showing the overlay");
    if let Err(e) = show(app) {
        tracing::warn!(error = %e, "couldn't show overlay for second instance");
    }
    let _ = changed(app);
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn window_show(app: AppHandle) -> AppResult<WindowState> {
    show(&app)?;
    changed(&app)
}

#[tauri::command]
pub async fn window_hide(app: AppHandle) -> AppResult<WindowState> {
    hide(&app)?;
    changed(&app)
}

#[tauri::command]
pub async fn window_toggle(app: AppHandle) -> AppResult<WindowState> {
    toggle(&app)?;
    current_state(&app)
}

#[tauri::command]
pub async fn window_get_state(app: AppHandle) -> AppResult<WindowState> {
    current_state(&app)
}

#[tauri::command]
pub async fn window_set_size(
    app: AppHandle,
    width: Option<f64>,
    height: Option<f64>,
) -> AppResult<WindowState> {
    set_size_logical(&app, width, height)?;
    changed(&app)
}

#[tauri::command]
pub async fn window_set_click_through(app: AppHandle, enabled: bool) -> AppResult<WindowState> {
    crate::settings::update_with(&app, |s| s.overlay.click_through = enabled)?;
    // settings side effects already broadcast window://state and refresh the tray
    current_state(&app)
}

#[tauri::command]
pub async fn window_set_content_protection(
    app: AppHandle,
    enabled: bool,
) -> AppResult<WindowState> {
    crate::settings::update_with(&app, |s| s.overlay.content_protection = enabled)?;
    // settings side effects already broadcast window://state and refresh the tray
    current_state(&app)
}

#[tauri::command]
pub async fn window_set_always_on_top(app: AppHandle, enabled: bool) -> AppResult<WindowState> {
    crate::settings::update_with(&app, |s| s.overlay.always_on_top = enabled)?;
    // settings side effects already broadcast window://state and refresh the tray
    current_state(&app)
}

#[tauri::command]
pub async fn window_begin_typing(app: AppHandle) -> AppResult<WindowState> {
    begin_typing(&app)?;
    current_state(&app)
}

#[tauri::command]
pub async fn window_end_typing(app: AppHandle, restore_focus: bool) -> AppResult<WindowState> {
    end_typing(&app, restore_focus)?;
    current_state(&app)
}

#[tauri::command]
pub async fn window_move_by(app: AppHandle, dx: f64, dy: f64) -> AppResult<WindowState> {
    if !dx.is_finite() || !dy.is_finite() {
        return Err(AppError::invalid_argument("dx/dy must be numbers"));
    }
    let s = main_scale(&app);
    move_by_physical(&app, (dx * s).round() as i32, (dy * s).round() as i32)?;
    changed(&app)
}

#[tauri::command]
pub async fn window_reset_position(app: AppHandle) -> AppResult<WindowState> {
    reset_position(&app)?;
    changed(&app)
}

#[tauri::command]
pub async fn dashboard_open(app: AppHandle, route: Option<String>) -> AppResult<()> {
    dashboard::open(&app, route.as_deref())
}

#[tauri::command]
pub async fn dashboard_toggle(app: AppHandle, route: Option<String>) -> AppResult<()> {
    dashboard::toggle(&app, route.as_deref())
}

#[tauri::command]
pub async fn app_quit(app: AppHandle) -> AppResult<()> {
    crate::quit(&app);
    Ok(())
}

#[tauri::command]
pub async fn app_open_logs(app: AppHandle) -> AppResult<()> {
    crate::open_logs(&app)
}

#[tauri::command]
pub async fn app_version(app: AppHandle) -> AppResult<String> {
    Ok(app.package_info().version.to_string())
}
