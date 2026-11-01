# Pluely X: native ↔ frontend contract

This is the single source of truth for every Tauri command, event and channel. Rust (`src-tauri/`) implements it; the frontend wraps it in `src/core/native.ts`, and `src/core/native.mock.ts` mocks it for browser dev. If the contract has to change, change this file first and say so in your report.

## Conventions

- **Commands:** every command is `async` on the Rust side and returns `Result<T, AppError>`.
- **JSON:** camelCase on the wire (`#[serde(rename_all = "camelCase")]`). Command arguments are passed as a single object; Tauri converts snake_case Rust arg names to camelCase JS keys.
- **Errors:** `AppError = { code: string; message: string; detail?: string }`, serialized as that object. The frontend shows `message` and branches on `code`.
- **Streams:** high-rate or ordered, caller-scoped streams use `tauri::ipc::Channel<T>` (session events, LLM events). Discrete UI events use `emit_to("main", ...)`; cross-window state uses broadcast `emit`. Event names use the `area://name` form.
- **Windows:**
  - `main`: the overlay. Built in code in `setup`, not in `tauri.conf.json`. Route `/`.
  - `dashboard`: sessions and settings. Route `/dashboard`.
  - `capture-overlay-<n>`: region selector. Route `/capture?monitor=<name>`.
  - `src/main.tsx` picks the UI by window label.
- **Secrets:**
  - API keys never cross into the webview. No command returns a secret.
  - The webview never calls `api.anthropic.com` or Deepgram itself. All network calls go through Rust.
- **Logging:** logs never contain secrets or transcript text (transcripts only at debug level).

```ts
type Source = "me" | "them";
type Mode = "meeting" | "interview" | "coding" | "lecture" | "ask";
type Provider = "anthropic" | "deepgram";
interface AppError { code: string; message: string; detail?: string }
```

---

## 1. Keys

| Command | Args | Returns |
|---|---|---|
| `keys_status` | – | `{ anthropic: KeyStatus; deepgram: KeyStatus }`, where `KeyStatus = { present: boolean; hint?: string }` and `hint` is the last 4 characters, e.g. `"…a1b2"` |
| `keys_set` | `{ provider, value }` | `void` (trims whitespace; stored in Windows Credential Manager, service `com.sabih.pluelyx`) |
| `keys_delete` | `{ provider }` | `void` |
| `keys_test` | `{ provider, candidate?: string }` | `{ ok: boolean; code?: string; message?: string; latencyMs: number }`. Tests `candidate` if given, otherwise the stored key |

**How `keys_test` checks each key:**
- **anthropic:** `GET https://api.anthropic.com/v1/models?limit=1` with `x-api-key` and `anthropic-version: 2026-10-21`.
- **deepgram:** opens a real `/v1/listen` WebSocket handshake with the session params, then sends `{"type":"CloseStream"}` immediately. Fall back to `GET https://api.deepgram.com/v1/projects` if that's impractical.

**Error codes:** `key_missing`, `key_invalid` (401/403), `key_no_credit` (402), `network`, `keyring_error`.

## 2. LLM (Anthropic Messages API, proxied by Rust)

| Command | Args | Returns |
|---|---|---|
| `llm_stream` | `{ requestId: string; body: object; betas?: string[] }` plus `onEvent: Channel<LlmEvent>` | resolves when the stream ends (after `stop` or `error`) |
| `llm_cancel` | `{ requestId }` | `void` (aborts the HTTP stream; emits `{type:"error", code:"cancelled"}` on the channel) |

**What Rust does with `body`:**
- **Request:** `POST https://api.anthropic.com/v1/messages` with these headers:
  - `x-api-key` (from the keyring)
  - `anthropic-version: 2026-10-21`
  - `content-type: application/json`
  - `anthropic-beta: <betas joined by ",">` when given
- **Body:** forces `"stream": true` and otherwise passes `body` through unchanged. The body is a Messages API request built by the frontend with `@anthropic-ai/sdk` types: `model`, `max_tokens`, `system`, `messages`, `output_config`, `cache_control`, `fallbacks`, ... and **no** `temperature`/`top_p`/`top_k`.
- **Screenshots in the body:** any image block whose `source.data` is the string `"@screenshot:<id>"` is replaced by the stored screenshot's base64 data, and its `media_type` is set from the stored mime type. The frontend never moves image bytes across IPC.

