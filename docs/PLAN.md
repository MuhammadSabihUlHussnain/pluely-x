# Pluely X: a rebuilt, much better Pluely

## Context

Sabih installed Pluely (open-source Cluely alternative, GPL-3.0, Tauri 2 + React) and an unlocked source build. He wants a "10x version" that works better and is far more intuitive, for **all four** uses: meetings & calls, interviews & coding, lectures & classes, and asking about the screen. Decisions already made:
- **Rebuild on Pluely.** Keep its native layer (WASAPI system audio, screenshots, overlay hidden from screen share, global shortcuts). Rebuild the UX, AI and speech layers. The code stays GPL-3.0.
- **Cloud speech-to-text** (Deepgram streaming).

Two code audits found these problems in today's Pluely:
- **Setup:** about 12 actions spread over two windows before a first meeting answer, using cURL templates in "Dev space", typed-in model names and a second key for speech-to-text.
- **Latency:** batch speech-to-text after 1 s of silence, then upload, then the LLM: 2–5 s after the person stops talking.
- **Listening mode:** sends *every* utterance to the LLM with the history in **reverse order**, overlapping streams interleave, and the user's own voice never enters context.
- **Overlay:** a 600×54 bar that jumps to 600 px tall, with 9–11 px text, no tooltips, and answers that vanish when you click away.
- **Errors:** shown and saved as if they were AI answers. There are also no meeting or lecture notes and no transcript view.
- **Native bugs:** loopback capture dies silently after 3 s of silence; mic device IDs don't match; DPI bugs in region capture; "hide" doesn't really hide on Windows; Ctrl+Shift+M collides with Teams mute.
- **The public repo trails the official binary.** The official release has 7 DB migrations, the public source has 2. So feature parity has to be built, not merged.

Intended outcome: a self-built Windows app, **Pluely X**, installed alongside the current Pluely (new identifier `com.sabih.pluelyx`, so current data stays untouched). It is set up in about a minute, transcribes both sides live, answers questions automatically as they're asked, solves what's on screen with one key, and writes notes afterwards.

---

## What the user gets (the "10x")

1. **One-minute setup wizard** (first run only):
   - **Claude key:** paste the Anthropic key; it's validated with a live call and a green check.
   - **Deepgram key (optional):** "for listening". Validated the same way; includes a $200 free-credit signup link.
   - **Audio check:** mic and speaker meters show real levels. Say something and watch it transcribe.
   - **Shortcut card.** Done.

   Keys go to **Windows Credential Manager**, never localStorage.

2. **One overlay panel, not a bar.** Resizable (default 440×560), remembers size and position, always clamped on screen. Three zones:
   - **Top bar:** mode switcher, a **Listen** button with real level meters, a "hidden from screen share" indicator, a pin/click-through toggle, and a collapse-to-pill button.
   - **Live transcript strip:** *Them* and *You* color-coded, with interim words fading in as they're spoken; it collapses to two lines.
   - **Answer cards:** see item 5.

   **The ask box is always there,** including while listening or while an answer is streaming. Readable 13–15 px text and tooltips throughout. Collapsing shrinks the panel to a small pill showing mode and status.

3. **Modes** replace the six separate prompt surfaces. Each mode is a tuned prompt plus behaviour, editable in one place:

   | Mode | Live behaviour | After the session |
   |---|---|---|
   | **Meeting** | Suggests replies when you're asked something; rolling notes (decisions and action items) | Summary, decisions, action items, draft follow-up email |
   | **Interview** | Detects the interviewer's question and answers it (STAR for behavioural questions), grounded in your résumé and the job description | The questions asked, how you answered, tips |
   | **Coding** | "Solve screen" gives approach, code, complexity and edge cases; also follows the spoken problem | The problems and the solutions given |
   | **Lecture** | No auto-answers. Rolling key points, jargon explained inline, ask anything | Study notes, key terms, practice questions |
   | **Ask** | Plain screen and question assistant (no listening needed) | — |

4. **Auto question detection** (the core upgrade):
   - **Trigger:** when *Them* finishes a turn (Deepgram endpointing / utterance-end).
   - **Fast filter:** a local check (`?`, wh-words, "can you / tell me / walk me through / how would you"), then a small structured-output Claude call that returns `{is_question_for_me, question}` from the last ~60 s of transcript.
   - **Answer:** if it's a question, an answer card starts streaming within about 1 s.
   - **No duplicates:** stale requests are cancelled and the same question is never answered twice.
   - **Manual trigger:** a **"What should I say?"** hotkey answers from the last ~2 minutes at any time.

