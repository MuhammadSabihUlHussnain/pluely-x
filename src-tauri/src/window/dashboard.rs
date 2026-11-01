//! The dashboard window (sessions + settings). A normal, focusable app window that is created
//! lazily, hidden instead of closed, and reused. Route changes on an existing window are sent
//! as `dashboard://navigate` so the SPA doesn't reload.

use tauri::{
    AppHandle, Emitter, Manager, Runtime, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};

use crate::error::{AppError, AppResult};
use crate::settings::SettingsState;

pub const LABEL: &str = "dashboard";
pub const EVENT_NAVIGATE: &str = "dashboard://navigate";

/// Only in-app routes under /dashboard are allowed.
pub fn normalize_route(route: Option<&str>) -> AppResult<String> {
    let r = route
        .map(str::trim)
        .filter(|r| !r.is_empty())
        .unwrap_or("/dashboard");
    let ok = (r == "/dashboard" || r.starts_with("/dashboard/") || r.starts_with("/dashboard?"))
        && !r.contains("..")
        && !r.contains("//")
        && r.chars().all(|c| c.is_ascii_graphic());
    if !ok {
        return Err(AppError::invalid_argument(format!(
            "Invalid dashboard route \"{r}\""
        )));
    }
    Ok(r.to_string())
}

fn create<R: Runtime>(app: &AppHandle<R>, route: &str) -> AppResult<WebviewWindow<R>> {
    let protected = app
        .state::<SettingsState>()
        .get()
        .overlay
        .content_protection;
    let path = route.trim_start_matches('/');
    let window = WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App(path.into()))
        .title("Pluely X")
        .inner_size(1100.0, 760.0)
        .min_inner_size(800.0, 560.0)
        .center()
        .decorations(true)
        .content_protected(protected)
        .visible(false)
        .build()?;
    let w = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::CloseRequested { api, .. } = event {
            api.prevent_close();
            let _ = w.hide();
        }
    });
    Ok(window)
}

pub fn open<R: Runtime>(app: &AppHandle<R>, route: Option<&str>) -> AppResult<()> {
    let route = normalize_route(route)?;
    let (window, fresh) = match app.get_webview_window(LABEL) {
        Some(w) => (w, false),
        None => (create(app, &route)?, true),
    };
    if !fresh {
        let _ = window.emit_to(LABEL, EVENT_NAVIGATE, serde_json::json!({ "route": route }));
    }
    if window.is_minimized().unwrap_or(false) {
        window.unminimize()?;
    }
    window.show()?;
    window.set_focus()?;
    Ok(())
}

pub fn toggle<R: Runtime>(app: &AppHandle<R>, route: Option<&str>) -> AppResult<()> {
    if let Some(w) = app.get_webview_window(LABEL) {
        let visible = w.is_visible().unwrap_or(false) && !w.is_minimized().unwrap_or(false);
        if visible && (w.is_focused().unwrap_or(false) || route.is_none()) {
            w.hide()?;
            return Ok(());
        }
    }
    open(app, route)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes() {
        assert_eq!(normalize_route(None).unwrap(), "/dashboard");
        assert_eq!(normalize_route(Some("  ")).unwrap(), "/dashboard");
        assert_eq!(
            normalize_route(Some("/dashboard/settings")).unwrap(),
            "/dashboard/settings"
        );
        assert_eq!(
            normalize_route(Some("/dashboard?tab=keys")).unwrap(),
            "/dashboard?tab=keys"
        );
        for bad in [
            "/",
            "/capture",
            "https://evil.example",
            "/dashboard/../x",
            "/dashboardx",
            "/dashboard//x",
            "/dashboard/a b",
        ] {
            assert!(normalize_route(Some(bad)).is_err(), "{bad}");
        }
    }
}
