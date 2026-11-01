// Typed wrappers for every command, event and channel in docs/CONTRACT.md.
// This is the only module that imports @tauri-apps/api. In a plain browser
// (no window.__TAURI_INTERNALS__) every call goes to the mock in native.mock.ts.

import { Channel, invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import { openUrl as openerOpenUrl } from "@tauri-apps/plugin-opener";
import { createMockNative } from "./native.mock";
import type {
  AppError,
  AppSettings,
  AudioDevices,
  DeepPartial,
  EventMap,
  EventName,
  ImageData64,
  KeysStatus,
  KeyTestResult,
  LlmEvent,
  LogLevel,
  Provider,
  ScreenshotFullArgs,
  ScreenshotMeta,
  SessionEvent,
  SessionInfo,
  SessionOptions,
  SessionStatus,
  SessionSummary,
  ShortcutsStatus,
  Source,
  WindowState,
} from "./types";

export type Unlisten = () => void;
export type ResizeDirection =
  | "East"
  | "North"
  | "NorthEast"
  | "NorthWest"
  | "South"
  | "SouthEast"
  | "SouthWest"
  | "West";

export interface NativeApi {
  readonly isMock: boolean;
  /** Window label: "main" | "dashboard" | "capture-overlay-<n>" */
  windowLabel(): string;

  // 1. Keys
  keysStatus(): Promise<KeysStatus>;
  keysSet(provider: Provider, value: string): Promise<void>;
  keysDelete(provider: Provider): Promise<void>;
  keysTest(provider: Provider, candidate?: string): Promise<KeyTestResult>;

  // 2. LLM
  llmStream(
    requestId: string,
    body: object,
    betas: string[] | undefined,
    onEvent: (e: LlmEvent) => void,
  ): Promise<void>;
  llmCancel(requestId: string): Promise<void>;

  // 3. Live session
  audioListDevices(): Promise<AudioDevices>;
  audioOpenMicPrivacySettings(): Promise<void>;
  sessionStart(opts: SessionOptions, onEvent: (e: SessionEvent) => void): Promise<SessionInfo>;
  sessionStop(): Promise<SessionSummary>;
  sessionSetSource(args: {
    source: Source;
    enabled?: boolean;
    muted?: boolean;
    deviceId?: string | null;
  }): Promise<void>;
  sessionStatus(): Promise<SessionStatus>;
  sessionReattach(onEvent: (e: SessionEvent) => void): Promise<void>;

  // 4. Window
  windowShow(): Promise<WindowState>;
  windowHide(): Promise<WindowState>;
  windowToggle(): Promise<WindowState>;
  windowGetState(): Promise<WindowState>;
  windowSetSize(size: { width?: number; height?: number }): Promise<WindowState>;
  windowSetClickThrough(enabled: boolean): Promise<WindowState>;
  windowSetContentProtection(enabled: boolean): Promise<WindowState>;
  windowSetAlwaysOnTop(enabled: boolean): Promise<WindowState>;
  windowBeginTyping(): Promise<WindowState>;
  windowEndTyping(restoreFocus: boolean): Promise<WindowState>;
  windowMoveBy(dx: number, dy: number): Promise<WindowState>;
  windowResetPosition(): Promise<WindowState>;
  dashboardOpen(route?: string): Promise<void>;
  dashboardToggle(route?: string): Promise<void>;
  appQuit(): Promise<void>;
  appOpenLogs(): Promise<void>;
  appVersion(): Promise<string>;
  /** Frameless window drag / resize (core:window permissions). */
  startDragging(): Promise<void>;
  startResizeDragging(direction: ResizeDirection): Promise<void>;

  // 5. Screenshots
  screenshotFull(args?: ScreenshotFullArgs): Promise<ScreenshotMeta>;
  screenshotRegionStart(): Promise<void>;
  screenshotRegionCommit(
    monitor: string,
    rect: { x: number; y: number; width: number; height: number },
  ): Promise<ScreenshotMeta>;
  screenshotRegionCancel(): Promise<void>;
  screenshotGet(id: string): Promise<ImageData64>;
  screenshotSave(id: string, sessionId: string): Promise<{ path: string }>;
  screenshotLoad(path: string): Promise<ImageData64>;
  screenshotDiscard(id: string): Promise<void>;

  // 6. Settings
  settingsGet(): Promise<AppSettings>;
  settingsUpdate(patch: DeepPartial<AppSettings>): Promise<AppSettings>;
  settingsReset(section?: keyof AppSettings): Promise<AppSettings>;

  // 7. Shortcuts
  shortcutsValidate(key: string): Promise<boolean>;
  /** Registration state, including failures that happened before the page listened. */
  shortcutsStatus(): Promise<ShortcutsStatus>;

  // 8. Misc
  /** Writes to the app log (target `webview`). Never pass secrets or transcript text. */
  logWrite(level: LogLevel, message: string): Promise<void>;

  // Events (window://state, settings://changed, shortcut://action, ...)
  on<E extends EventName>(event: E, handler: (payload: EventMap[E]) => void): Promise<Unlisten>;

  // Misc
  openUrl(url: string): Promise<void>;
}

/** Normalise anything thrown by invoke() into the contract's AppError shape. */
export function toAppError(e: unknown): AppError {
  if (e && typeof e === "object" && "code" in e && "message" in e) {
    const o = e as { code: unknown; message: unknown; detail?: unknown };
    return {
      code: String(o.code),
      message: String(o.message),
      detail: o.detail == null ? undefined : String(o.detail),
    };
  }
  if (e instanceof Error) return { code: "unknown", message: e.message };
  if (typeof e === "string") return { code: "unknown", message: e };
  return { code: "unknown", message: "Something went wrong" };
}

async function call<T>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  try {
    return await invoke<T>(cmd, args);
  } catch (e) {
    throw toAppError(e);
  }
}

