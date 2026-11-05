//! Pluely X native layer (Windows). The command/event/channel API is specified in
//! docs/CONTRACT.md; design notes are in docs/NATIVE_DESIGN.md.

mod db;
pub mod error;
mod keys;
mod llm;
mod logging;
mod screen;
pub mod secrets;
pub mod settings;
mod shortcuts;
mod tray;
mod window;

// ---- live session (audio/stt/session) — merged from audio branch ----
mod audio;
mod session;
mod stt;
// ---- end live session ----

use std::sync::Arc;

use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime};

use crate::error::{AppError, AppResult};
use crate::secrets::{KeyringStore, Secrets, KEYRING_SERVICE};
use crate::settings::SettingsState;

/// Broadcast by the live session on start/stop (contract §3); the tray listens to it.
pub const EVENT_SESSION_STATUS: &str = "session://status";
/// Background failures for the overlay (contract §8).
pub const EVENT_APP_ERROR: &str = "app://error";

pub fn emit_app_error<R: Runtime>(app: &AppHandle<R>, code: &str, message: &str) {
    let _ = app.emit_to(
        window::MAIN,
        EVENT_APP_ERROR,
        serde_json::json!({ "code": code, "message": message }),
    );
}

/// Stop background work and exit. Safe to call from anywhere (commands run on the async
/// runtime, the tray handler on the main thread): the session stop is awaited on a spawned
/// task, never `block_on` inside the runtime (which panicked and left the app running).
pub fn quit<R: Runtime>(app: &AppHandle<R>) {
    static QUITTING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if QUITTING.swap(true, std::sync::atomic::Ordering::SeqCst) {
        return; // already on the way out (repeated clicks)
    }
    tracing::info!("quit requested");
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // ---- live session (audio/stt/session) — merged from audio branch ----
        // flush Deepgram (CloseStream, ≤ ~4 s) and broadcast session://status before exiting
        session::stop_for_quit(&app).await;
        // ---- end live session ----
        tracing::info!("exiting");
        app.exit(0);
    });
}

pub fn open_logs<R: Runtime>(app: &AppHandle<R>) -> AppResult<()> {
    use tauri_plugin_opener::OpenerExt;
    let dir = app.path().app_log_dir()?;
    std::fs::create_dir_all(&dir)?;
    app.opener()
        .open_path(dir.to_string_lossy(), None::<&str>)
        .map_err(|e| {
            AppError::new("open_failed", "Couldn't open the logs folder").with_detail(e.to_string())
        })
}

fn setup<R: Runtime>(app: &AppHandle<R>) -> Result<(), Box<dyn std::error::Error>> {
    let log_dir = app.path().app_log_dir()?;
    app.manage(logging::init(&log_dir));
    if let Some(note) = db::REPAIR_NOTE.get() {
        tracing::warn!("{note}");
    }

    let config_dir = app.path().app_config_dir()?;
    app.manage(SettingsState::load(config_dir.join("settings.json")));
    app.manage(window::OverlayRuntime::new(config_dir));

    // macOS: a menu-bar app with no Dock icon (the Windows overlay likewise stays out of the
    // taskbar). It's also what lets the overlay sit over other apps' full-screen Spaces.
    #[cfg(target_os = "macos")]
    if let Err(e) = app.set_activation_policy(tauri::ActivationPolicy::Accessory) {
        tracing::warn!(error = %e, "couldn't switch to an accessory (menu-bar) app");
    }

    let settings = app.state::<SettingsState>().get();
    window::create_main(app, &settings)?;
    settings::apply_side_effects(app, None, &settings);
    if let Err(e) = tray::create(app) {
        tracing::error!(error = %e, "tray icon failed");
    }

    tracing::info!("setup complete");
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        // must be first: a second launch just shows the existing overlay
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            window::on_second_instance(app);
        }))
        .plugin(tauri_plugin_opener::init())
        // must come before the SQL plugin, which verifies migration checksums in its setup
        .plugin(db::repair_plugin())
        .plugin(
            tauri_plugin_sql::Builder::default()
                .add_migrations(db::DB_URL, db::migrations())
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| shortcuts::handle(app, shortcut, event))
                .build(),
        )
        .manage(Secrets(Arc::new(KeyringStore::new(KEYRING_SERVICE))))
        .manage(llm::Http::new())
        .manage(llm::LlmState::default())
        .manage(screen::store::ScreenshotStore::default())
        .manage(screen::RegionState::default())
        .manage(shortcuts::ShortcutRegistry::default())
        // ---- live session (audio/stt/session) — merged from audio branch ----
        .manage(session::SessionManager::default())
        // ---- end live session ----
        .invoke_handler(tauri::generate_handler![
            // §1 keys
            keys::keys_status,
            keys::keys_set,
            keys::keys_delete,
            keys::keys_test,
            // §2 llm
            llm::llm_stream,
            llm::llm_cancel,
            // ---- live session (audio/stt/session) — merged from audio branch ----
            // §3 live session
            session::audio_list_devices,
            session::audio_open_mic_privacy_settings,
            session::session_start,
            session::session_stop,
            session::session_set_source,
            session::session_status,
            session::session_reattach,
            // ---- end live session ----
            // §4 window
            window::window_show,
            window::window_hide,
            window::window_toggle,
            window::window_get_state,
            window::window_set_size,
            window::window_set_click_through,
            window::window_set_content_protection,
            window::window_set_always_on_top,
            window::window_begin_typing,
            window::window_end_typing,
            window::window_move_by,
            window::window_reset_position,
            window::dashboard_open,
            window::dashboard_toggle,
            window::app_quit,
            window::app_open_logs,
            window::app_version,
            // §5 screenshots
            screen::screenshot_full,
            screen::screenshot_region_start,
            screen::screenshot_region_commit,
            screen::screenshot_region_cancel,
            screen::screenshot_get,
            screen::screenshot_save,
            screen::screenshot_load,
            screen::screenshot_discard,
            // §6 settings
            settings::settings_get,
            settings::settings_update,
            settings::settings_reset,
            // §7 shortcuts
            shortcuts::shortcuts_validate,
            shortcuts::shortcuts_status,
            // §8 misc
            logging::log_write,
        ])
        .setup(|app| setup(app.handle()))
        .build(tauri::generate_context!())
        .expect("error while building Pluely X");

    app.run(|app, event| match event {
        // closing the last window must not quit: the tray keeps the app alive
        RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
        RunEvent::Exit => {
            // ---- live session (audio/stt/session) — merged from audio branch ----
            // best-effort session stop if quit() wasn't the path taken (no-op when idle)
            session::stop_on_exit(app);
            // ---- end live session ----
            tracing::info!("exit");
        }
        _ => {}
    });
}
