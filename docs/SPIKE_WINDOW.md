# Window spike: non-activating overlay on Windows 11 (Step 0)

**Date:** 2026-11-01 · **Machine:** Windows 11 Home 10.0.26200, one 1920×1080 monitor at 125 %, WebView2 (Evergreen) · **Stack:** tauri 2.8.2 / tao 0.34.2 / wry 0.53.1 · **Build:** debug, placeholder frontend (`src-tauri/dev-frontend/`).

## Question

With the overlay built `focusable(false)` (tao adds `WS_EX_NOACTIVATE`), plus a `SetWindowSubclass` hook that returns `MA_NOACTIVATE` on `WM_MOUSEACTIVATE`, forces `WS_EX_TOOLWINDOW`/strips `WS_EX_APPWINDOW` on `WM_STYLECHANGING(GWL_EXSTYLE)` and adds `SWP_NOACTIVATE` on `WM_WINDOWPOSCHANGING` while not typing:

1. Does clicking a button inside the WebView2 content steal the foreground from Notepad?
2. Does the window stay out of Alt+Tab after repeated hide/show?

## Method

An automated PowerShell + user32 harness (per-monitor-DPI-aware, so all coordinates are physical):

1. Start a Notepad instance and make it the foreground window (hold Alt → `SetForegroundWindow` → release Alt, the same trick tao uses; nothing is typed into Notepad).
2. Start the spike exe (`PLUELYX_SPIKE_SUBCLASS=1|0`). The overlay page has a "Click me" button that calls a command logging `GetForegroundWindow()`, and a "Hide/show ×20" button that runs 20 cycles of tauri `hide()`/`show()` 120 ms apart, logging `GWL_EXSTYLE` after each.
3. Click "Click me" three times with `SetCursorPos` + `mouse_event`, after checking with `WindowFromPoint`/`GetAncestor(GA_ROOT)` that the overlay is really under the cursor. Check `GetForegroundWindow()` after each click.
4. Click "Hide/show ×20", wait for it to finish, check the foreground window and the extended style again.
5. Kill both processes; restore the cursor.

"In Alt+Tab" is evaluated by the shell's documented rules: a visible top-level window is listed when it has `WS_EX_APPWINDOW`, or when it has neither `WS_EX_TOOLWINDOW` nor `WS_EX_NOACTIVATE`. (Nobody pressed Alt+Tab on the live desktop.)

## Results

| | Subclass ON (shipping design) | Baseline: `focusable(false)` only |
|---|---|---|
| Ex-style at startup | `0x08000198`: TOOLWINDOW ✓, APPWINDOW ✗, NOACTIVATE ✓ | `0x08040118`: TOOLWINDOW ✗, **APPWINDOW ✓**, NOACTIVATE ✓ |
| Foreground after 3 clicks on a WebView2 button | Notepad (3/3) | Notepad (3/3) |
| Clicks delivered to the page | 3/3 | 3/3 |
| Foreground during/after 20 hide/show cycles | Notepad | Notepad |
| Ex-style after every one of the 20 cycles | unchanged `0x08000198` | unchanged `0x08040118` |
| In Alt+Tab | **No** | **Yes** (APPWINDOW is re-added by tao on every flag change) |
| `GetWindowDisplayAffinity` | `0x11` (WDA_EXCLUDEFROMCAPTURE) | `0x11` |

## Conclusions and chosen approach

- **(a) Clicks do not steal focus.** On this machine `WS_EX_NOACTIVATE` alone is enough: clicking WebView2 content leaves Notepad in the foreground, and tauri `show()` (tao `SW_SHOW`) doesn't activate the window either. The subclass's `MA_NOACTIVATE` and `SWP_NOACTIVATE` handling are kept as belt-and-braces (they cost nothing and cover other WebView2 builds), but they are not what makes it work. **The restore-foreground fallback is not needed** and was not implemented.
- **(b) Alt+Tab needs the subclass.** Without it the overlay is an `APPWINDOW` and shows up in Alt+Tab; with the `WM_STYLECHANGING` rewrite it stays a `TOOLWINDOW` through every hide/show (tao rewrites `GWL_EXSTYLE` on each one, and the hook catches each rewrite).
- Keyboard input still goes to the foreground app, as intended. Typing into the overlay uses **typing mode** (`window_begin_typing` → focusable + `set_focus`, `window_end_typing({restoreFocus})` → non-focusable + `SetForegroundWindow(previous)`), see `src-tauri/src/window/`.
- Implementation: `src-tauri/src/window/win32.rs` (subclass, `ensure_topmost`, foreground helpers) and `window/mod.rs` (build in code with `focusable(false).focused(false).visible(false)`, install the subclass on the main thread before the first `show()`).

## Notes and caveats

- The debug build opens a console window, which became the foreground window in the first run and covered Notepad; later runs started the exe attached to the (hidden) parent console. Release builds have no console.
- Windows 11 Notepad restored its previous session tab (`.env`) when launched; nothing was typed or saved, and the process was killed afterwards.
- Not covered by this spike: other WebView2 runtime versions, a second monitor, Zoom/Teams as the foreground app. The native checklist in `docs/PLAN.md` still applies.

## Re-running

The spike's temporary commands are gone; `src-tauri/dev-frontend/` is now a smoke-runner page that drives the real commands and reports each result through `log_write` as `SMOKE <step> <json>` lines in `%LOCALAPPDATA%\com.sabih.pluelyx\logs\`. Build it with:

```powershell
# from src-tauri, with the placeholder frontend embedded
$env:CARGO_BUILD_JOBS=4; $env:TAURI_CONFIG='{"build":{"frontendDist":"dev-frontend"}}'
cargo build --features tauri/custom-protocol      # -> target\debug\pluelyx.exe
```

The external harness (Notepad as the foreground app, user32 clicks/keys, style and display-affinity checks) was a throwaway script; its steps are listed under Method.

## Follow-up: foundation smoke test (same day, full native layer)

With the real `window/` module the same properties held, and typing mode was added to the checks:

- Overlay at launch: `0x08000198` (tool window, not in Alt+Tab), affinity `0x11`, Notepad keeps the foreground.
- 6 hide/show toggles via the global hotkey: never took focus, style unchanged. (While hidden, `GetWindowDisplayAffinity` reads `0`; it reads `0x11` again as soon as the window is shown.)
- `focus_input` hotkey → overlay becomes foreground (`WS_EX_NOACTIVATE` dropped) → Esc → `window_end_typing({restoreFocus:true})` → Notepad is foreground again, `WS_EX_NOACTIVATE` back.
- Click in the ask box (the click lands in the WebView2 process, not ours) → `window_begin_typing` → overlay is foreground → typed `xyz` arrived in the input → Enter → focus back to Notepad.
- Region-capture overlays and the dashboard also report affinity `0x11`. The capture overlays initially showed up in Alt+Tab (`APPWINDOW`); a style-only subclass (`win32::install_tool_subclass`) now keeps them tool windows while still letting them take focus.