5. **Answer cards built for glancing:**
   - **Layout:** a large one-line answer first, then 2–4 bullets, then an expandable "details / code" section with syntax highlighting and copy buttons.
   - **Context:** the triggering question (or a screenshot thumbnail) is shown on the card.
   - **Follow-up chips:** *Shorter · More detail · Example · Follow-up*.
   - **History:** cards stack and stay scrollable; nothing vanishes on click-away.
   - **Errors** appear as red inline cards with a fix button (for example "Key rejected → Open settings"). They are never saved as answers.

6. **Screen in two keys:** *Ask about screen* captures the monitor the overlay is on; *Ask about region* drags a box. The image is downscaled to ≤1568 px on its long side before sending. Coding mode auto-solves. Other modes attach the image and focus the ask box (or answer immediately if a question is already typed).

7. **Briefs:** per-session context such as a résumé, job description, agenda or slides (PDF, DOCX or TXT turned into text), saved as reusable profiles. They are sent as a **cached prompt prefix**, so long briefs stay fast and cheap.

8. **Sessions library** (the dashboard window): every listening or ask session is saved with its transcript, answer cards, notes and screenshots. It has full-text search (SQLite FTS5), a per-session view, Markdown export, delete and delete-all.

9. **One Settings page:**
   - **Keys & models:** shows status, lets you re-test, and picks models.
   - **Audio:** devices.
   - **Shortcuts:** recorder with conflict warnings.
   - **Overlay:** opacity, click-through, hide from screen share (default on), always on top (default on), launch at startup (default **off**).
   - **Modes & briefs.**
   - **Data:** export / delete.

10. **New default shortcuts** that avoid Teams, Office and editor keys. All can be remapped.

    | Action | Default |
    |---|---|
    | Show / hide overlay | Ctrl+\ |
    | Start / stop listening | Ctrl+Alt+L |
    | "What should I say?" | Ctrl+Alt+Enter |
    | Ask about screen | Ctrl+Alt+S |
    | Ask about region | Ctrl+Alt+R |
    | Type a question (focus the ask box) | Ctrl+Alt+Q |
    | Click-through on / off | Ctrl+Alt+C |
    | Move overlay | Ctrl+Alt+Arrows |

**Removed:**
- Pluely server, license and promo code, PostHog, the updater, and machine-uid.
- cURL templates and "Dev space"; the Responses, Screenshot and Dashboard pages; the custom cursor; silent autostart.
- The webview mic pipeline and the CDN-loaded VAD model.

---

## Architecture

**Where:** the new repo is `C:\Users\muham\Desktop\Pluely X`, cloned locally from `Desktop\Pluely` at upstream tag `app-v1.1.0` (dc35e93) to keep GPL history and attribution, on branch `main`. The existing `Desktop\Pluely` (`local-unlocked`) and the installed app are left untouched. It is never pushed anywhere unless Sabih asks.

### Native layer (Rust, `src-tauri/`)

**Delete:**
- `api.rs`, `activate.rs` and the whole `speaker/` module.
- The env plumbing in `build.rs`, the updater config, and the `updater`/`keychain`/`posthog`/`http:**` capabilities.
- Crates: updater, keychain, posthog, machine-uid, shell (use `opener`), cpal, ringbuf, once_cell, dotenv, and the mac/linux dependencies.

**New modules:**

```
lib.rs        single-instance (first), logging, state, handlers, setup, exit → session stop
error.rs      AppError {code,message,detail} (thiserror + Serialize)
logging.rs    tracing-subscriber + daily rolling file in app_log_dir (never logs keys/transcripts)
settings.rs   Rust-owned AppSettings (applies hotkeys/window/protection before the webview loads), atomic writes
secrets.rs    keyring v3 with features=["windows-native"] (Windows Credential Manager); MemStore for tests
audio/        frame.rs, level.rs, timeline.rs, resample.rs [pure]; win/{com,devices,notify,control,capture}.rs
stt/          deepgram/{config,protocol,backoff}.rs [pure] + client.rs; turns.rs, echo.rs [pure]
session/      SessionManager + supervisor (spawns both captures + 2 Deepgram tasks, restarts, 10 Hz levels)
screen/       geometry.rs, encode.rs [pure]; capture.rs (xcap), store.rs (last ~10 shots by id)
window/       mod.rs, win32.rs (subclass), placement.rs [pure], dashboard.rs
tray.rs, shortcuts.rs (reuse registration shortcuts.rs:325-448 + lib.rs:145-195 handler, no license gate)
llm/anthropic.rs   SSE proxy, so the Anthropic key never enters the webview
```

