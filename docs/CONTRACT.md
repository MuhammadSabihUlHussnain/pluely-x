# Pluely X: native ↔ frontend contract

This is the single source of truth for every Tauri command, event and channel. Rust (`src-tauri/`) implements it; the frontend wraps it in `src/core/native.ts`, and `src/core/native.mock.ts` mocks it for browser dev. If the contract has to change, change this file first and say so in your report.

## Conventions

- **Commands:** every command is `async` on the Rust side and returns `Result<T, AppError>`.
- **JSON:** camelCase on the wire (`#[serde(rename_all = "camelCase")]`). Command arguments are passed as a single object; Tauri converts snake_case Rust arg names to camelCase JS keys.
- **Errors:** `AppError = { code: string; message: string; detail?: string }`, serialized as that object. The frontend shows `message` and branches on `code`. Besides the per-section codes, any command can return the generic codes `internal`, `invalid_argument`, `not_found`, `io`, `json`, `tauri` (window-system failure) and `window_missing`.
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
| `keys_set` | `{ provider, value }` | `void` (trims whitespace; stored in Windows Credential Manager, service `com.sabih.pluelyx`, one entry per provider). Rejects an empty key, one with spaces/control characters or one over 512 chars with `key_invalid` |
| `keys_delete` | `{ provider }` | `void` (deleting a missing key is fine) |
| `keys_test` | `{ provider, candidate?: string }` | `{ ok: boolean; code?: string; message?: string; latencyMs: number }`. Tests `candidate` if given, otherwise the stored key. **Never rejects:** every failure, including `key_missing` and `keyring_error`, comes back as `ok: false` with a `code` |

**Event:** `keys://changed` (broadcast) → `{ anthropic: KeyStatus; deepgram: KeyStatus }` after `keys_set`/`keys_delete`, so the overlay and dashboard stay in sync. *(added)*

**How `keys_test` checks each key:**
- **anthropic:** `GET https://api.anthropic.com/v1/models?limit=1` with `x-api-key` and `anthropic-version: 2026-10-21`.
- **deepgram:** opens a real `/v1/listen` WebSocket handshake with the session params, then sends `{"type":"CloseStream"}` immediately. Fall back to `GET https://api.deepgram.com/v1/projects` if that's impractical.

**Error codes:** `key_missing`, `key_invalid` (401/403), `key_no_credit` (402), `network` (unreachable or 12 s timeout), `keyring_error`, plus `rate_limited` (429), `bad_request` (400) and `server` (5xx) in `keys_test` results.

## 2. LLM (Anthropic Messages API, proxied by Rust)

| Command | Args | Returns |
|---|---|---|
| `llm_stream` | `{ requestId: string; body: object; betas?: string[] }` plus `onEvent: Channel<LlmEvent>` | resolves when the stream ends (after `stop` or `error`) |
| `llm_cancel` | `{ requestId }` | `void` (aborts the HTTP stream; emits `{type:"error", code:"cancelled"}` on the channel; unknown ids are ignored) |

`llm_stream` itself never rejects: every failure (missing key, bad body, HTTP errors) arrives as an `error` event and then the command resolves. Reusing a `requestId` that is still streaming cancels the older stream (it gets `cancelled`). `betas` entries must match `[A-Za-z0-9._-]{1,64}` (max 16). *(clarified)*

**What Rust does with `body`:**
- **Request:** `POST https://api.anthropic.com/v1/messages` with these headers:
  - `x-api-key` (from the keyring)
  - `anthropic-version: 2026-10-21`
  - `content-type: application/json`
  - `anthropic-beta: <betas joined by ",">` when given
