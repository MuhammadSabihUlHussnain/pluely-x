//! System tray (contract §8). Left click toggles the overlay; the menu mirrors overlay state.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::menu::{CheckMenuItem, Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, Listener, Manager, Runtime};

use crate::error::AppResult;
use crate::settings::SettingsState;
use crate::shortcuts::{self, ShortcutAction};

pub const TRAY_ID: &str = "main";

/// Handles to the menu items whose text/check state changes.
pub struct TrayMenu<R: Runtime> {
    show_hide: MenuItem<R>,
    listen: MenuItem<R>,
    click_through: CheckMenuItem<R>,
    protection: CheckMenuItem<R>,
    listening: AtomicBool,
}

pub fn create<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    let show_hide = MenuItem::with_id(app, "show_hide", "Hide overlay", true, None::<&str>)?;
    let listen = MenuItem::with_id(app, "listen", "Start listening", true, None::<&str>)?;
    let click_through = CheckMenuItem::with_id(
        app,
        "click_through",
        "Click-through",
        true,
        false,
        None::<&str>,
    )?;
    let protection = CheckMenuItem::with_id(
        app,
        "protection",
        "Hide from screen share",
        true,
        true,
        None::<&str>,
    )?;
    let dashboard = MenuItem::with_id(app, "dashboard", "Open dashboard", true, None::<&str>)?;
    let logs = MenuItem::with_id(app, "logs", "Open logs", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit Pluely X", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &show_hide,
            &listen,
            &sep1,
            &click_through,
            &protection,
            &sep2,
            &dashboard,
            &logs,
            &sep3,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Pluely X")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id().as_ref()))
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = crate::window::toggle(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;

    app.manage(TrayMenu {
        show_hide,
        listen,
        click_through,
        protection,
        listening: AtomicBool::new(false),
    });

    // keep "Start/Stop listening" in sync with the live session (contract §3 broadcast)
    let handle = app.clone();
    app.listen_any(crate::EVENT_SESSION_STATUS, move |event| {
        let active = serde_json::from_str::<serde_json::Value>(event.payload())
            .ok()
            .and_then(|v| v.get("active").and_then(|a| a.as_bool()))
            .unwrap_or(false);
        if let Some(m) = handle.try_state::<TrayMenu<R>>() {
            m.listening.store(active, Ordering::SeqCst);
        }
        refresh(&handle);
    });
    refresh(app);
    tracing::info!("tray ready");
    Ok(())
}

fn on_menu<R: Runtime>(app: &AppHandle<R>, id: &str) {
    let res: AppResult<()> = match id {
        "show_hide" => crate::window::toggle(app),
        "listen" => {
            let _ = crate::window::show(app);
            shortcuts::emit_action(app, ShortcutAction::ToggleListen);
            Ok(())
        }
        "click_through" => crate::settings::update_with(app, |s| {
            s.overlay.click_through = !s.overlay.click_through
        })
        .map(|_| ()),
        "protection" => crate::settings::update_with(app, |s| {
            s.overlay.content_protection = !s.overlay.content_protection
        })
        .map(|_| ()),
        "dashboard" => crate::window::dashboard::open(app, None),
        "logs" => crate::open_logs(app),
        "quit" => {
            crate::quit(app);
            Ok(())
        }
        _ => Ok(()),
    };
    if let Err(e) = res {
        tracing::warn!(item = id, error = %e, "tray action failed");
    }
    refresh(app);
}

/// Re-sync item texts and check marks with the current state.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(m) = app.try_state::<TrayMenu<R>>() else {
        return;
    };
    let visible = crate::window::main_window(app)
        .ok()
        .and_then(|w| w.is_visible().ok())
        .unwrap_or(false);
    let o = app.state::<SettingsState>().get().overlay;
    let _ = m.show_hide.set_text(if visible {
        "Hide overlay"
    } else {
        "Show overlay"
    });
    let _ = m.listen.set_text(if m.listening.load(Ordering::SeqCst) {
        "Stop listening"
    } else {
        "Start listening"
    });
    let _ = m.click_through.set_checked(o.click_through);
    let _ = m.protection.set_checked(o.content_protection);
}
