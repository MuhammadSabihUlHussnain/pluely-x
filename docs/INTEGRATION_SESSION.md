# Integrating the live session (CONTRACT §3) into `main`

The live-listening engine lives in three self-contained module trees. They depend on the rest
of the crate only through `crate::error::{AppError, AppResult}` (`AppError::new`,
`.with_detail`, public `code`/`message` fields) and `crate::secrets::{Provider, Secrets}`
(`Secrets::get(Provider::Deepgram)`), which main already provides with the same API.

## 1. Files

Copy these from the audio branch as-is:

```
src-tauri/src/audio/        mod.rs dump.rs frame.rs level.rs pump.rs resample.rs timeline.rs
src-tauri/src/audio/win/    mod.rs capture.rs com.rs control.rs devices.rs notify.rs
src-tauri/src/stt/          mod.rs echo.rs turns.rs
src-tauri/src/stt/deepgram/ mod.rs backoff.rs client.rs client_tests.rs config.rs mock.rs protocol.rs
src-tauri/src/stt/deepgram/fixtures/*.json   (golden Deepgram messages, used by tests)
src-tauri/src/session/      mod.rs hub.rs supervisor.rs supervisor_tests.rs types.rs
```

**Don't copy** `src-tauri/src/error.rs` or `src-tauri/src/secrets.rs` from the audio branch.
They are stand-ins (first line `// STUB — replaced at merge by main's version`); keep main's.

## 2. `src-tauri/src/lib.rs`

At the `// ---- live session` marker near the top:

```rust
mod audio;
mod stt;
mod session;
```

At the managed-state marker in `run()` (no `AppHandle` needed; this spawns the `audio-ctl`
MTA thread used for device listing and default-device notifications):

```rust
.manage(session::SessionManager::default())
```

In `tauri::generate_handler![...]`:

```rust
// §3 live session
session::audio_list_devices,
session::audio_open_mic_privacy_settings,
session::session_start,
session::session_stop,
session::session_set_source,
session::session_status,
session::session_reattach,
```

Exit hook. Main prevents exit when the last window closes (`code.is_none()`), so stop the
session only on a real quit, plus a best-effort stop on `Exit` (a no-op when nothing runs):

```rust
app.run(|app, event| match event {
    RunEvent::ExitRequested { api, code, .. } if code.is_none() => api.prevent_exit(),
    RunEvent::ExitRequested { .. } => session::stop_on_exit(app),
    RunEvent::Exit => {
        session::stop_on_exit(app);
        tracing::info!("exit");
    }
    _ => {}
});
```

(`_app` must become `app`. `stop_on_exit` blocks ≤ 4 s on the CloseStream flush and then
broadcasts `session://status {active:false}`.)

`session_start`/`session_stop` broadcast `session://status` themselves; nothing else to wire.
If main ever restricts app commands through an app manifest/capability, allow the seven
command names above.

## 3. `src-tauri/Cargo.toml`

Already in main (keep): `tokio-tungstenite = { version = "0.30", features = ["native-tls"] }`,
`thiserror = "2"`, `fastrand = "2"`, `uuid` (v4), `futures-util`, `tokio` (full), `tracing`,
`serde`/`serde_json`, `tauri-plugin-opener`, and `hound = "3.5.1"` under dev-dependencies.

Add to `[target.'cfg(windows)'.dependencies]`:

```toml
wasapi = "0.19.0"
windows-core = "0.61"
```

and add these features to the existing `windows = { version = "0.61", features = [...] }`
(same 0.61 as wasapi's, so the COM types are shared):

```toml
"Win32_Media_Audio",
"Win32_System_Com",
"Win32_System_Performance",
```

(`Win32_Foundation` is already there.)

Add to `[dev-dependencies]` (the KeepAlive test uses `tokio::time::pause()`):

```toml
tokio = { version = "1", features = ["full", "test-util"] }
```

## 4. Verify

```
cd src-tauri
set CARGO_BUILD_JOBS=4
cargo build
cargo test --lib                    # 96 tests, no hardware or keys needed (mock Deepgram on 127.0.0.1)
cargo test --lib hw_ -- --ignored --nocapture --test-threads=1   # real devices; plays two quiet 0.4 s tones
```

`generate_context!` needs `../dist` to exist at compile time (a placeholder `dist/index.html`
is enough when the frontend hasn't been built).

## 5. Behaviour notes for the frontend

- `session_start` fails only when the Deepgram key is missing (`key_missing`), a session is
  already running (`session_active`), or **every** enabled source failed to open (the first
  source's error: `mic_permission_denied`, `device_not_found`, `audio_init_failed`). A
  partial start succeeds; the failed source gets `error {source, fatal:true}` +
  `state {source, state:"error"}` and `SessionInfo` omits its `me`/`them`.
- `session_stop` with no session returns `no_session`. `session_set_source` likewise.
- `session_set_source`: `deviceId` absent = unchanged, `null` = follow the default, a string
  = that endpoint (falls back to the default with `device {reason:"fallback"}` if missing;
  switches back automatically when it reappears). `enabled:false` sends CloseStream, flushes,
  emits a `turn {reason:"stop"}` for pending finals and then `state {source, "idle"}`.
  `muted:true` sends Finalize and stops sending audio (meters keep running).
- Debug: set `PLUELYX_DEBUG_DUMP=1` to write `pluelyx-<sessionId>-<me|them>.wav` (the exact
  16 kHz PCM sent to Deepgram) into the app log dir.