- **Body:** forces `"stream": true` and otherwise passes `body` through unchanged. The body is a Messages API request built by the frontend with `@anthropic-ai/sdk` types: `model`, `max_tokens`, `system`, `messages`, `output_config`, `cache_control`, `fallbacks`, ... and **no** `temperature`/`top_p`/`top_k`.
- **Screenshots in the body:** any image block whose `source.data` is the string `"@screenshot:<id>"` is replaced by the stored screenshot's base64 data, its `media_type` is set from the stored mime type and `source.type` is set to `"base64"`. This applies anywhere in the body (including inside `tool_result` content). If the id is no longer in the store, the stream fails with `screenshot_missing`. The frontend never moves image bytes across IPC.

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
| `key_invalid` | 401/403, SSE `authentication_error`/`permission_error` | no |
| `key_no_credit` | 402, SSE `billing_error`, or a 400 whose message mentions the credit balance *(added)* | no |
| `keyring_error` | Credential Manager failure *(added)* | no |
| `screenshot_missing` | `@screenshot:<id>` not in the store *(added)* | no |
| `bad_request` | 400/404/413/422, SSE `invalid_request_error` | no |
| `rate_limited` | 429 | yes |
| `overloaded` | 529 | yes |
| `server` | 5xx | yes |
| `network` | connection failure, body ended before `message_stop`, or no bytes for 120 s | yes |
| `cancelled` | `llm_cancel` | no |
| `stream_parse` | malformed SSE | no |
| `server` (from SSE) | SSE `api_error` or an unknown error type | yes |

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

*(added at merge)* Command-level codes:
- `session_active`: `session_start` while a session runs.
- `no_session`: `session_stop` or `session_set_source` with no session.
- `invalid_argument`: bad `session_set_source` payload.
- `open_failed`: the privacy settings page couldn't be opened.
- `session_ended`.

