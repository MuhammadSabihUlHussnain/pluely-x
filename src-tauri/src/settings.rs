//! Rust-owned app settings, persisted as `<appConfig>/settings.json`.
//!
//! Rust owns them so hotkeys, window flags and content protection are applied before the
//! webview loads. Updates are JSON merge-patches (RFC 7396: `null` resets a field to its
//! default), validated as a whole, saved atomically (tmp + rename), then applied and broadcast
//! as `settings://changed`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tauri::{AppHandle, Emitter, Manager, Runtime};

use crate::error::{AppError, AppResult};
use crate::shortcuts::{self, ShortcutAction};

/// 2: the end-of-sentence pause (`stt.endpointingMs`) default went from 300 ms to 700 ms.
pub const SETTINGS_VERSION: u32 = 2;
/// Default `stt.endpointingMs`. 300 ms split questions spoken with natural pauses into
/// pieces, so answers kept starting on half a question and being cancelled.
pub const DEFAULT_ENDPOINTING_MS: u32 = 700;
const OLD_DEFAULT_ENDPOINTING_MS: u64 = 300;
pub const EVENT_CHANGED: &str = "settings://changed";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Meeting,
    Interview,
    Coding,
    Lecture,
    Ask,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AecMode {
    Auto,
    On,
    Off,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OverlaySettings {
    pub width: f64,
    pub height: f64,
    pub opacity: f64,
    pub click_through: bool,
    pub content_protection: bool,
    pub always_on_top: bool,
    pub launch_at_startup: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSettings {
    pub mic_device_id: Option<String>,
    pub output_device_id: Option<String>,
    pub aec: AecMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SttSettings {
    pub language: String,
    pub endpointing_ms: u32,
    pub utterance_end_ms: u32,
    pub diarize_them: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AiSettings {
    pub model: String,
    pub live_effort: Effort,
    pub deep_effort: Effort,
    pub max_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShortcutBinding {
    pub key: String,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub version: u32,
    pub onboarded: bool,
    pub mode: Mode,
    pub overlay: OverlaySettings,
    pub audio: AudioSettings,
    pub stt: SttSettings,
    pub ai: AiSettings,
    pub shortcuts: BTreeMap<ShortcutAction, ShortcutBinding>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            version: SETTINGS_VERSION,
            onboarded: false,
            mode: Mode::Ask,
            overlay: OverlaySettings {
                width: 440.0,
                height: 560.0,
                opacity: 1.0,
                click_through: false,
                content_protection: true,
                always_on_top: true,
                launch_at_startup: false,
            },
            audio: AudioSettings {
                mic_device_id: None,
                output_device_id: None,
                aec: AecMode::Auto,
            },
            stt: SttSettings {
                language: "en".into(),
                endpointing_ms: DEFAULT_ENDPOINTING_MS,
                utterance_end_ms: 1000,
                diarize_them: false,
            },
            ai: AiSettings {
                model: "claude-opus-5-5".into(),
                live_effort: Effort::Low,
                deep_effort: Effort::Medium,
                max_tokens: 16_000,
            },
            shortcuts: shortcuts::default_bindings(),
        }
    }
}

/// Valid ranges (also used by the window module for clamping).
pub const MIN_OVERLAY_WIDTH: f64 = 200.0;
pub const MIN_OVERLAY_HEIGHT: f64 = 40.0;
pub const MAX_OVERLAY_EDGE: f64 = 8192.0;

// ---------------------------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------------------------

/// RFC 7396 JSON merge-patch.
pub fn merge_patch(target: &mut Value, patch: &Value) {
    match patch {
        Value::Object(p) => {
            if !target.is_object() {
                *target = Value::Object(Map::new());
            }
            let t = target.as_object_mut().expect("object");
            for (k, v) in p {
                if v.is_null() {
                    t.remove(k);
                } else {
                    merge_patch(t.entry(k.clone()).or_insert(Value::Null), v);
                }
            }
        }
        other => *target = other.clone(),
    }
}

/// Fill keys missing from `value` with `defaults` (recursively for objects).
pub fn fill_defaults(value: &mut Value, defaults: &Value) {
    if let (Value::Object(v), Value::Object(d)) = (value, defaults) {
        for (k, dv) in d {
            match v.get_mut(k) {
                None => {
                    v.insert(k.clone(), dv.clone());
                }
                Some(existing) => fill_defaults(existing, dv),
            }
        }
    }
}

/// Drop shortcut entries for actions this build doesn't know (e.g. written by a newer build).
fn drop_unknown_shortcuts(value: &mut Value) {
    if let Some(Value::Object(map)) = value.get_mut("shortcuts") {
        map.retain(|k, _| {
            serde_json::from_value::<ShortcutAction>(Value::String(k.clone())).is_ok()
        });
    }
}

/// Parse a (possibly partial) settings JSON value: defaults fill gaps, then validate.
pub fn from_value(mut value: Value) -> AppResult<AppSettings> {
    let defaults = serde_json::to_value(AppSettings::default())?;
    drop_unknown_shortcuts(&mut value);
    fill_defaults(&mut value, &defaults);
    // version is owned by Rust
    if let Some(obj) = value.as_object_mut() {
        obj.insert("version".into(), Value::from(SETTINGS_VERSION));
    }
    let s: AppSettings = serde_json::from_value(value)
        .map_err(|e| AppError::new("invalid_settings", format!("Invalid settings: {e}")))?;
    validate(&s)?;
    Ok(s)
}

/// Apply a merge-patch to `current` and return the validated result (current is untouched).
pub fn apply_patch(current: &AppSettings, patch: &Value) -> AppResult<AppSettings> {
    if !patch.is_object() {
        return Err(AppError::new(
            "invalid_settings",
            "Settings patch must be an object",
        ));
    }
    let mut v = serde_json::to_value(current)?;
    merge_patch(&mut v, patch);
    from_value(v)
}

fn invalid(msg: impl Into<String>) -> AppError {
    AppError::new("invalid_settings", msg)
}

pub fn validate(s: &AppSettings) -> AppResult<()> {
    let o = &s.overlay;
    let edge_ok = |v: f64, min: f64| v.is_finite() && (min..=MAX_OVERLAY_EDGE).contains(&v);
    if !edge_ok(o.width, MIN_OVERLAY_WIDTH) {
        return Err(invalid(format!(
            "overlay.width must be {MIN_OVERLAY_WIDTH}-{MAX_OVERLAY_EDGE}"
        )));
    }
    if !edge_ok(o.height, MIN_OVERLAY_HEIGHT) {
        return Err(invalid(format!(
            "overlay.height must be {MIN_OVERLAY_HEIGHT}-{MAX_OVERLAY_EDGE}"
        )));
    }
    if !(o.opacity.is_finite() && (0.3..=1.0).contains(&o.opacity)) {
        return Err(invalid("overlay.opacity must be between 0.3 and 1"));
    }
    let lang = s.stt.language.trim();
    if lang.is_empty()
        || lang.len() > 16
        || !lang.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(invalid(
            "stt.language must be a language code like \"en\" or \"en-US\"",
        ));
    }
    if !(10..=5_000).contains(&s.stt.endpointing_ms) {
        return Err(invalid("stt.endpointingMs must be 10-5000"));
    }
    if !(1_000..=5_000).contains(&s.stt.utterance_end_ms) {
        return Err(invalid("stt.utteranceEndMs must be 1000-5000"));
    }
    if s.ai.model.trim().is_empty() || s.ai.model.len() > 128 {
        return Err(invalid("ai.model must be a model id"));
    }
    if !(256..=128_000).contains(&s.ai.max_tokens) {
        return Err(invalid("ai.maxTokens must be 256-128000"));
    }
    for id in [&s.audio.mic_device_id, &s.audio.output_device_id]
        .into_iter()
        .flatten()
    {
        if id.len() > 512 {
            return Err(invalid("audio device id too long"));
        }
    }
    shortcuts::validate_bindings(&s.shortcuts).map_err(invalid)?;
    Ok(())
}

/// Write `settings` to `path` atomically (tmp file in the same dir, fsync, rename).
pub fn save_atomic(path: &Path, settings: &AppSettings) -> AppResult<()> {
    use std::io::Write;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(settings)?;
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(&json)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    Loaded,
    /// Loaded from an older version and upgraded (the file is rewritten).
    Migrated,
    Created,
    /// The file was unreadable or invalid; it was moved aside and defaults were used.
    Recovered,
}

pub fn load_or_default(path: &Path) -> (AppSettings, LoadOutcome) {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (AppSettings::default(), LoadOutcome::Created)
        }
        Err(e) => {
            tracing::warn!(error = %e, "settings unreadable; using defaults");
            return (AppSettings::default(), LoadOutcome::Recovered);
        }
    };
    let mut migrated = false;
    let parsed = serde_json::from_slice::<Value>(&bytes)
        .map_err(AppError::from)
        .and_then(|mut v| {
            migrated = migrate(&mut v);
            from_value(v)
        });
    match parsed {
        Ok(s) if migrated => (s, LoadOutcome::Migrated),
        Ok(s) => (s, LoadOutcome::Loaded),
        Err(e) => {
            let ts = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0);
            let bad = path.with_extension(format!("json.bad-{ts}"));
            let _ = std::fs::rename(path, &bad);
            tracing::warn!(error = %e, backup = %bad.display(), "settings invalid; using defaults");
            (AppSettings::default(), LoadOutcome::Recovered)
        }
    }
}

