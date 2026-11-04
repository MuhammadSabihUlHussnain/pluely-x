# Native layer design notes (Windows 11)

These findings were checked against tauri 2.8.2, tao 0.34.2, wasapi 0.19.0, xcap 0.0.12 and image 0.25.6 sources in `~/.cargo/registry`. **(unverified)** marks claims about crates that weren't available locally (tokio-tungstenite, keyring, tauri-plugin-single-instance, Deepgram protocol). The API surface lives in `docs/CONTRACT.md`; the product plan lives in `docs/PLAN.md`.

## Pitfalls in the existing code and crates

- `speaker/windows.rs:201` `.expect` panics when the device is missing.
- `:248-251` the capture thread exits after a 3 s event timeout, so capture dies during silence.
- `:253-273` reads one packet per event and copies bytes one at a time through `VecDeque<u8>`; the QPC timestamp is dropped.
- `:146-156`: if init fails, it pretends the rate is 44100 and the stream never produces anything.
- **No COM init anywhere.** The capture thread only works because it lands in the implicit MTA. The sync device-list commands run on the main (STA) thread.
- **wasapi 0.19 bugs:**
  - `read_from_device_to_deque` calls `ReleaseBuffer(..).unwrap()` (api.rs:1517); use `read_from_device` with a preallocated buffer.
  - `set_echo_cancellation_render_endpoint(Some(id))` builds a PCWSTR from a dropped temporary HSTRING (api.rs:1594-1595, use-after-free); pass `None`.
  - `Handle` has no Drop, so each restart leaks an event handle (api.rs:1531); close it yourself.
  - There is no IMMNotificationClient; write it with `windows-core` `#[implement]`.
  - Process loopback has no `get_mixformat`/`get_device_period`/session control (api.rs:562-576).
- **tao rewrites the whole `GWL_EXSTYLE`** on every flag change (show, hide, set_focusable, set_ignore_cursor_events, set_always_on_top) and always re-adds `WS_EX_APPWINDOW` (`window_state.rs:426-441`, `window.rs:1164`, `window_state.rs:258-259`). A one-time `WS_EX_TOOLWINDOW` patch doesn't stick.
- **`skip_taskbar`** only removes the taskbar button (`ITaskbarList::DeleteTab`, window.rs:1529-1533). The window still appears in Alt+Tab.
- **xcap 0.0.12:**
  - Monitor x/y/width/height are physical; `name()` is `szDevice`.
  - tauri `Monitor::name()` is also `szDevice` (tao monitor.rs:181-184), so pair monitors by **name**, not by index.
  - Don't trust xcap `scale_factor()`; use tauri's.
- `capture.rs:209` uses an eager `ok_or({...})` that clears `overlay_active` on every call (fix: `ok_or_else`). `:193` has an emit `.unwrap()`.
- **TLS:** the lockfile already has ring 0.17 and native-tls (via reqwest/hyper-tls). Don't pull in aws-lc-rs: it needs CMake/NASM on Windows.
- **Anthropic API:** use `anthropic-version: 2026-10-21`. That version rejects temperature/top_p/top_k. Current Opus/Sonnet accept images up to a 2576 px long edge; 1568 is a cost/latency choice.

## Module layout (`src-tauri/src`)

```
main.rs                 unchanged (calls lib run())
lib.rs                  Builder: tauri-plugin-single-instance FIRST, logging, managed state, invoke_handler, setup, RunEvent::ExitRequested -> session stop
error.rs                AppError {code,message,detail} (thiserror + Serialize) + From impls
logging.rs              tracing-subscriber + tracing-appender (daily, keep 7) in app_log_dir; tracing-log LogTracer; tungstenite=info; `log_write` command
settings.rs             AppSettings (versioned, Rust-owned so hotkeys/window/protection apply before webview loads); atomic write tmp+rename; merge-patch + validate; emits settings://changed
secrets.rs              trait SecretStore {get,set,delete}; KeyringStore (keyring v3, features=["windows-native"] - WITHOUT it keyring silently uses a mock store!) + MemStore (tests); in-memory cache
audio/  mod.rs, frame.rs[pure], level.rs[pure], timeline.rs[pure], resample.rs[pure], win/{com,devices,notify,control,capture}.rs
stt/    deepgram/{config,protocol,backoff}.rs[pure], deepgram/client.rs, turns.rs[pure], echo.rs[pure]
session/ types.rs, mod.rs (SessionManager, single slot, commands), supervisor.rs
screen/ geometry.rs[pure], encode.rs[pure], capture.rs, store.rs, mod.rs
window/ mod.rs, win32.rs (subclass), placement.rs[pure], dashboard.rs
tray.rs, shortcuts.rs, llm/anthropic.rs, db/ (tauri-plugin-sql migrations)
```