*(clarified at merge)*
- **When `session_start` rejects:** only for `key_missing`, `session_active`, or when **every** enabled source failed to open (rejects with the first source's code: `mic_permission_denied` / `device_not_found` / `audio_init_failed`). A **partial start succeeds**: the failed source gets `error {source, fatal:true}` + `state {source, state:"error"}`, and `SessionInfo` omits its `me`/`them`.
- **Session-level state** (`source: null`) goes `connecting` → `live` (the first source is live) → `error` (every enabled source failed fatally) → `stopping` → `stopped`. Disabling a source ends with `state {source, "idle"}`, plus a `turn {reason:"stop"}` for pending finals.
- **Stop timing:** `session_stop` can take ~4.1 s worst case (capture stop + 3 s CloseStream flush + socket close).
- **Clocks:** `startedAtMs` is Unix epoch ms; every event time (`t0Ms`, `t1Ms`, `tMs`, `lastWordEndMs`) is ms since session start.
- **Connection counters:** `conn` increments on every connection attempt (including failed ones); `stats.reconnects` counts successful reconnections only. Hence a re-enabled source can have segIds like `me-2-0`.
- **Dropped events:** empty interims/finals are dropped unless they clear an interim already on screen.
- **Echo:** `echo` is only ever set on `me` finals, and echo text never forms a `me` turn.
- **Levels:** a source that is disabled or has no open capture reports `null` in `levels`.
- **AEC:** `SessionInfo.aec` is `"off"` when the mic isn't open.
- **`session_set_source` `deviceId`:** absent = unchanged, `null` = follow the default, a string = that endpoint (falls back to the default with `device {reason:"fallback"}` if missing, and switches back automatically when it reappears). `muted:true` sends Finalize and stops sending audio, but the meters keep running.
- **Debug dump:** `PLUELYX_DEBUG_DUMP=1` writes `pluelyx-<sessionId>-<me|them>.wav` (the exact 16 kHz PCM sent to Deepgram) into the app log dir.

**Broadcast:** `session://status` → `{ active: boolean; sessionId?: string }` on start and stop, so the dashboard and tray stay in sync.

## 4. Window

`WindowState = { visible: boolean; clickThrough: boolean; contentProtected: boolean; alwaysOnTop: boolean; typing: boolean; bounds: { x: number; y: number; width: number; height: number } }` (logical px).

| Command | Args | Returns |
|---|---|---|
| `window_show` / `window_hide` / `window_toggle` | – | `WindowState` (a real native hide/show; showing never steals focus) |
| `window_get_state` | – | `WindowState` |
| `window_set_size` | `{ width?: number; height?: number }` | `WindowState` (logical px, clamped to the monitor work area and to a 200×40 minimum). **Not persisted:** the size used at launch is `settings.overlay.width/height`, so persist the expanded (non-pill) size with `settings_update` *(clarified)* |
| `window_set_click_through` | `{ enabled }` | `WindowState` (also persists `settings.overlay.clickThrough` and broadcasts `settings://changed`) |
| `window_set_content_protection` | `{ enabled }` | `WindowState` (applies to **all** app windows; persists `settings.overlay.contentProtection`) |
| `window_set_always_on_top` | `{ enabled }` | `WindowState` (persists `settings.overlay.alwaysOnTop`) |
| `window_begin_typing` | – | `WindowState` (overlay becomes focusable and takes focus; remembers the previous foreground window) |
| `window_end_typing` | `{ restoreFocus: boolean }` | `WindowState` (overlay goes non-activating again; refocuses the previous window if asked) |
| `window_move_by` | `{ dx: number; dy: number }` | `WindowState` (logical px; the overlay never leaves the visible work areas) |
| `window_reset_position` | – | `WindowState` (top-centre of the primary work area) |
| `dashboard_open` / `dashboard_toggle` | `{ route?: string }` | `void` (route e.g. `"/dashboard/settings"`; only `/dashboard`, `/dashboard/...` and `/dashboard?...` are accepted, otherwise `invalid_argument`). The dashboard is created on first open; its close button hides it |
| `app_quit` / `app_open_logs` | – | `void` |
| `app_version` | – | `string` |

**Events:**
- `window://state` (broadcast) → `WindowState` whenever anything changes (debounced ~120 ms for moves/resizes).
- `window://typing` (main) → `{ active: boolean }`. This also fires when typing ends because focus left the overlay.
- `dashboard://navigate` (dashboard) → `{ route: string }` when `dashboard_open`/`dashboard_toggle` gets a route while the dashboard window already exists; the dashboard router should navigate instead of reloading. *(added)*

**Notes:** the overlay position is saved natively (debounced) in `<appConfig>/placement.json` and restored and clamped at launch. Showing the overlay never takes focus; only `window_begin_typing` (or the `focus_input` hotkey) does. Clicking buttons in the overlay does not steal focus from the user's app (verified, see docs/SPIKE_WINDOW.md), so typing mode is only needed for text input: call `window_begin_typing` on mousedown in the ask box and `window_end_typing({ restoreFocus: true })` on Esc/submit. `settings.overlay.opacity` is applied by the frontend (CSS); Rust doesn't change window opacity.

## 5. Screenshots

```ts
interface ScreenshotMeta { id: string; mime: "image/jpeg" | "image/png"; width: number; height: number;
  sourceWidth: number; sourceHeight: number; monitor: string; bytes: number; thumbDataUrl: string /* ≤ 320px data URL */ }
```

| Command | Args | Returns |
|---|---|---|
| `screenshot_full` | `{ target?: "cursor" \| "overlay"; maxEdge?: number; format?: "auto" \| "jpeg" \| "png"; quality?: number }` | `ScreenshotMeta` (defaults: cursor monitor, 1568, auto, 82). `maxEdge` is clamped to 64–2576 and `quality` to 30–95. `auto` = PNG when the captured source is under 1 MP, JPEG otherwise; images are always RGB (no alpha). A full 1080p shot is about 200 KB of JPEG |
| `screenshot_region_start` | – | `void` (freezes every monitor, then opens `capture-overlay-<n>` on each at route `/capture?monitor=<percent-encoded name>`; the overlay under the cursor gets focus; the result arrives as an event) |
| `screenshot_region_commit` | `{ monitor: string; rect: { x: number; y: number; width: number; height: number } }` | `ScreenshotMeta` (called by the capture overlay; `monitor` is the decoded `monitor` query value; rect in CSS px of that overlay, negative width/height allowed; Rust converts using the monitor scale and closes all capture overlays). A selection under 2×2 physical px fails with `invalid_argument` and also emits `screenshot://cancelled` |
| `screenshot_region_cancel` | – | `void` |
| `screenshot_get` | `{ id }` | `{ mime: string; dataBase64: string }` |
| `screenshot_save` | `{ id; sessionId }` | `{ path: string }` (copies into `<appData>/screenshots/<sessionId>/<id>.<ext>` for history) |
| `screenshot_load` | `{ path }` | `{ mime: string; dataBase64: string }` (only paths under `<appData>/screenshots`; otherwise `forbidden`, or `not_found` if the file doesn't exist) |
| `screenshot_discard` | `{ id }` | `void` |

**Events (main):**
- `screenshot://captured` → `ScreenshotMeta`
- `screenshot://cancelled` → `{}`

The store keeps the last 10 shots in memory by id. `screenshot_save` ids and sessionIds must match `[A-Za-z0-9_-]{1,80}`. Capture errors use `capture_failed` / `encode_failed`. After a region capture the previously focused app gets the foreground back.

## 6. Settings (Rust-owned; persisted as `<appConfig>/settings.json`)

```ts
interface AppSettings {
  version: 1;                                   // owned by Rust; patches can't change it
  onboarded: boolean;                           // default false
  mode: Mode;                                   // last used mode; default "ask"
  overlay: { width: number /*440, 200–8192*/; height: number /*560, 40–8192*/; opacity: number /*1, 0.3–1*/; clickThrough: boolean /*false*/;
             contentProtection: boolean /*default true*/; alwaysOnTop: boolean /*true*/; launchAtStartup: boolean /*false*/ };
  audio: { micDeviceId: string | null /*null*/; outputDeviceId: string | null /*null*/; aec: "auto" | "on" | "off" /*"auto"*/ };
  stt: { language: string /*"en"*/; endpointingMs: number /*300, 10–5000*/; utteranceEndMs: number /*1000, 1000–5000*/; diarizeThem: boolean /*false*/ };
  ai: { model: string /*"claude-opus-5-5"*/; liveEffort: "low" | "medium" | "high" /*"low"*/; deepEffort: "low" | "medium" | "high" /*"medium"*/; maxTokens: number /*16000, 256–128000*/ };
  shortcuts: Record<ShortcutAction, { key: string; enabled: boolean }>;   // defaults in §7
}
```

| Command | Args | Returns |
|---|---|---|
| `settings_get` | – | `AppSettings` |
| `settings_update` | `{ patch: DeepPartial<AppSettings> }` | `AppSettings` (validates, saves atomically, applies side effects: shortcuts, window flags, autostart). The patch is an RFC 7396 merge-patch: `null` resets that field to its default (so `{audio:{micDeviceId:null}}` means "default mic"). Invalid values, unknown enum values, unparseable shortcuts or two enabled actions sharing a key reject the whole patch with `invalid_settings`, and nothing is saved. Shortcut registration failures don't reject; they're reported via `shortcut://error`. `overlay.width/height` are used at the next launch and don't resize the running overlay |
| `settings_reset` | `{ section?: keyof AppSettings }` | `AppSettings` (no section = everything, including `onboarded`) |

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
| `focus_input` | `ctrl+alt+q` |
| `toggle_click_through` | `ctrl+alt+c` |
| `toggle_dashboard` | `ctrl+alt+d` |
| `move_*` | `ctrl+alt+arrowup` / `ctrl+alt+arrowdown` / `ctrl+alt+arrowleft` / `ctrl+alt+arrowright` (hold to move) |

**Where each action is handled:**
- **Natively** (Rust): `toggle_overlay`, `toggle_click_through`, `toggle_dashboard`, `move_*`, and `focus_input`, which calls `window_begin_typing` first and then also emits the action.
- **Frontend:** everything else arrives as `shortcut://action` (main) → `{ action: ShortcutAction }`.

| Command | Args | Returns |
|---|---|---|
| `shortcuts_validate` | `{ key: string }` | `boolean` (false for unparseable keys and for keys without Ctrl, Alt or Win, unless the key is F1–F24) |
| `shortcuts_status` | – | `{ registered: ShortcutAction[]; failures: { action: ShortcutAction; key: string; error: string }[] }` *(added: startup failures happen before the webview listens, so call this on mount)* |

Shortcuts are changed via `settings_update({ patch: { shortcuts } })`. Registration failures (for example a key taken by another app) emit `shortcut://error` (main) → `{ failures: { action: ShortcutAction; key: string; error: string }[] }`. On the dev machine `ctrl+backslash` was taken by the installed Pluely and `ctrl+alt+space` by another app (so `focus_input` now defaults to `ctrl+alt+q`), so the UI must surface these failures and offer the recorder. Some Intel graphics drivers also use Ctrl+Alt+arrows.

## 8. Misc events and commands

| Command | Args | Returns |
|---|---|---|
| `log_write` | `{ level: "error" \| "warn" \| "info" \| "debug"; message: string }` | `void` (writes to the app log under target `webview`; max 8 KB; anything that looks like an `sk-ant-` key is masked. Never pass secrets or transcript text) *(added; listed in PLAN.md but missing here)* |

- `app://error` (main) → `{ code: string; message: string }` for background failures (e.g. `autostart_failed`).
- **Tray menu** (left click on the icon toggles the overlay):
  - Show/Hide overlay
  - Start/Stop listening (shows the overlay, then emits `shortcut://action {action:"toggle_listen"}`; the label follows `session://status`)
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
CREATE INDEX idx_sessions_started ON sessions(started_at DESC);   -- (added)
```

Search with e.g. `SELECT t.* FROM transcript_fts f JOIN transcript t ON t.rowid = f.rowid WHERE transcript_fts MATCH ?`. Foreign keys are on (sqlx default), so deleting a session cascades to its rows and their FTS entries. The FTS indexes use the tables' implicit rowids, which a `VACUUM` may renumber: after any VACUUM run `INSERT INTO transcript_fts(transcript_fts) VALUES('rebuild')` and the same for `cards_fts`. The exact SQL is `src-tauri/src/db/migrations/v1.sql`.

*(added at integration)* **Stored values** (written by the frontend; keep both sides in agreement):

| Field | Values |
|---|---|
| `cards.kind` | `auto` (auto-answer to a Them question), `ask_now` ("What should I say?"), `typed`, `screen` (screenshot ask/solve), `followup:<chip>` (`shorter` / `more` / `example` / `followup`) |
| `cards.status` | `done` or `truncated` (`stop_reason: max_tokens`); error cards are never stored |
| `notes.kind` | the mode (`meeting`, `lecture`, ...) for rolling notes |
| `sessions.summary_json` | the end-of-session summary |
| `briefs.mode` | marks a brief as the default for that mode, so a brief can be the default for one mode at a time |

---

## Changelog

**2026-11-01 (native foundation):** explicit settings defaults, ranges and merge-patch semantics (§6); `shortcuts_status` (§7); `log_write` (§8); `keys://changed` and a never-rejecting `keys_test` (§1); `key_no_credit`, `keyring_error` and `screenshot_missing` LLM error codes and a never-rejecting `llm_stream` (§2); `dashboard://navigate` and persistence notes for size, flags and placement (§4); format, clamping and route details for screenshots (§5); generic error codes (Conventions); `idx_sessions_started` and the VACUUM note (§9).

**2026-11-01 (live session merge):** §3 command-level error codes (`session_active`, `no_session`, `invalid_argument`, `open_failed`, `session_ended`); when `session_start` rejects and the partial-start behaviour; session-level state machine; stop timing; epoch vs session clocks; `conn` and `reconnects` counters; empty/echo/levels rules; `deviceId` absent/null/string semantics for `session_set_source`; the debug dump. The §3 commands live in the `session` module (`session::audio_list_devices` etc.).