function createTauriNative(): NativeApi {
  return {
    isMock: false,
    windowLabel: () => getCurrentWindow().label,

    keysStatus: () => call("keys_status"),
    keysSet: (provider, value) => call("keys_set", { provider, value }),
    keysDelete: (provider) => call("keys_delete", { provider }),
    keysTest: (provider, candidate) => call("keys_test", { provider, candidate }),

    llmStream: (requestId, body, betas, onEvent) => {
      const ch = new Channel<LlmEvent>();
      ch.onmessage = onEvent;
      return call("llm_stream", { requestId, body, betas, onEvent: ch });
    },
    llmCancel: (requestId) => call("llm_cancel", { requestId }),

    audioListDevices: () => call("audio_list_devices"),
    audioOpenMicPrivacySettings: () => call("audio_open_mic_privacy_settings"),
    sessionStart: (opts, onEvent) => {
      const ch = new Channel<SessionEvent>();
      ch.onmessage = onEvent;
      return call("session_start", { opts, onEvent: ch });
    },
    sessionStop: () => call("session_stop"),
    sessionSetSource: (args) => call("session_set_source", args),
    sessionStatus: () => call("session_status"),
    sessionReattach: (onEvent) => {
      const ch = new Channel<SessionEvent>();
      ch.onmessage = onEvent;
      return call("session_reattach", { onEvent: ch });
    },

    windowShow: () => call("window_show"),
    windowHide: () => call("window_hide"),
    windowToggle: () => call("window_toggle"),
    windowGetState: () => call("window_get_state"),
    windowSetSize: (size) => call("window_set_size", size),
    windowSetClickThrough: (enabled) => call("window_set_click_through", { enabled }),
    windowSetContentProtection: (enabled) => call("window_set_content_protection", { enabled }),
    windowSetAlwaysOnTop: (enabled) => call("window_set_always_on_top", { enabled }),
    windowBeginTyping: () => call("window_begin_typing"),
    windowEndTyping: (restoreFocus) => call("window_end_typing", { restoreFocus }),
    windowMoveBy: (dx, dy) => call("window_move_by", { dx, dy }),
    windowResetPosition: () => call("window_reset_position"),
    dashboardOpen: (route) => call("dashboard_open", { route }),
    dashboardToggle: (route) => call("dashboard_toggle", { route }),
    appQuit: () => call("app_quit"),
    appOpenLogs: () => call("app_open_logs"),
    appVersion: () => call("app_version"),
    startDragging: () => getCurrentWindow().startDragging(),
    startResizeDragging: (direction) =>
      getCurrentWindow().startResizeDragging(
        direction as Parameters<ReturnType<typeof getCurrentWindow>["startResizeDragging"]>[0],
      ),

    screenshotFull: (args) => call("screenshot_full", { ...(args ?? {}) }),
    screenshotRegionStart: () => call("screenshot_region_start"),
    screenshotRegionCommit: (monitor, rect) => call("screenshot_region_commit", { monitor, rect }),
    screenshotRegionCancel: () => call("screenshot_region_cancel"),
    screenshotGet: (id) => call("screenshot_get", { id }),
    screenshotSave: (id, sessionId) => call("screenshot_save", { id, sessionId }),
    screenshotLoad: (path) => call("screenshot_load", { path }),
    screenshotDiscard: (id) => call("screenshot_discard", { id }),

    settingsGet: () => call("settings_get"),
    settingsUpdate: (patch) => call("settings_update", { patch }),
    settingsReset: (section) => call("settings_reset", { section }),

    shortcutsValidate: (key) => call("shortcuts_validate", { key }),
    shortcutsStatus: () => call("shortcuts_status"),

    logWrite: (level, message) => call("log_write", { level, message: message.slice(0, 8000) }),

    on: async (event, handler) => {
      const un = await listen(event, (e) => handler(e.payload as never));
      return un;
    },

    openUrl: (url) => openerOpenUrl(url),
  };
}

export const isTauri =
  typeof window !== "undefined" && "__TAURI_INTERNALS__" in (window as object);

export const native: NativeApi = isTauri ? createTauriNative() : createMockNative();