**Delete:**
- `api.rs`, `activate.rs`, `speaker/`.
- The env injection in `build.rs` (keep `tauri_build::build()`).
- The updater config and `createUpdaterArtifacts`.
- The updater/keychain/posthog/http capability entries.
- Crates: tauri-plugin-updater, -keychain, -posthog, -machine-uid, -shell (use `opener` for `ms-settings:` links), -http (all network calls in Rust), cpal, ringbuf, once_cell, dotenv, and the macOS/Linux deps. Keep `hound` as a dev-dependency (debug WAV dump).

**Add:**
- tokio-tungstenite (native-tls feature)
- keyring 3 (windows-native)
- `windows`/`windows-core` **0.61** (match wasapi to avoid duplicates). Features: Win32_Media_Audio, Win32_System_Com, Win32_UI_WindowsAndMessaging, Win32_UI_Shell, Win32_System_Performance, Win32_Foundation, Win32_Graphics_Gdi as needed.
- thiserror, tracing-subscriber, tracing-appender, tracing-log, tauri-plugin-single-instance, fastrand
- tauri features `["tray-icon","image-png"]`
- `[profile.dev.package."*"] opt-level = 2`
- Replace `"csp": null` with a real CSP.

## Audio pipeline

### Threads

| Thread | Owns |
|---|---|
| main (tao) | windows, subclass, tray, hotkeys |
| tokio | commands, supervisor, two Deepgram tasks, 10 Hz level ticker |
| `audio-ctl` (std, MTA) | IMMDeviceEnumerator + notification client + device listing |
| `cap-them`, `cap-me` (std, MTA) | that source's WASAPI objects |

COM objects never cross threads. Commands that do I/O must be `async` (sync commands run on the main thread).

### COM and format

- **COM:** `ComGuard` = `wasapi::initialize_mta()` (S_OK/S_FALSE both fine), `deinitialize()` in Drop.
- **Format:** request **16 kHz mono f32**, shared, event-driven, `autoconvert: true` (sets AUTOCONVERTPCM|SRC_DEFAULT_QUALITY, api.rs:950-956). Convert to i16 yourself.
- **Buffer:** `buffer_duration_hns = 2_000_000` (200 ms; event mode drains every ~10 ms, so no added latency).
- **Fallback:** on `AUDCLNT_E_UNSUPPORTED_FORMAT`, use the mix format + `resample.rs`.

### Mic init order

1. Resolve the device → `get_iaudioclient`.
2. If AEC isn't off, call `set_audio_stream_category(Communications)` **before** `initialize_client` (examples/aec.rs:31-37). Never on loopback.
3. `initialize_client`.
4. If AEC is wanted and `is_aec_supported()`: `get_aec_control().set_echo_cancellation_render_endpoint(None)`.
5. `set_get_eventhandle`, `get_audiocaptureclient`.
6. `register_session_notification` with a disconnected flag.
7. `start_stream`.

**Loopback:** the same flow on the Render device (the loopback flag is added automatically, api.rs:938-941).

### Drain loop

```
loop {
  if stop { break Stopped }
  match evt.wait_for_event(100) { Err(timeout) => { idle_tick(); continue } _ => {} }   // never exit on silence
  loop { n = get_next_packet_size()?.unwrap_or(0); if n==0 {break}
         (frames, info) = read_from_device(&mut buf)?;  if info.flags.silent { zero } ; if discontinuity { stats.disc+=1 }
         t_ms = clock.qpc_to_session_ms(info.timestamp);
         asm.push(samples, t_ms, |frame| { level.update(&frame); if tx.try_send(frame).is_err() { stats.dropped_ms += 50 } }) }
}
```

- **Exit::DeviceLost** on `AUDCLNT_E_DEVICE_INVALIDATED` or the session-disconnected flag.
- **Exit::PermissionDenied** on `E_ACCESSDENIED` (mic privacy off; the UI links to `ms-settings:privacy-microphone`).

### Silence, devices, frames, levels

- **Silence:** tolerant timeouts, no keep-alive render stream. **Tail-fill** sends up to 1500 ms of synthetic zeros (100 ms chunks, session-timestamped) after the last real packet, so Deepgram endpointing/UtteranceEnd fire when a source goes quiet mid-sentence. After that nothing is sent, and the socket's KeepAlive holds the connection.
- **Devices:**
  - The notify callbacks only `send()` (never block or re-enter COM). The supervisor debounces 400 ms; a source that follows "default" and whose default changed gets its capture restarted.
  - `DeviceLost` restarts with backoff (100 ms → 500 ms → 1 s → 2 s → 5 s cap). A missing chosen device falls back to the default and emits `device{reason:"fallback"}`.
  - Default roles: loopback = Console; mic = the user's choice, else Communications.
  - The UI shows the captured endpoint name and a meter (Bluetooth hands-free switching).
