//! Global shortcuts: defaults, parsing/validation, (re-)registration from settings and the
//! central handler (contract §7).
//!
//! Handled natively: toggle_overlay, toggle_click_through, toggle_dashboard, move_* (hold to
//! move) and focus_input (begins typing mode, then is also emitted). Everything else is emitted
//! to the overlay as `shortcut://action`.

use std::collections::{BTreeMap, HashMap};
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, Shortcut, ShortcutEvent, ShortcutState};

use crate::error::AppResult;
use crate::settings::ShortcutBinding;

pub const EVENT_ACTION: &str = "shortcut://action";
pub const EVENT_ERROR: &str = "shortcut://error";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ShortcutAction {
    ToggleOverlay,
    ToggleListen,
    AskNow,
    ScreenshotFull,
    ScreenshotRegion,
    FocusInput,
    ToggleClickThrough,
    ToggleDashboard,
    MoveUp,
    MoveDown,
    MoveLeft,
    MoveRight,
}

impl ShortcutAction {
    pub const ALL: [ShortcutAction; 12] = [
        ShortcutAction::ToggleOverlay,
        ShortcutAction::ToggleListen,
        ShortcutAction::AskNow,
        ShortcutAction::ScreenshotFull,
        ShortcutAction::ScreenshotRegion,
        ShortcutAction::FocusInput,
        ShortcutAction::ToggleClickThrough,
        ShortcutAction::ToggleDashboard,
        ShortcutAction::MoveUp,
        ShortcutAction::MoveDown,
        ShortcutAction::MoveLeft,
        ShortcutAction::MoveRight,
    ];

    pub fn default_key(self) -> &'static str {
        match self {
            ShortcutAction::ToggleOverlay => "ctrl+backslash",
            ShortcutAction::ToggleListen => "ctrl+alt+l",
            ShortcutAction::AskNow => "ctrl+alt+enter",
            ShortcutAction::ScreenshotFull => "ctrl+alt+s",
            ShortcutAction::ScreenshotRegion => "ctrl+alt+r",
            ShortcutAction::FocusInput => "ctrl+alt+space",
            ShortcutAction::ToggleClickThrough => "ctrl+alt+c",
            ShortcutAction::ToggleDashboard => "ctrl+alt+d",
            ShortcutAction::MoveUp => "ctrl+alt+arrowup",
            ShortcutAction::MoveDown => "ctrl+alt+arrowdown",
            ShortcutAction::MoveLeft => "ctrl+alt+arrowleft",
            ShortcutAction::MoveRight => "ctrl+alt+arrowright",
        }
    }

    /// (dx, dy) unit vector for move_* actions.
    pub fn move_dir(self) -> Option<(i32, i32)> {
        match self {
            ShortcutAction::MoveUp => Some((0, -1)),
            ShortcutAction::MoveDown => Some((0, 1)),
            ShortcutAction::MoveLeft => Some((-1, 0)),
            ShortcutAction::MoveRight => Some((1, 0)),
            _ => None,
        }
    }
}

pub fn default_bindings() -> BTreeMap<ShortcutAction, ShortcutBinding> {
    ShortcutAction::ALL
        .iter()
        .map(|a| {
            (
                *a,
                ShortcutBinding {
                    key: a.default_key().to_string(),
                    enabled: true,
                },
            )
        })
        .collect()
}

/// Parse an accelerator like `ctrl+alt+l`. Requires Ctrl, Alt or Win unless the key is F1-F24,
/// so a binding can never swallow ordinary typing.
pub fn parse(key: &str) -> Result<Shortcut, String> {
    let norm = key.trim().to_ascii_lowercase().replace(' ', "");
    if norm.is_empty() {
        return Err("empty shortcut".into());
    }
    let shortcut = Shortcut::from_str(&norm).map_err(|e| e.to_string())?;
    let tokens: Vec<&str> = norm.split('+').collect();
    let has_strong_mod = tokens[..tokens.len() - 1].iter().any(|t| {
        matches!(
            *t,
            "ctrl"
                | "control"
                | "alt"
                | "option"
                | "super"
                | "cmd"
                | "command"
                | "win"
                | "cmdorctrl"
                | "commandorcontrol"
        )
    });
    let last = tokens[tokens.len() - 1];
    let is_fkey = last.len() >= 2
        && last.starts_with('f')
        && last[1..]
            .parse::<u8>()
            .map(|n| (1..=24).contains(&n))
            .unwrap_or(false);
    if !has_strong_mod && !is_fkey {
        return Err("shortcut needs Ctrl, Alt or Win (or an F-key)".into());
    }
    Ok(shortcut)
}