/// Upgrades a settings file written by an older version in place. Returns true when it did.
///
/// v1 → v2: a file still on the old 300 ms end-of-sentence default gets the new 700 ms
/// default; a value the user picked is kept.
fn migrate(value: &mut Value) -> bool {
    let Some(obj) = value.as_object_mut() else {
        return false;
    };
    let version = obj.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version >= u64::from(SETTINGS_VERSION) {
        return false;
    }
    if let Some(stt) = obj.get_mut("stt").and_then(Value::as_object_mut) {
        if stt.get("endpointingMs").and_then(Value::as_u64) == Some(OLD_DEFAULT_ENDPOINTING_MS) {
            stt.insert("endpointingMs".into(), Value::from(DEFAULT_ENDPOINTING_MS));
        }
    }
    true
}

/// Reset one top-level section (or everything when `section` is None).
pub fn reset_section(current: &AppSettings, section: Option<&str>) -> AppResult<AppSettings> {
    let d = AppSettings::default();
    let mut s = current.clone();
    match section {
        None => s = d,
        Some("version") => {}
        Some("onboarded") => s.onboarded = d.onboarded,
        Some("mode") => s.mode = d.mode,
        Some("overlay") => s.overlay = d.overlay,
        Some("audio") => s.audio = d.audio,
        Some("stt") => s.stt = d.stt,
        Some("ai") => s.ai = d.ai,
        Some("shortcuts") => s.shortcuts = d.shortcuts,
        Some(other) => {
            return Err(AppError::invalid_argument(format!(
                "Unknown settings section \"{other}\""
            )))
        }
    }
    validate(&s)?;
    Ok(s)
}