- **Frames:**
  - 50 ms frames (800 samples, i16 LE). Each source has a `tokio::mpsc::channel(64)`; the capture thread uses `try_send` and never blocks.
  - While reconnecting, the Deepgram task keeps a 90 s backlog (drop oldest, count droppedMs) and replays it on reconnect.
  - Each sent frame records (conn_sample_offset → t_ms) in that connection's Timeline.
- **Levels:** a peak-hold AtomicU32 per source. A 100 ms tokio interval swaps both values and emits one `levels` event; silence decays to the floor.

### Echo

- **Setting:** `aec: auto|on|off`, default auto. Windows AEC is often unsupported (`is_aec_supported()` returns false) and the Communications category may cause ducking. Headphones are recommended in onboarding.
- **Always-on `stt/echo.rs`:**
  - For each "me" final, compare it with "them" finals overlapping [t0-1500, t1+1500].
  - Score = LCS(normalized tokens) / me-token count. Echo when ≥ 0.6 with ≥ 3 tokens, or an exact match for 1-2 tokens.
  - Emit the "me" final immediately, then send `transcriptPatch{segId, echo:true}` once the matching "them" final arrives.

## Deepgram client

- **Connection:** tokio-tungstenite, `connect_async_tls_with_config(req, cfg, disable_nagle=true, None)`, native-tls (SChannel, Windows cert store). No HTTP proxy support (phase 2: manual CONNECT).
- **Auth:** `req.headers_mut().insert("Authorization", "Token <key>")`. Log `dg-request-id`; never log headers.
- **One socket per source** (separate clocks; independent reconnects; diarize "them" only).
- **URL:**
  ```
  wss://api.deepgram.com/v1/listen?model=nova-3&language=en&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true&punctuate=true&endpointing=300&utterance_end_ms=1000&vad_events=true&tag=pluelyx-{me|them}[&diarize=true][&keyterm=T1&keyterm=T2]
  ```
  `utterance_end_ms` needs interim_results and is ≥ 1000. `keyterm` is nova-3 only.
- **Loop:** a biased `select!` over:
  - (a) a frame → Binary + timeline;
  - (b) a server message → parse/emit;
  - (c) a 1 s tick:
    - KeepAlive: send the TEXT frame `{"type":"KeepAlive"}` after ≥ 4 s without audio (Deepgram closes after ~10 s).
    - Watchdog: if ≥ 10 s of audio has been sent since the last server message, treat the socket as half-open and reconnect.
  - (d) control:
    - Mute → `{"type":"Finalize"}`.
    - Stop → `{"type":"CloseStream"}`, then keep reading Results/Metadata until close or 3 s.
- **Reconnect:** full-jitter exponential backoff (250 ms base, ×2, 10 s cap; reset after 30 s healthy).
  - **Fatal:** handshake 401/403 (dg_auth), 402 (dg_payment), 400 (dg_bad_request).
  - **Retry:** 429 (≥ 5 s), 5xx, network errors, close 1006/1011, `NET-0001`.
  - New connection ⇒ `conn += 1`; Deepgram timestamps restart at 0, so the Timeline is per connection.
- **Server messages:** `Results{channel_index,start,duration,is_final,speech_final,from_finalize,channel.alternatives[0]{transcript,confidence,words[{word,punctuated_word,start,end,confidence,speaker?}]}}`, `UtteranceEnd{channel,last_word_end}`, `SpeechStarted{channel,timestamp}`, `Metadata`. `#[serde(other)] Unknown`. Interims share `start` until final, so `segId = "{source}-{conn}-{round(start*1000)}"`.
- **turns.rs:** gather is_final segments. Emit one `turn` on speech_final, or on UtteranceEnd if unemitted finals remain (dedupes speech_final followed by UtteranceEnd). `question` = the text ends with `?`.

## Window behaviour

### Creation

- Build in setup (not tauri.conf.json): `.focusable(false).focused(false).visible(false).skip_taskbar(true).always_on_top(true).content_protected(true).transparent(true).decorations(false).shadow(false).resizable(true).min_inner_size(..)`.
- Install the subclass **on the main thread** (`run_on_main_thread`), restore placement, `show()`.

### Subclass (win32.rs)

- Installed with `SetWindowSubclass` using your own id. It runs before tao's.
- On `WM_STYLECHANGING` with wParam == GWL_EXSTYLE, edit `STYLESTRUCT.styleNew`: add WS_EX_TOOLWINDOW, clear WS_EX_APPWINDOW. Then call SetWindowLongPtrW once to apply.
- Return `MA_NOACTIVATE` from `WM_MOUSEACTIVATE` while not typing.

### Typing mode