```ts
type LlmEvent =
  | { type: "start"; requestId: string; model: string }
  | { type: "text"; delta: string }                       // content_block_delta / text_delta
  | { type: "thinking" }                                   // any thinking block activity (UI may show "thinking…")
  | { type: "stop"; stopReason: string | null;             // "end_turn" | "max_tokens" | "refusal" | ...
      usage: { inputTokens: number; outputTokens: number; cacheReadInputTokens: number; cacheCreationInputTokens: number } }
  | { type: "error"; code: string; message: string; status?: number; retryable: boolean };
```

**Error codes** (from HTTP status or the SSE `error` event type):

| Code | Source | Retryable |
|---|---|---|
| `key_missing` | no stored key | no |
| `key_invalid` | 401/403 | no |
| `bad_request` | 400 | no |
| `rate_limited` | 429 | yes |
| `overloaded` | 529 | yes |
| `server` | 5xx | yes |
| `network` | connection failure | yes |
| `cancelled` | `llm_cancel` | no |
| `stream_parse` | malformed SSE | no |

For 400s, `message` carries the API's error message.

## 3. Live session (Phase 2): audio capture + Deepgram

| Command | Args | Returns |
|---|---|---|
| `audio_list_devices` | – | `{ inputs: Device[]; outputs: Device[] }`, `Device = { id: string; name: string; isDefault: boolean; isDefaultComms: boolean }` |
| `audio_open_mic_privacy_settings` | – | `void` (opens `ms-settings:privacy-microphone`) |
| `session_start` | `{ opts: SessionOptions }` plus `onEvent: Channel<SessionEvent>` | `SessionInfo = { sessionId: string; startedAtMs: number; me?: DeviceInfo; them?: DeviceInfo; aec: "on" \| "off" \| "unsupported" }` |
| `session_stop` | – | `SessionSummary = { sessionId: string; durationMs: number; reconnects: number; droppedMs: number }` (waits ≤ 3 s for CloseStream flush) |
| `session_set_source` | `{ source: Source; enabled?: boolean; muted?: boolean; deviceId?: string \| null }` | `void` |
| `session_status` | – | `{ active: boolean; sessionId?: string; startedAtMs?: number }` |
| `session_reattach` | `onEvent: Channel<SessionEvent>` | `void` (after a webview reload; first sends a `snapshot`) |

```ts
interface SessionOptions {
  me:   { enabled: boolean; deviceId?: string | null };            // null/undefined = default comms mic
  them: { enabled: boolean; deviceId?: string | null };            // loopback of default (console) output
  stt:  { language: string; keyterms: string[]; endpointingMs: number; utteranceEndMs: number; diarizeThem: boolean };
  aec:  "auto" | "on" | "off";
  echoDedup: boolean;
}
interface DeviceInfo { deviceId: string; name: string; format: string }
type Level = { rmsDb: number; peakDb: number };                     // -90..0 dBFS
type SessionEvent =
 | { type: "state"; source: Source | null; state: "connecting" | "live" | "reconnecting" | "idle" | "stopping" | "stopped" | "error"; attempt?: number; retryInMs?: number }
 | { type: "device"; source: Source; reason: "opened" | "defaultChanged" | "lost" | "fallback"; deviceId: string; name: string; format: string; aec?: "on" | "off" | "unsupported" }
 | { type: "transcript"; source: Source; segId: string; conn: number; isFinal: boolean; speechFinal: boolean; fromFinalize: boolean;
     text: string; confidence: number; t0Ms: number; t1Ms: number; speaker?: number; echo?: boolean }
 | { type: "transcriptPatch"; segId: string; echo: boolean }
 | { type: "speechStarted"; source: Source; tMs: number }
 | { type: "utteranceEnd"; source: Source; lastWordEndMs: number }
 | { type: "turn"; source: Source; turnId: string; text: string; t0Ms: number; t1Ms: number; reason: "speechFinal" | "utteranceEnd" | "stop"; question: boolean }
 | { type: "levels"; tMs: number; me: Level | null; them: Level | null }          // 10 Hz
 | { type: "error"; source?: Source; code: string; message: string; fatal: boolean; retryInMs?: number }
 | { type: "stats"; source: Source; sentMs: number; droppedMs: number; reconnects: number; discontinuities: number }   // every 5 s
 | { type: "snapshot"; status: { active: boolean; sessionId?: string; startedAtMs?: number }; recentFinals: SessionEvent[] };
```

**Semantics:**
- **Interim results:** interims for the same `segId` replace each other until `isFinal`.
- **Time:** `t0Ms`/`t1Ms` are on one session clock (ms since session start), so they're comparable across me and them.
- **Turns:** exactly one `turn` is sent per speaker turn. `question` is a cheap hint (ends with `?`).
- **Echo:** an `echo: true` transcript (or a later `transcriptPatch`) is mic audio duplicating `them`; the UI hides it by default.