// ---------------------------------------------------------------------------------------------
// Managed state + side effects
// ---------------------------------------------------------------------------------------------

pub struct SettingsState {
    path: PathBuf,
    current: Mutex<AppSettings>,
}

impl SettingsState {
    pub fn load(path: PathBuf) -> Self {
        let (settings, outcome) = load_or_default(&path);
        if outcome != LoadOutcome::Loaded {
            if let Err(e) = save_atomic(&path, &settings) {
                tracing::error!(error = %e, "couldn't write settings file");
            }
        }
        tracing::info!(?outcome, path = %path.display(), "settings ready");
        Self {
            path,
            current: Mutex::new(settings),
        }
    }

    pub fn get(&self) -> AppSettings {
        self.current
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Validate + save + swap. Returns (old, new). Does not apply side effects.
    fn replace(&self, new: AppSettings) -> AppResult<(AppSettings, AppSettings)> {
        validate(&new)?;
        let mut cur = self.current.lock().unwrap_or_else(|e| e.into_inner());
        if *cur == new {
            return Ok((cur.clone(), new));
        }
        save_atomic(&self.path, &new)?;
        let old = std::mem::replace(&mut *cur, new.clone());
        Ok((old, new))
    }
}

/// Persist `new`, apply side effects and broadcast. The single write path for settings.
pub fn commit<R: Runtime>(app: &AppHandle<R>, new: AppSettings) -> AppResult<AppSettings> {
    let state = app.state::<SettingsState>();
    let (old, new) = state.replace(new)?;
    if old != new {
        apply_side_effects(app, Some(&old), &new);
        let _ = app.emit(EVENT_CHANGED, &new);
    }
    Ok(new)
}

/// Read-modify-write helper for native toggles (tray, hotkeys, window commands).
pub fn update_with<R: Runtime>(
    app: &AppHandle<R>,
    f: impl FnOnce(&mut AppSettings),
) -> AppResult<AppSettings> {
    let mut s = app.state::<SettingsState>().get();
    f(&mut s);
    commit(app, s)
}

/// Apply settings to the running app. `old == None` means startup (apply everything).
pub fn apply_side_effects<R: Runtime>(
    app: &AppHandle<R>,
    old: Option<&AppSettings>,
    new: &AppSettings,
) {
    let changed = |f: &dyn Fn(&AppSettings) -> bool| old.map(|o| f(o) != f(new)).unwrap_or(true);

    if old.map(|o| o.shortcuts != new.shortcuts).unwrap_or(true) {
        shortcuts::apply(app, &new.shortcuts);
    }
    let overlay_changed = changed(&|s| s.overlay.click_through)
        || changed(&|s| s.overlay.content_protection)
        || changed(&|s| s.overlay.always_on_top);
    if overlay_changed {
        crate::window::apply_overlay_flags(app, &new.overlay);
    }
    if changed(&|s| s.overlay.launch_at_startup) {
        apply_autostart(app, new.overlay.launch_at_startup);
    }
    if old.is_some() && overlay_changed {
        crate::window::broadcast_state(app);
        crate::tray::refresh(app);
    }
}

fn apply_autostart<R: Runtime>(app: &AppHandle<R>, enabled: bool) {
    use tauri_plugin_autostart::ManagerExt;
    let al = app.autolaunch();
    let current = al.is_enabled().unwrap_or(false);
    if current == enabled {
        return;
    }
    let res = if enabled { al.enable() } else { al.disable() };
    match res {
        Ok(()) => tracing::info!(enabled, "autostart updated"),
        Err(e) => {
            tracing::warn!(error = %e, "autostart update failed");
            crate::emit_app_error(app, "autostart_failed", "Couldn't change launch-at-startup");
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------------------------

#[tauri::command]
pub async fn settings_get(state: tauri::State<'_, SettingsState>) -> AppResult<AppSettings> {
    Ok(state.get())
}

#[tauri::command]
pub async fn settings_update(app: AppHandle, patch: Value) -> AppResult<AppSettings> {
    let current = app.state::<SettingsState>().get();
    let new = apply_patch(&current, &patch)?;
    commit(&app, new)
}

#[tauri::command]
pub async fn settings_reset(app: AppHandle, section: Option<String>) -> AppResult<AppSettings> {
    let current = app.state::<SettingsState>().get();
    let new = reset_section(&current, section.as_deref())?;
    commit(&app, new)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn defaults_match_contract() {
        let v = serde_json::to_value(AppSettings::default()).unwrap();
        assert_eq!(v["version"], 2);
        assert_eq!(v["onboarded"], false);
        assert_eq!(v["overlay"]["contentProtection"], true);
        assert_eq!(v["overlay"]["alwaysOnTop"], true);
        assert_eq!(v["overlay"]["launchAtStartup"], false);
        assert_eq!(v["overlay"]["clickThrough"], false);
        assert_eq!(v["overlay"]["width"], 440.0);
        assert_eq!(v["overlay"]["height"], 560.0);
        assert_eq!(v["audio"]["micDeviceId"], Value::Null);
        assert_eq!(v["audio"]["aec"], "auto");
        assert_eq!(v["stt"]["language"], "en");
        assert_eq!(v["stt"]["endpointingMs"], 700);
        assert_eq!(v["stt"]["utteranceEndMs"], 1000);
        assert_eq!(v["ai"]["model"], "claude-opus-5-5");
        assert_eq!(v["ai"]["liveEffort"], "low");
        assert_eq!(v["ai"]["deepEffort"], "medium");
        assert_eq!(v["ai"]["maxTokens"], 16000);
        assert_eq!(v["shortcuts"]["toggle_overlay"]["key"], "ctrl+backslash");
        assert_eq!(v["shortcuts"]["move_left"]["key"], "ctrl+alt+arrowleft");
        assert_eq!(v["shortcuts"].as_object().unwrap().len(), 12);
        validate(&AppSettings::default()).unwrap();
        // round trip
        let back: AppSettings = serde_json::from_value(v).unwrap();
        assert_eq!(back, AppSettings::default());
    }

    #[test]
    fn merge_patch_rfc7396() {
        let mut t = json!({"a": {"b": 1, "c": 2}, "d": [1, 2], "e": "x"});
        merge_patch(
            &mut t,
            &json!({"a": {"b": 5, "c": null}, "d": [3], "f": {"g": true}}),
        );
        assert_eq!(
            t,
            json!({"a": {"b": 5}, "d": [3], "e": "x", "f": {"g": true}})
        );
    }

    #[test]
    fn patch_updates_nested_fields_only() {
        let cur = AppSettings::default();
        let new = apply_patch(
            &cur,
            &json!({"overlay": {"opacity": 0.8}, "mode": "coding"}),
        )
        .unwrap();
        assert_eq!(new.overlay.opacity, 0.8);
        assert_eq!(new.mode, Mode::Coding);
        assert_eq!(new.overlay.width, cur.overlay.width);
        assert_eq!(new.shortcuts, cur.shortcuts);
    }

    #[test]
    fn null_resets_to_default_and_sets_nullable() {
        let mut cur = AppSettings::default();
        cur.audio.mic_device_id = Some("{0.0.1.00000000}.{abc}".into());
        cur.overlay.width = 600.0;
        let new = apply_patch(
            &cur,
            &json!({"audio": {"micDeviceId": null}, "overlay": {"width": null}}),
        )
        .unwrap();
        assert_eq!(new.audio.mic_device_id, None);
        assert_eq!(new.overlay.width, 440.0);
    }

    #[test]
    fn patch_validation_rejects_bad_values() {
        let cur = AppSettings::default();
        for bad in [
            json!({"overlay": {"opacity": 0.1}}),
            json!({"overlay": {"width": 10}}),
            json!({"stt": {"utteranceEndMs": 500}}),
            json!({"stt": {"language": ""}}),
            json!({"ai": {"maxTokens": 10}}),
            json!({"ai": {"liveEffort": "max"}}),
            json!({"mode": "party"}),
            json!({"shortcuts": {"ask_now": {"key": "ctrl+alt+nope", "enabled": true}}}),
            json!({"shortcuts": {"ask_now": {"key": "ctrl+alt+l", "enabled": true}}}), // dup of toggle_listen
            json!("not an object"),
        ] {
            let err = apply_patch(&cur, &bad).unwrap_err();
            assert_eq!(err.code, "invalid_settings", "{bad}");
        }
        // a disabled duplicate is fine
        apply_patch(
            &cur,
            &json!({"shortcuts": {"ask_now": {"key": "ctrl+alt+l", "enabled": false}}}),
        )
        .unwrap();
    }

    #[test]
    fn version_is_rust_owned() {
        let new = apply_patch(&AppSettings::default(), &json!({"version": 99})).unwrap();
        assert_eq!(new.version, SETTINGS_VERSION);
    }

    #[test]
    fn partial_file_gets_defaults_and_unknown_actions_dropped() {
        let s = from_value(json!({"onboarded": true, "overlay": {"opacity": 0.5},
            "shortcuts": {"future_action": {"key": "ctrl+alt+f", "enabled": true}}}))
        .unwrap();
        assert!(s.onboarded);
        assert_eq!(s.overlay.opacity, 0.5);
        assert!(s.overlay.content_protection);
        assert_eq!(s.shortcuts.len(), 12);
    }

    #[test]
    fn atomic_save_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cfg").join("settings.json");
        let (s, outcome) = load_or_default(&path);
        assert_eq!(outcome, LoadOutcome::Created);
        let mut s2 = s.clone();
        s2.onboarded = true;
        save_atomic(&path, &s2).unwrap();
        save_atomic(&path, &s2).unwrap(); // overwrite existing works on Windows
        assert!(!path.with_extension("json.tmp").exists());
        let (loaded, outcome) = load_or_default(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(loaded, s2);
    }

    #[test]
    fn v1_file_on_the_old_default_pause_is_upgraded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let mut v1 = serde_json::to_value(AppSettings::default()).unwrap();
        v1["version"] = json!(1);
        v1["onboarded"] = json!(true);
        v1["stt"]["endpointingMs"] = json!(300);
        std::fs::write(&path, serde_json::to_vec(&v1).unwrap()).unwrap();
        let (s, outcome) = load_or_default(&path);
        assert_eq!(outcome, LoadOutcome::Migrated);
        assert_eq!(s.version, 2);
        assert_eq!(s.stt.endpointing_ms, 700);
        assert!(s.onboarded, "everything else is kept");

        // A pause the user chose is kept; a v2 file is never touched again.
        v1["stt"]["endpointingMs"] = json!(450);
        std::fs::write(&path, serde_json::to_vec(&v1).unwrap()).unwrap();
        assert_eq!(load_or_default(&path).0.stt.endpointing_ms, 450);
        let mut v2 = serde_json::to_value(AppSettings::default()).unwrap();
        v2["stt"]["endpointingMs"] = json!(300);
        std::fs::write(&path, serde_json::to_vec(&v2).unwrap()).unwrap();
        let (s, outcome) = load_or_default(&path);
        assert_eq!(outcome, LoadOutcome::Loaded);
        assert_eq!(s.stt.endpointing_ms, 300);
    }

    #[test]
    fn corrupt_file_is_moved_aside() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{ not json").unwrap();
        let (s, outcome) = load_or_default(&path);
        assert_eq!(outcome, LoadOutcome::Recovered);
        assert_eq!(s, AppSettings::default());
        assert!(!path.exists());
        let backups: Vec<_> = std::fs::read_dir(dir.path()).unwrap().collect();
        assert_eq!(backups.len(), 1);
    }

    #[test]
    fn reset_one_section() {
        let mut cur = AppSettings::default();
        cur.onboarded = true;
        cur.overlay.opacity = 0.5;
        cur.ai.max_tokens = 4000;
        let r = reset_section(&cur, Some("overlay")).unwrap();
        assert_eq!(r.overlay.opacity, 1.0);
        assert_eq!(r.ai.max_tokens, 4000);
        assert!(r.onboarded);
        let all = reset_section(&cur, None).unwrap();
        assert_eq!(all, AppSettings::default());
        assert!(reset_section(&cur, Some("nope")).is_err());
    }
}
