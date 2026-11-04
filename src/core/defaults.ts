import type { AppSettings, Mode, ShortcutAction } from "./types";

export const DEFAULT_MODEL = "claude-opus-5-5";

export const DEFAULT_SHORTCUTS: AppSettings["shortcuts"] = {
  toggle_overlay: { key: "ctrl+backslash", enabled: true },
  toggle_listen: { key: "ctrl+alt+l", enabled: true },
  ask_now: { key: "ctrl+alt+enter", enabled: true },
  screenshot_full: { key: "ctrl+alt+s", enabled: true },
  screenshot_region: { key: "ctrl+alt+r", enabled: true },
  focus_input: { key: "ctrl+alt+q", enabled: true },
  toggle_click_through: { key: "ctrl+alt+c", enabled: true },
  toggle_dashboard: { key: "ctrl+alt+d", enabled: true },
  move_up: { key: "ctrl+alt+arrowup", enabled: true },
  move_down: { key: "ctrl+alt+arrowdown", enabled: true },
  move_left: { key: "ctrl+alt+arrowleft", enabled: true },
  move_right: { key: "ctrl+alt+arrowright", enabled: true },
};

export const DEFAULT_SETTINGS: AppSettings = {
  version: 2,
  onboarded: false,
  mode: "ask",
  overlay: {
    width: 440,
    height: 560,
    opacity: 1,
    clickThrough: false,
    contentProtection: true,
    alwaysOnTop: true,
    launchAtStartup: false,
  },
  audio: { micDeviceId: null, outputDeviceId: null, aec: "auto" },
  stt: { language: "en", endpointingMs: 700, utteranceEndMs: 1000, diarizeThem: false },
  ai: { model: DEFAULT_MODEL, liveEffort: "low", deepEffort: "medium", maxTokens: 16000 },
  shortcuts: DEFAULT_SHORTCUTS,
};

export const SHORTCUT_LABELS: Record<ShortcutAction, { label: string; hint: string }> = {
  toggle_overlay: { label: "Show / hide overlay", hint: "Hide the panel completely" },
  toggle_listen: { label: "Start / stop listening", hint: "Transcribe both sides of the call" },
  ask_now: { label: "What should I say?", hint: "Answer from the last two minutes" },
  screenshot_full: { label: "Ask about screen", hint: "Capture the monitor under the cursor" },
  screenshot_region: { label: "Ask about region", hint: "Drag a box to capture" },
  focus_input: { label: "Type a question", hint: "Focus the ask box" },
  toggle_click_through: { label: "Click-through on / off", hint: "Let clicks pass to the app below" },
  toggle_dashboard: { label: "Open dashboard", hint: "Sessions and settings" },
  move_up: { label: "Move overlay up", hint: "Hold to keep moving" },
  move_down: { label: "Move overlay down", hint: "Hold to keep moving" },
  move_left: { label: "Move overlay left", hint: "Hold to keep moving" },
  move_right: { label: "Move overlay right", hint: "Hold to keep moving" },
};

export const MODE_META: Record<
  Mode,
  { label: string; blurb: string; autoAnswers: boolean; rollingNotes: boolean; listens: boolean }
> = {
  meeting: {
    label: "Meeting",
    blurb: "Suggests replies when you're asked something; keeps decisions and action items",
    autoAnswers: true,
    rollingNotes: true,
    listens: true,
  },
  interview: {
    label: "Interview",
    blurb: "Answers the interviewer's questions, grounded in your résumé and the job",
    autoAnswers: true,
    rollingNotes: false,
    listens: true,
  },
  coding: {
    label: "Coding",
    blurb: "Solves what's on screen: approach, code, complexity, edge cases",
    autoAnswers: true,
    rollingNotes: false,
    listens: true,
  },
  lecture: {
    label: "Lecture",
    blurb: "No auto-answers. Rolling key points and terms; ask anything",
    autoAnswers: false,
    rollingNotes: true,
    listens: true,
  },
  ask: {
    label: "Ask",
    blurb: "Plain screen and question assistant, no listening needed",
    autoAnswers: false,
    rollingNotes: false,
    listens: false,
  },
};

/** Model choices offered in Settings. Opus 5.5 is the default for everything. */
export const MODEL_OPTIONS = [
  { id: "claude-opus-5-5", label: "Claude Opus 5.5", note: "Default · best answers" },
  { id: "claude-sonnet-5-5", label: "Claude Sonnet 5.5", note: "Faster, cheaper" },
  { id: "claude-haiku-5-5", label: "Claude Haiku 5.5", note: "Fastest" },
];