**Audio (Windows):**
- **Threads:** one MTA thread per source (`cap-me` for the mic, `cap-them` for loopback), each with a `ComGuard`. A separate `audio-ctl` MTA thread does device listing and `IMMNotificationClient`.
- **Format:** request 16 kHz mono f32 with autoconvert, and a 200 ms buffer.
- **Drain loop:** drain *all* packets per event (`get_next_packet_size`), honour the SILENT flag, and keep QPC timestamps on a shared session clock.
- **Silence:** a 100 ms wait timeout that never exits on silence. 1.5 s of synthetic tail-fill lets Deepgram endpoint after the audio stops.
- **Frames:** 50 ms frames go over `try_send` (`mpsc(64)`) to the Deepgram task. Reconnects get a 90 s backlog.
- **Recovery:** the device follows the default with a 400 ms debounce, and restarts with backoff when the device is lost.
- **Errors:** mic privacy denial is turned into a clear error with a settings link.
- **Workarounds for wasapi bugs:** use `read_from_device`, not the `_to_deque` variant; pass `None` for the AEC endpoint; close event handles.

**Echo:**
- Windows AEC on the mic (setting auto/on/off; it's often unsupported).
- Headphones recommended during onboarding.
- Always-on transcript-level echo flagging (`stt/echo.rs`: "me" finals that overlap and match "them" finals are hidden).

**Deepgram:**
- **Connection:** one `tokio-tungstenite` (native-tls) socket per source, `Authorization: Token …`.
- **Query parameters:** `model=nova-3&encoding=linear16&sample_rate=16000&channels=1&interim_results=true&smart_format=true&punctuate=true&endpointing=300&utterance_end_ms=1000&vad_events=true`, plus `keyterm=` from the brief (names, tech terms).
- **Keep-alive:** a KeepAlive text frame after 4 s idle, and a half-open watchdog.
- **Reconnect:** jittered backoff. 401/402/400 are fatal; 429/5xx/network errors retry.
- **Stop:** CloseStream on stop, then flush the final results.
- **Turns:** `stt/turns.rs` emits exactly one `turn` per speaker turn (on speech_final or UtteranceEnd).

**Contract:**
- **Streams:** sessions and LLM streams run over `ipc::Channel` (ordered, caller-scoped).
- **`SessionEvent` types:** `state`, `device`, `transcript` (interim/final with `t0Ms`/`t1Ms` on one clock), `transcriptPatch` (echo), `speechStarted`, `utteranceEnd`, `turn` (with a `question` hint), `levels` (10 Hz), `error` (code, fatal), `stats`, `snapshot` (reattach after a webview reload).
- **Commands:**
  - `session_start/stop/set_source/status/reattach`
  - `audio_list_devices`
  - `keys_status/set/delete/test`: never returns secrets. The Anthropic test is `GET /v1/models`; the Deepgram test is a real handshake + CloseStream.
  - `llm_stream/llm_cancel`
  - `window_*`
  - `screenshot_full/region_*/get/discard`
  - `settings_*`, `shortcuts_*`, `log_write`
- **Overlay events:** `emit_to("main", …)`, plus a few broadcast `*://` state events.

**Window (Windows):**
- **Built in code:** `focusable(false)`, `always_on_top`, `skip_taskbar`, `content_protected`, transparent, resizable.
- **Alt+Tab:** a `SetWindowSubclass` hook rewrites `WM_STYLECHANGING` (force `WS_EX_TOOLWINDOW`, strip `WS_EX_APPWINDOW`) because tao rewrites the extended style on every change. It also returns `MA_NOACTIVATE` from `WM_MOUSEACTIVATE`.
- **Typing mode:** saves the foreground window, makes the overlay focusable, and on Esc/submit gives focus back to Zoom/Teams.
- **Hide:** a real `hide()`/`show()`.
- **Topmost:** `ensure_topmost` (`SetWindowPos HWND_TOPMOST|SWP_NOACTIVATE`) on show and every 3 s.
- **Click-through:** `set_ignore_cursor_events`; the off switch is a hotkey and the tray.
- **Content protection:** on all windows, including the region-capture overlays.
- **Placement:** saved and clamped to monitor work areas.
- **Shell:** tray menu, single-instance.

**Screenshots:**
- **Monitors:** paired by name (not index), with physical position and size set on the overlay windows (fixes mixed DPI).
- **Crop:** computed in Rust from the monitor's scale factor; `ok_or_else` fix at `capture.rs:209`.
- **Encoding:** downscaled to a long edge of 1568 (a "hi-fi" 2576 option for dense code), RGB JPEG q82, or PNG for small regions.
- **Default target:** the monitor under the cursor.

### Frontend (React + TS + Tailwind + shadcn; `src/` rewritten)

```
src/
  main.tsx                 window router: overlay | dashboard | region-capture
  app/overlay/             TopBar, TranscriptStrip, AnswerStack, AnswerCard, AskBox, Pill
  app/dashboard/           Sessions (list/search/detail/export), Settings, Onboarding wizard
  app/capture/             region selector (port of components/Overlay.tsx, DPI-fixed)
  core/ai/                 claude.ts (SDK client), prompts/ (per-mode), detect.ts, notes.ts, summarize.ts
  core/live/               session store: transcript assembly, turn tracking, question pipeline
  core/db/                 SQLite access + migrations-aware queries
  core/native.ts           typed wrappers for every Tauri command/event (single source of truth)
  core/settings.ts         settings store (SQLite app_settings; keys via native keyring)
  ui/                      kept shadcn components + Markdown renderer
```

**Reused from Pluely:**
- `src/components/ui/*` (shadcn) and `components/Markdown/*` (Streamdown + shiki + KaTeX; also add `@tailwindcss/typography` and actually pass `isStreaming`).
- `CopyButton` / `useCopyToClipboard`.
- `ShortcutRecorder.tsx` and `lib/storage/shortcuts.storage.ts` (validation, conflicts, display).
- The `components/Overlay.tsx` selection logic.
- `ThemeProvider` and the translucent CSS (`global.css:125-136`).
- `lib/platform-instructions.ts` as a seed for mode prompts.
- The `chat-constants.ts` ID helpers.

The 1,050-line `useCompletion`, `useChatCompletion`, `useSystemAudio`, `app.context.tsx` and the cURL/STT template system are **replaced**, not patched.

**State:** small Zustand stores (overlay UI, live session, settings) instead of the 700-line god-context. The two windows sync through Tauri events, not localStorage `storage` events.

### AI layer (Claude: Opus 5.5 everywhere, as Sabih chose)

- **Transport:**
  - The TS side builds request bodies typed with `@anthropic-ai/sdk` types (`MessageCreateParams`, content blocks).
  - Rust `llm/anthropic.rs` sends them as raw HTTPS (Rust has no official SDK): it adds `x-api-key` and `anthropic-version: 2026-10-21`, streams SSE events back over a Channel, and supports cancel.
  - The key never reaches the webview. No `temperature`/`top_p` (400s on that API version).
- **Model:** **`claude-opus-5-5`** for everything. Thinking is always on and not configurable on this model, so `output_config.effort` controls speed:
  - **`low`:** live answers and "What should I say?".
  - **`medium`:** solve-screen, rolling notes and session summaries.
  - Per-task model and effort can be changed in Settings.
- **Single-pass detect + answer** (keeps auto-answers fast without a second model):
  1. The local pre-filter (plus Deepgram's `question` hint) picks likely questions from *Them* turns.
  2. The Opus call is told: "if the latest Them turn isn't a question or request for the user, output exactly `∅`; otherwise answer."
  3. Cards whose stream starts with `∅` are dropped silently.
  4. A newer turn cancels an in-flight one.
- **Streaming everywhere:** `max_tokens` 16k (code answers never cut off; today's cap is 1,024). `stop_reason` is checked (`refusal`, `max_tokens`) before content is shown.
- **Fallbacks:** refusal fallbacks on (`fallbacks: "default"` + beta `server-side-fallback-2026-07-01`).
- **Prompt caching:** `cache_control` on the stable prefix (mode prompt + brief, kept byte-stable); the transcript and question go after it. Checked through `usage.cache_read_input_tokens` in the logs.
- **Structured outputs** (`output_config.format`) for rolling notes and the end-of-session summary JSON.
- **History:** the conversation is sent **in chronological order** with speaker labels ("Them:" / "You:"), capped by a token budget. Errors are mapped to error cards and never saved as answers.

### Data (SQLite via `tauri-plugin-sql`, fresh schema in `src-tauri/src/db`)

- `sessions(id, mode, title, started_at, ended_at, brief_id)`
- `transcript(id, session_id, speaker, text, start_ms, end_ms)` + FTS5
- `cards(id, session_id, kind, question, content, screenshot_path, created_at)` + FTS5
- `notes(session_id, kind, content_json)`
- `briefs(id, name, mode, content, files_json)`
- `mode_prompts(mode, prompt)`
- `app_settings(key, value)`

Screenshots are stored as files under the app data dir, not as base64 in rows.

---

## Build phases

Each phase ends with a working installer (`npx tauri build --bundles nsis`, ~7 min). Cargo builds run one at a time because RAM is limited.

0. **Window spike, about 1 hour.** This is the riskiest unknown, so it goes first. In the current codebase, check whether clicks into a `WS_EX_NOACTIVATE` WebView2 overlay steal focus from Notepad or Zoom, and whether the `WM_STYLECHANGING` subclass keeps the overlay out of Alt+Tab. If clicks do steal focus, fall back to "typing mode only on hotkey, buttons via hotkeys plus restore-foreground".
1. **Foundation** (usable for typed + screen questions):
   - Clone and rename.
   - Strip the server, license, telemetry and updater code.
   - Native keyring, window fixes, screenshot downscale + DPI fix.
   - New overlay shell with AskBox + AnswerCards, the Claude client, the onboarding wizard, and the Ask + Coding "solve screen" flows.
2. **Live listening:**
   - Native mic + loopback → Deepgram streaming → live transcript strip with real meters.
   - Turn tracking, auto question detection, "What should I say?".
3. **Modes & briefs:** the per-mode prompts and behaviours (Meeting rolling notes, Interview STAR + résumé/JD, Lecture key points and jargon); brief profiles with PDF/DOCX text extraction.
4. **Sessions:** persistence, end-of-session summaries per mode, the library with search, Markdown export.
5. **Polish:**
   - Settings page and shortcuts.
   - Click-through, tray, single-instance, out of Alt+Tab, collapse-to-pill.
   - Error UX, empty states, design pass (impeccable skill), final installer.

---

## Verification

- **Unit tests (vitest):**
  - Transcript assembly (interim/final merge, ordering).
  - The local question pre-filter.
  - Prompt builders (chronological order, labels, cache-prefix stability).
  - Error mapping and the DB query layer (against in-memory sqlite).
- **Rust tests:**
  - Pure modules: frame packing, levels, timeline, Deepgram config/protocol (golden JSON fixtures), backoff, turns, echo, monitor/DPI geometry, encode, placement, settings.
  - A **mock Deepgram server** (`accept_hdr_async` on localhost, paused tokio time) checks the auth header, frame cadence, KeepAlive, CloseStream flush, reconnect + backlog replay, and fatal 401.
  - A `FakeCapture` feeds the session pipeline.
  - Hardware tests are `#[ignore]`.
  - A `PLUELYX_DEBUG_DUMP=1` env var writes the exact PCM sent to Deepgram to WAV.
- **UI review:** the overlay is excluded from screen capture, so UI is checked in a browser with Vite dev + a mocked `core/native.ts` (Playwright screenshots of every state: onboarding, listening, answer streaming, error, pill). A dev-only flag turns off content protection so the real app can be screenshotted.
- **End-to-end on this PC** (needs Sabih's Anthropic + Deepgram keys, entered by him in the wizard):
  1. Play a recorded mock interview through the speakers. Check that Them lines appear live, a question gets an auto answer card in ≤2 s, and statements don't trigger answers.
  2. Speak into the mic and check that You lines appear and don't duplicate Them.
  3. Run *Ask about screen* on a LeetCode page; Coding mode solves it.
  4. End the session and check the summary + the library entry + the export.
  5. Screen-share in Teams/Zoom and confirm the overlay is invisible.
- **Native manual checklist:**
  - Overlay buttons don't steal the Notepad caret; Esc in typing mode returns focus.
  - Hiding/showing 20× keeps it out of Alt+Tab; click-through toggles by hotkey.
  - Invisible in Teams/Zoom share, OBS and Snipping Tool.
  - Region crop exact on 150% + 100% monitors; a 4K full shot is <400 KB.
  - Loopback stays live through 60 s of silence; switching the default output mid-session recovers in about 1 s.
  - Wi-Fi off for 10 s → reconnects and backfills; a bad Deepgram key gives one fatal error and no retry loop.
  - Logs contain no `sk-ant-` or Deepgram key.
  - A 2 h run keeps memory and handle count flat.
- **Install:** the NSIS installer installs side-by-side with the existing Pluely; launch, quit and relaunch all work, and a second instance focuses the first.

## Cost and effort notes

- **Running cost:**
  - Deepgram is roughly $0.30–0.60 per listening hour (two streams).
  - Claude Opus 5.5 at $4/$20 per million tokens, with low effort, the local pre-filter and prompt caching, is roughly $0.50–2 per busy hour. This depends on how many turns pass the filter and how long the brief is.
- **Legal note:** recording or transcribing other people can require their consent, depending on where you are. The app shows a one-line reminder in onboarding.
- **Build effort:** large. Expect the work to run across several long sessions; each phase ships an installable build.
