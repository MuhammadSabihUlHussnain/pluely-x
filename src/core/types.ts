// Types mirrored 1:1 from docs/CONTRACT.md. Keep this file in sync with the contract.

export type Source = "me" | "them";
export type Mode = "meeting" | "interview" | "coding" | "lecture" | "ask";
export type Provider = "anthropic" | "deepgram";
export type Effort = "low" | "medium" | "high";

export const MODES: Mode[] = ["meeting", "interview", "coding", "lecture", "ask"];

export interface AppError {
  code: string;
  message: string;
  detail?: string;
}

// ---- 1. Keys -------------------------------------------------------------
export interface KeyStatus {
  present: boolean;
  hint?: string;
}
export interface KeysStatus {
  anthropic: KeyStatus;
  deepgram: KeyStatus;
}
export interface KeyTestResult {
  ok: boolean;
  code?: string;
  message?: string;
  latencyMs: number;
}

// ---- 2. LLM --------------------------------------------------------------
export interface LlmUsage {
  inputTokens: number;
  outputTokens: number;
  cacheReadInputTokens: number;
  cacheCreationInputTokens: number;
}
export type LlmEvent =
  | { type: "start"; requestId: string; model: string }
  | { type: "text"; delta: string }
  | { type: "thinking" }
  | { type: "stop"; stopReason: string | null; usage: LlmUsage }
  | { type: "error"; code: string; message: string; status?: number; retryable: boolean };

// ---- 3. Live session -----------------------------------------------------
export interface Device {
  id: string;
  name: string;
  isDefault: boolean;
  isDefaultComms: boolean;
}
export interface AudioDevices {
  inputs: Device[];
  outputs: Device[];
}
export type AecSetting = "auto" | "on" | "off";
export type AecState = "on" | "off" | "unsupported";

export interface SessionOptions {
  me: { enabled: boolean; deviceId?: string | null };
  them: { enabled: boolean; deviceId?: string | null };
  stt: {
    language: string;
    keyterms: string[];
    endpointingMs: number;
    utteranceEndMs: number;
    diarizeThem: boolean;
  };
  aec: AecSetting;
  echoDedup: boolean;
}
export interface DeviceInfo {
  deviceId: string;
  name: string;
  format: string;
}
export interface SessionInfo {
  sessionId: string;
  startedAtMs: number;
  me?: DeviceInfo;
  them?: DeviceInfo;
  aec: AecState;
}
export interface SessionSummary {
  sessionId: string;
  durationMs: number;
  reconnects: number;
  droppedMs: number;
}
export interface SessionStatus {
  active: boolean;
  sessionId?: string;
  startedAtMs?: number;
}
export type Level = { rmsDb: number; peakDb: number };
export type SourceState =
  | "connecting"
  | "live"
  | "reconnecting"
  | "idle"
  | "stopping"
  | "stopped"
  | "error";

export interface TranscriptEvent {
  type: "transcript";
  source: Source;
  segId: string;
  conn: number;
  isFinal: boolean;
  speechFinal: boolean;
  fromFinalize: boolean;
  text: string;
  confidence: number;
  t0Ms: number;
  t1Ms: number;
  speaker?: number;
  echo?: boolean;
}
export interface TurnEvent {
  type: "turn";
  source: Source;
  turnId: string;
  text: string;
  t0Ms: number;
  t1Ms: number;
  reason: "speechFinal" | "utteranceEnd" | "stop";
  question: boolean;
}
export type SessionEvent =
  | {
      type: "state";
      source: Source | null;
      state: SourceState;
      attempt?: number;
      retryInMs?: number;
    }
  | {
      type: "device";
      source: Source;
      reason: "opened" | "defaultChanged" | "lost" | "fallback";
      deviceId: string;
      name: string;
      format: string;
      aec?: AecState;
    }
  | TranscriptEvent
  | { type: "transcriptPatch"; segId: string; echo: boolean }
  | { type: "speechStarted"; source: Source; tMs: number }
  | { type: "utteranceEnd"; source: Source; lastWordEndMs: number }
  | TurnEvent
  | { type: "levels"; tMs: number; me: Level | null; them: Level | null }
  | {
      type: "error";
      source?: Source;
      code: string;
      message: string;
      fatal: boolean;
      retryInMs?: number;
    }
  | {
      type: "stats";
      source: Source;
      sentMs: number;
      droppedMs: number;
      reconnects: number;
      discontinuities: number;
    }
  | { type: "snapshot"; status: SessionStatus; recentFinals: SessionEvent[] };

// ---- 4. Window -----------------------------------------------------------
export interface Bounds {
  x: number;
  y: number;
  width: number;
  height: number;
}
export interface WindowState {
  visible: boolean;
  clickThrough: boolean;
  contentProtected: boolean;
  alwaysOnTop: boolean;
  typing: boolean;
  bounds: Bounds;
}

// ---- 5. Screenshots ------------------------------------------------------
export interface ScreenshotMeta {
  id: string;
  mime: "image/jpeg" | "image/png";
  width: number;
  height: number;
  sourceWidth: number;
  sourceHeight: number;
  monitor: string;
  bytes: number;
  thumbDataUrl: string;
}
export interface ScreenshotFullArgs {
  target?: "cursor" | "overlay";
  maxEdge?: number;
  format?: "auto" | "jpeg" | "png";
  quality?: number;
}
export interface ImageData64 {
  mime: string;
  dataBase64: string;
}

// ---- 6. Settings ---------------------------------------------------------
export type ShortcutAction =
  | "toggle_overlay"
  | "toggle_listen"
  | "ask_now"
  | "screenshot_full"
  | "screenshot_region"
  | "focus_input"
  | "toggle_click_through"
  | "toggle_dashboard"
  | "move_up"
  | "move_down"
  | "move_left"
  | "move_right";

export interface ShortcutBinding {
  key: string;
  enabled: boolean;
}

export interface AppSettings {
  version: 1;
  onboarded: boolean;
  mode: Mode;
  overlay: {
    width: number;
    height: number;
    opacity: number;
    clickThrough: boolean;
    contentProtection: boolean;
    alwaysOnTop: boolean;
    launchAtStartup: boolean;
  };
  audio: { micDeviceId: string | null; outputDeviceId: string | null; aec: AecSetting };
  stt: { language: string; endpointingMs: number; utteranceEndMs: number; diarizeThem: boolean };
  ai: { model: string; liveEffort: Effort; deepEffort: Effort; maxTokens: number };
  shortcuts: Record<ShortcutAction, ShortcutBinding>;
}

export type DeepPartial<T> = {
  [K in keyof T]?: T[K] extends object ? DeepPartial<T[K]> : T[K];
};

// ---- 7/8. Events ---------------------------------------------------------
export interface ShortcutFailure {
  action: ShortcutAction;
  key: string;
  error: string;
}

export interface ShortcutsStatus {
  registered: ShortcutAction[];
  failures: ShortcutFailure[];
}

export type LogLevel = "error" | "warn" | "info" | "debug";

export interface EventMap {
  "keys://changed": KeysStatus;
  "dashboard://navigate": { route: string };
  "window://state": WindowState;
  "window://typing": { active: boolean };
  "screenshot://captured": ScreenshotMeta;
  "screenshot://cancelled": Record<string, never>;
  "settings://changed": AppSettings;
  "shortcut://action": { action: ShortcutAction };
  "shortcut://error": { failures: ShortcutFailure[] };
  "session://status": { active: boolean; sessionId?: string };
  "app://error": { code: string; message: string };
}
export type EventName = keyof EventMap;