- **Begin:** save `prev = GetForegroundWindow()`, `set_focusable(true)`, `set_focus()`, emit `window://typing {active:true}`.
- **End** (Esc, submit, or Focused(false)): `set_focusable(false)`; `SetForegroundWindow(prev)` if `IsWindow(prev)` and restore was asked for.
- **Caveat:** tao's `force_window_active` may send a synthetic Alt key.

### Other behaviour

- **Riskiest unknown, spike first:** do clicks into WebView2 child HWNDs still activate a NOACTIVATE top-level window? Test with a Notepad caret and log `GetForegroundWindow()`.
- **Hide/show:** only through tauri `hide()`/`show()`, never a raw ShowWindow (tao's VISIBLE flag goes stale, and the next apply_diff then hides it).
- **Topmost:** `ensure_topmost` = raw `SetWindowPos(HWND_TOPMOST, SWP_NOMOVE|SWP_NOSIZE|SWP_NOACTIVATE)` on show, on mouse-enter, and every 3 s while visible.
- **Click-through:** `set_ignore_cursor_events` (WS_EX_TRANSPARENT|LAYERED). The off switch is a hotkey plus the tray. Verify content protection still holds while layered.
- **Content protection:** `set_content_protected` → `SetWindowDisplayAffinity(WDA_EXCLUDEFROMCAPTURE)`. Apply it to main, dashboard and capture overlays (the current overlay builder at capture.rs:122-138 doesn't).
- **Resize:** `resizable(true)` on an undecorated window gets native edge hit-testing; JS can use `startResizeDragging`.
- **Placement (pure):**
  - Persist on Moved/Resized, debounced 500 ms: physical position, logical size, monitor name, scale.
  - On restore, the rect must overlap some `work_area()` by ≥ 64×32; otherwise clamp to the nearest monitor or top-centre of the primary.
  - Re-clamp on ScaleFactorChanged, on show and on move_by.
- **Tray:** a TrayIconBuilder menu; left click toggles the overlay.
- **Single-instance:** the callback shows the existing overlay without activating it.

## Screenshots

- **Region overlays:**
  1. Build `visible(false)`.
  2. `set_position(Physical(mon.position()))`, **then** `set_size(Physical(mon.size()))`.
  3. Show and focus.
  - Pair monitors by name and pass the monitor name in the URL.
  - Replace the `thread::sleep`s (capture.rs:145, :159) with an overlay-ready call.
- **Crop:** physical = CSS px × the **monitor's** scale factor, computed in Rust. Don't trust JS devicePixelRatio (browser zoom changes it).
- **Encoding:** downscale so the long edge ≤ maxEdge (`FilterType::Triangle`). Use RGB JPEG q82 (the JPEG encoder rejects RGBA, so drop alpha), or PNG for small (<1 MP) regions. maxEdge 2576 is the "hi-fi" option.
- **Full-screen target:** default to the monitor under the cursor (`xcap::Monitor::from_point`); target "overlay" reuses the overlap logic at capture.rs:318-359.
- **When content protection is off:** hide the overlay, wait ~50 ms, capture, then show it again without activating.

## Testing

**Pure unit tests:**

| Module | What to test |
|---|---|
| frame | packet splits/merges, silent zeroing, i16 clamp/LE |
| level | sine dBFS ±0.1, silence floor |
| timeline | gaps, reconnect reset, QPC→ms |
| deepgram config/protocol | URL/keyterm encoding; golden JSON fixtures for interim/final/speech_final/UtteranceEnd/SpeechStarted/Metadata/unknown |
| backoff | bounds, reset, fatal vs retry |
| turns | event sequences → exactly one turn |
| echo | table-driven cases |
| geometry | mixed-DPI fixtures (4K@200% at (0,0) + 1080p@100% at (3840,540) + negative-x monitor); name pairing; crop math |
| encode | 5000×2000 RGBA → 1568 long edge, decodable JPEG |
| placement | |
| settings | merge-patch/validate/atomic write |
| secrets | MemStore |

**Integration:**
- **Mock Deepgram:** `tokio_tungstenite::accept_hdr_async` on 127.0.0.1 with `tokio::time::pause()`. Assert the auth header and params, the 50 ms cadence, KeepAlive after 4 s idle, and the CloseStream flush. Fault injection: drop → reconnect + backlog replay + conn++; 401 fatal; 429 retries; a silent half-open socket → the watchdog fires.
- **FakeCapture:** implements `CaptureBackend` and feeds the session pipeline through `Channel::new(|body| collect)`.

**Hardware tests** (`#[ignore]`): loopback survives 15 s of silence; mic open + AEC query; keyring round-trip with a throwaway service.

**Debug:** `PLUELYX_DEBUG_DUMP=1` writes the exact 16 kHz PCM sent per source to WAV.