**Error codes:** `mic_permission_denied`, `device_not_found`, `device_lost`, `audio_init_failed`, `key_missing`, `dg_auth`, `dg_payment`, `dg_rate_limited`, `dg_bad_request`, `dg_network`, `dg_server`, `dg_timeout`.

**Broadcast:** `session://status` → `{ active: boolean; sessionId?: string }` on start and stop, so the dashboard and tray stay in sync.

## 4. Window

`WindowState = { visible: boolean; clickThrough: boolean; contentProtected: boolean; alwaysOnTop: boolean; typing: boolean; bounds: { x: number; y: number; width: number; height: number } }` (logical px).

| Command | Args | Returns |
|---|---|---|
| `window_show` / `window_hide` / `window_toggle` | – | `WindowState` (a real native hide/show; showing never steals focus) |
| `window_get_state` | – | `WindowState` |
| `window_set_size` | `{ width?: number; height?: number }` | `WindowState` (logical px, clamped to the monitor work area) |
| `window_set_click_through` | `{ enabled }` | `WindowState` |
| `window_set_content_protection` | `{ enabled }` | `WindowState` (applies to **all** app windows) |
| `window_set_always_on_top` | `{ enabled }` | `WindowState` |
| `window_begin_typing` | – | `WindowState` (overlay becomes focusable and takes focus; remembers the previous foreground window) |
| `window_end_typing` | `{ restoreFocus: boolean }` | `WindowState` (overlay goes non-activating again; refocuses the previous window if asked) |
| `window_move_by` | `{ dx: number; dy: number }` | `WindowState` |
| `window_reset_position` | – | `WindowState` (top-centre of the primary work area) |
| `dashboard_open` / `dashboard_toggle` | `{ route?: string }` | `void` (route e.g. `"/dashboard/settings"`) |
| `app_quit` / `app_open_logs` | – | `void` |
| `app_version` | – | `string` |

**Events:**
- `window://state` (broadcast) → `WindowState` whenever anything changes.
- `window://typing` (main) → `{ active: boolean }`. This also fires when typing ends because focus left the overlay.

## 5. Screenshots

```ts
interface ScreenshotMeta { id: string; mime: "image/jpeg" | "image/png"; width: number; height: number;
  sourceWidth: number; sourceHeight: number; monitor: string; bytes: number; thumbDataUrl: string /* ≤ 320px data URL */ }
```

| Command | Args | Returns |
|---|---|---|
| `screenshot_full` | `{ target?: "cursor" \| "overlay"; maxEdge?: number; format?: "auto" \| "jpeg" \| "png"; quality?: number }` | `ScreenshotMeta` (defaults: cursor monitor, 1568, auto, 82) |
| `screenshot_region_start` | – | `void` (opens `capture-overlay-*` on every monitor; the result arrives as an event) |
| `screenshot_region_commit` | `{ monitor: string; rect: { x: number; y: number; width: number; height: number } }` | `ScreenshotMeta` (called by the capture overlay; rect in CSS px of that overlay; Rust converts using the monitor scale) |
| `screenshot_region_cancel` | – | `void` |
| `screenshot_get` | `{ id }` | `{ mime: string; dataBase64: string }` |
| `screenshot_save` | `{ id; sessionId }` | `{ path: string }` (copies into `<appData>/screenshots/<sessionId>/<id>.<ext>` for history) |
| `screenshot_load` | `{ path }` | `{ mime: string; dataBase64: string }` (only paths under `<appData>/screenshots`) |
| `screenshot_discard` | `{ id }` | `void` |

**Events (main):**
- `screenshot://captured` → `ScreenshotMeta`
- `screenshot://cancelled` → `{}`

The store keeps the last ~10 shots in memory by id.

## 6. Settings (Rust-owned; persisted as `<appConfig>/settings.json`)

```ts
interface AppSettings {
  version: 1;
  onboarded: boolean;
  mode: Mode;                                   // last used mode
  overlay: { width: number; height: number; opacity: number /*0.3–1*/; clickThrough: boolean;
             contentProtection: boolean /*default true*/; alwaysOnTop: boolean /*true*/; launchAtStartup: boolean /*false*/ };
  audio: { micDeviceId: string | null; outputDeviceId: string | null; aec: "auto" | "on" | "off" };
  stt: { language: string /*"en"*/; endpointingMs: number /*300*/; utteranceEndMs: number /*1000*/; diarizeThem: boolean };
  ai: { model: string /*"claude-opus-5-5"*/; liveEffort: "low" | "medium" | "high"; deepEffort: "low" | "medium" | "high"; maxTokens: number /*16000*/ };
  shortcuts: Record<ShortcutAction, { key: string; enabled: boolean }>;
}
```