/// Every enabled binding parses and no two enabled bindings share a key.
pub fn validate_bindings(map: &BTreeMap<ShortcutAction, ShortcutBinding>) -> Result<(), String> {
    let mut seen: HashMap<u32, ShortcutAction> = HashMap::new();
    for (action, b) in map {
        if !b.enabled || b.key.trim().is_empty() {
            continue;
        }
        let s = parse(&b.key)
            .map_err(|e| format!("Shortcut for {action:?} (\"{}\") is invalid: {e}", b.key))?;
        if let Some(other) = seen.insert(s.id(), *action) {
            return Err(format!("{action:?} and {other:?} both use \"{}\"", b.key));
        }
    }
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutFailure {
    pub action: ShortcutAction,
    pub key: String,
    pub error: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutsStatus {
    pub registered: Vec<ShortcutAction>,
    pub failures: Vec<ShortcutFailure>,
}

/// Managed state: what is registered right now (hotkey id -> action) and the last failures.
#[derive(Default)]
pub struct ShortcutRegistry {
    by_id: Mutex<HashMap<u32, (ShortcutAction, Shortcut)>>,
    failures: Mutex<Vec<ShortcutFailure>>,
    moving: Mutex<HashMap<ShortcutAction, Arc<AtomicBool>>>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Re-register all shortcuts from settings. Failures are stored and emitted as
/// `shortcut://error` (main); they never fail the settings update.
pub fn apply<R: Runtime>(app: &AppHandle<R>, bindings: &BTreeMap<ShortcutAction, ShortcutBinding>) {
    let reg = app.state::<ShortcutRegistry>();
    stop_all_moves(app);
    let gs = app.global_shortcut();
    {
        let mut by_id = lock(&reg.by_id);
        for (_, (action, s)) in by_id.drain() {
            if let Err(e) = gs.unregister(s) {
                tracing::warn!(?action, error = %e, "unregister failed");
            }
        }
    }
    let mut registered = HashMap::new();
    let mut failures = Vec::new();
    for (action, b) in bindings {
        if !b.enabled || b.key.trim().is_empty() {
            continue;
        }
        let shortcut = match parse(&b.key) {
            Ok(s) => s,
            Err(e) => {
                failures.push(ShortcutFailure {
                    action: *action,
                    key: b.key.clone(),
                    error: e,
                });
                continue;
            }
        };
        match gs.register(shortcut) {
            Ok(()) => {
                registered.insert(shortcut.id(), (*action, shortcut));
            }
            Err(e) => {
                tracing::warn!(?action, key = %b.key, error = %e, "shortcut registration failed");
                failures.push(ShortcutFailure {
                    action: *action,
                    key: b.key.clone(),
                    error: friendly_register_error(&e.to_string()),
                });
            }
        }
    }
    tracing::info!(
        registered = registered.len(),
        failed = failures.len(),
        "shortcuts applied"
    );
    *lock(&reg.by_id) = registered;
    *lock(&reg.failures) = failures.clone();
    if !failures.is_empty() {
        let _ = app.emit_to(
            "main",
            EVENT_ERROR,
            serde_json::json!({ "failures": failures }),
        );
    }
}

fn friendly_register_error(raw: &str) -> String {
    if raw.to_ascii_lowercase().contains("already") {
        "Another app is already using this shortcut".into()
    } else {
        raw.to_string()
    }
}

/// Global-shortcut plugin handler (runs on the main thread).
pub fn handle<R: Runtime>(app: &AppHandle<R>, shortcut: &Shortcut, event: ShortcutEvent) {
    let action = {
        let reg = app.state::<ShortcutRegistry>();
        let by_id = lock(&reg.by_id);
        by_id.get(&shortcut.id()).map(|(a, _)| *a)
    };
    let Some(action) = action else { return };
    match event.state() {
        ShortcutState::Pressed => on_pressed(app, action),
        ShortcutState::Released => {
            if action.move_dir().is_some() {
                stop_move(app, action);
            }
        }
    }
}

fn on_pressed<R: Runtime>(app: &AppHandle<R>, action: ShortcutAction) {
    tracing::debug!(?action, "shortcut");
    match action {
        ShortcutAction::ToggleOverlay => {
            if let Err(e) = crate::window::toggle(app) {
                tracing::warn!(error = %e, "toggle overlay failed");
            }
        }
        ShortcutAction::ToggleClickThrough => {
            if let Err(e) = crate::settings::update_with(app, |s| {
                s.overlay.click_through = !s.overlay.click_through
            }) {
                tracing::warn!(error = %e, "toggle click-through failed");
            }
        }
        ShortcutAction::ToggleDashboard => {
            if let Err(e) = crate::window::dashboard::toggle(app, None) {
                tracing::warn!(error = %e, "toggle dashboard failed");
            }
        }
        ShortcutAction::MoveUp
        | ShortcutAction::MoveDown
        | ShortcutAction::MoveLeft
        | ShortcutAction::MoveRight => start_move(app, action),
        ShortcutAction::FocusInput => {
            if let Err(e) = crate::window::begin_typing(app) {
                tracing::warn!(error = %e, "begin typing failed");
            }
            emit_action(app, action);
        }
        ShortcutAction::ToggleListen
        | ShortcutAction::AskNow
        | ShortcutAction::ScreenshotFull
        | ShortcutAction::ScreenshotRegion => emit_action(app, action),
    }
}

pub fn emit_action<R: Runtime>(app: &AppHandle<R>, action: ShortcutAction) {
    if let Err(e) = app.emit_to(
        "main",
        EVENT_ACTION,
        serde_json::json!({ "action": action }),
    ) {
        tracing::warn!(error = %e, "emit shortcut action failed");
    }
}

/// Hold-to-move: step every 16 ms until the key is released.
fn start_move<R: Runtime>(app: &AppHandle<R>, action: ShortcutAction) {
    let Some((ux, uy)) = action.move_dir() else {
        return;
    };
    let reg = app.state::<ShortcutRegistry>();
    let mut moving = lock(&reg.moving);
    if moving.contains_key(&action) {
        return;
    }
    let stop = Arc::new(AtomicBool::new(false));
    moving.insert(action, stop.clone());
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        // Safety net: global-hotkey can miss a release; never move for more than 10 s.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(16));
        while !stop.load(Ordering::Relaxed) && tokio::time::Instant::now() < deadline {
            tick.tick().await;
            let step = 10.0 * crate::window::main_scale(&app);
            let (dx, dy) = ((ux as f64 * step) as i32, (uy as f64 * step) as i32);
            if let Err(e) = crate::window::move_by_physical(&app, dx, dy) {
                tracing::warn!(error = %e, "move failed");
                break;
            }
        }
        lock(&app.state::<ShortcutRegistry>().moving).remove(&action);
        crate::window::broadcast_state(&app);
    });
}

fn stop_move<R: Runtime>(app: &AppHandle<R>, action: ShortcutAction) {
    if let Some(flag) = lock(&app.state::<ShortcutRegistry>().moving).get(&action) {
        flag.store(true, Ordering::Relaxed);
    }
}

fn stop_all_moves<R: Runtime>(app: &AppHandle<R>) {
    for flag in lock(&app.state::<ShortcutRegistry>().moving).values() {
        flag.store(true, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn shortcuts_validate(key: String) -> AppResult<bool> {
    Ok(parse(&key).is_ok())
}

#[tauri::command]
pub async fn shortcuts_status(
    registry: tauri::State<'_, ShortcutRegistry>,
) -> AppResult<ShortcutsStatus> {
    let mut registered: Vec<ShortcutAction> =
        lock(&registry.by_id).values().map(|(a, _)| *a).collect();
    registered.sort();
    Ok(ShortcutsStatus {
        registered,
        failures: lock(&registry.failures).clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_defaults_parse_and_are_unique() {
        let d = default_bindings();
        assert_eq!(d.len(), 12);
        validate_bindings(&d).unwrap();
        for a in ShortcutAction::ALL {
            parse(a.default_key()).unwrap_or_else(|e| panic!("{a:?}: {e}"));
        }
    }

    #[test]
    fn parse_accepts_case_and_spaces() {
        let a = parse("Ctrl + Alt + L").unwrap();
        let b = parse("ctrl+alt+l").unwrap();
        assert_eq!(a.id(), b.id());
        assert_eq!(
            parse("ctrl+backslash").unwrap().id(),
            parse("Control+\\").unwrap().id()
        );
        assert_eq!(
            parse("ctrl+alt+up").unwrap().id(),
            parse("ctrl+alt+arrowup").unwrap().id()
        );
    }

    #[test]
    fn parse_rejects_bad_or_unsafe() {
        for bad in [
            "",
            "ctrl+",
            "ctrl+alt+nope",
            "a",
            "shift+a",
            "ctrl+a+b",
            "alt+ctrl",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} should be rejected");
        }
        assert!(parse("f9").is_ok());
        assert!(parse("shift+f12").is_ok());
        assert!(parse("super+k").is_ok());
    }

    #[test]
    fn validate_detects_duplicates_and_ignores_disabled() {
        let mut m = default_bindings();
        m.get_mut(&ShortcutAction::AskNow).unwrap().key = "ctrl+alt+s".into();
        assert!(validate_bindings(&m).unwrap_err().contains("both use"));
        m.get_mut(&ShortcutAction::AskNow).unwrap().enabled = false;
        validate_bindings(&m).unwrap();
        m.get_mut(&ShortcutAction::AskNow).unwrap().enabled = true;
        m.get_mut(&ShortcutAction::AskNow).unwrap().key = String::new();
        validate_bindings(&m).unwrap();
    }

    #[test]
    fn action_wire_names() {
        assert_eq!(
            serde_json::to_string(&ShortcutAction::ToggleClickThrough).unwrap(),
            "\"toggle_click_through\""
        );
        assert_eq!(
            serde_json::to_string(&ShortcutAction::MoveUp).unwrap(),
            "\"move_up\""
        );
        let a: ShortcutAction = serde_json::from_str("\"screenshot_region\"").unwrap();
        assert_eq!(a, ShortcutAction::ScreenshotRegion);
        assert_eq!(ShortcutAction::MoveLeft.move_dir(), Some((-1, 0)));
        assert_eq!(ShortcutAction::AskNow.move_dir(), None);
    }
}