| Command | Args | Returns |
|---|---|---|
| `settings_get` | – | `AppSettings` |
| `settings_update` | `{ patch: DeepPartial<AppSettings> }` | `AppSettings` (validates, saves atomically, applies side effects: shortcuts, window flags, autostart) |
| `settings_reset` | `{ section?: keyof AppSettings }` | `AppSettings` |

**Event:** `settings://changed` (broadcast) → `AppSettings`.

## 7. Shortcuts

```ts
type ShortcutAction = "toggle_overlay" | "toggle_listen" | "ask_now" | "screenshot_full" | "screenshot_region"
                    | "focus_input" | "toggle_click_through" | "toggle_dashboard" | "move_up" | "move_down" | "move_left" | "move_right";
```

**Defaults** (Windows accelerator strings):

| Action | Key |
|---|---|
| `toggle_overlay` | `ctrl+backslash` |
| `toggle_listen` | `ctrl+alt+l` |
| `ask_now` | `ctrl+alt+enter` |
| `screenshot_full` | `ctrl+alt+s` |
| `screenshot_region` | `ctrl+alt+r` |
| `focus_input` | `ctrl+alt+space` |
| `toggle_click_through` | `ctrl+alt+c` |
| `toggle_dashboard` | `ctrl+alt+d` |
| `move_*` | `ctrl+alt+arrowup` / `ctrl+alt+arrowdown` / `ctrl+alt+arrowleft` / `ctrl+alt+arrowright` (hold to move) |

**Where each action is handled:**
- **Natively** (Rust): `toggle_overlay`, `toggle_click_through`, `toggle_dashboard`, `move_*`, and `focus_input`, which calls `window_begin_typing` first and then also emits the action.
- **Frontend:** everything else arrives as `shortcut://action` (main) → `{ action: ShortcutAction }`.

| Command | Args | Returns |
|---|---|---|
| `shortcuts_validate` | `{ key: string }` | `boolean` |

Shortcuts are changed via `settings_update({ patch: { shortcuts } })`. Registration failures (for example a key taken by another app) emit `shortcut://error` (main) → `{ failures: { action: ShortcutAction; key: string; error: string }[] }`.

## 8. Misc events

- `app://error` (main) → `{ code: string; message: string }` for background failures.
- **Tray menu:**
  - Show/Hide overlay
  - Start/Stop listening (emits `shortcut://action {action:"toggle_listen"}`)
  - Click-through ✓
  - Hide from screen share ✓
  - Open dashboard
  - Open logs
  - Quit

## 9. Database (tauri-plugin-sql, `sqlite:pluelyx.db`, migrations in `src-tauri/src/db/`)

The frontend queries it directly through `@tauri-apps/plugin-sql`. Migration v1:

```sql
CREATE TABLE sessions (id TEXT PRIMARY KEY, mode TEXT NOT NULL, title TEXT NOT NULL DEFAULT '', started_at INTEGER NOT NULL,
  ended_at INTEGER, brief_id TEXT, summary_json TEXT);
CREATE TABLE transcript (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  speaker TEXT NOT NULL CHECK (speaker IN ('me','them')), text TEXT NOT NULL, t0_ms INTEGER NOT NULL, t1_ms INTEGER NOT NULL);
CREATE INDEX idx_transcript_session ON transcript(session_id, t0_ms);
CREATE TABLE cards (id TEXT PRIMARY KEY, session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  kind TEXT NOT NULL, question TEXT NOT NULL DEFAULT '', content TEXT NOT NULL DEFAULT '', screenshot_path TEXT,
  status TEXT NOT NULL DEFAULT 'done', created_at INTEGER NOT NULL);
CREATE INDEX idx_cards_session ON cards(session_id, created_at);
CREATE TABLE notes (session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE, kind TEXT NOT NULL,
  content_json TEXT NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY (session_id, kind));
CREATE TABLE briefs (id TEXT PRIMARY KEY, name TEXT NOT NULL, mode TEXT, content TEXT NOT NULL DEFAULT '',
  files_json TEXT NOT NULL DEFAULT '[]', created_at INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE mode_prompts (mode TEXT PRIMARY KEY, prompt TEXT NOT NULL, updated_at INTEGER NOT NULL);
CREATE VIRTUAL TABLE transcript_fts USING fts5(text, content='transcript', content_rowid='rowid');
CREATE VIRTUAL TABLE cards_fts USING fts5(question, content, content='cards', content_rowid='rowid');
-- plus AFTER INSERT/DELETE/UPDATE triggers keeping both FTS tables in sync
```
