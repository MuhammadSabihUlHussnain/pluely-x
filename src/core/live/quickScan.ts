// "Quick scan & fix": one button that checks everything an answer depends on and repairs
// what it safely can, without ending the conversation. The audio fix restarts each capture
// in turn, which re-opens it on the *current* default devices and reconnects Deepgram.

import type { AudioCheck, CallDevices } from "./audioCheck";

export type ScanStepId =
  | "keys"
  | "claude"
  | "deepgram"
  | "devices"
  | "route"
  | "mode"
  | "capture"
  | "answers"
  | "audio"
  | "shortcuts";

export type ScanStatus = "pending" | "running" | "ok" | "fixed" | "warn" | "fail" | "skipped";

export interface ScanStep {
  id: ScanStepId;
  label: string;
  status: ScanStatus;
  detail?: string;
  /** Where the user can fix what the scan couldn't. */
  action?: "settings" | "use_call_device";
  /** For "use_call_device": the device to switch to. */
  devices?: CallDevices;
}

export interface ScanState {
  running: boolean;
  steps: ScanStep[];
  finishedAt?: number;
}

export const SCAN_IDLE: ScanState = { running: false, steps: [] };

export const STEP_LABELS: Record<ScanStepId, string> = {
  keys: "Claude and Deepgram keys saved",
  claude: "Claude reachable",
  deepgram: "Deepgram (captions) reachable",
  devices: "Microphone and speakers found",
  route: "Listening where calls play",
  mode: "Mode answers automatically",
  capture: "Audio capture restarted on current devices",
  answers: "No stuck answers",
  audio: "Computer audio reaches Pluely X",
  shortcuts: "Shortcuts registered",
};

export const STEP_ORDER: ScanStepId[] = [
  "keys",
  "claude",
  "deepgram",
  "devices",
  "route",
  "mode",
  "capture",
  "answers",
  "audio",
  "shortcuts",
];

export interface ScanDeps {
  keysStatus(): Promise<{ anthropic: boolean; deepgram: boolean }>;
  testKey(provider: "anthropic" | "deepgram"): Promise<{ ok: boolean; message?: string }>;
  devices(): Promise<{ inputs: number; outputs: number }>;
  /** Call app's output vs the captured one when they differ (null when they match or can't tell). */
  callDevice(): Promise<CallDevices | null>;
  listening(): boolean;
  modeAutoAnswers(): { label: string; autoAnswers: boolean };
  /** Turns a source off then on again; resolves once it is capturing again (false on timeout). */
  restartSource(source: "me" | "them"): Promise<boolean>;
  /** Stops answers that have shown nothing for a long time. Returns how many. */
  clearStuckAnswers(): number;
  runAudioCheck(): Promise<AudioCheck>;
  shortcutFailures(): Promise<number>;
  /** Progress callback: called with the full step list after every change. */
  onUpdate(steps: ScanStep[]): void;
}

/** Runs every step in order; a failing step never stops the ones after it. */
export async function runQuickScan(deps: ScanDeps): Promise<ScanStep[]> {
  const steps: ScanStep[] = STEP_ORDER.map((id) => ({ id, label: STEP_LABELS[id], status: "pending" }));
  const put = (id: ScanStepId, patch: Partial<ScanStep>) => {
    const i = steps.findIndex((s) => s.id === id);
    steps[i] = { ...steps[i], ...patch };
    deps.onUpdate(steps.map((s) => ({ ...s })));
  };
  const step = async (id: ScanStepId, fn: () => Promise<Partial<ScanStep>>) => {
    put(id, { status: "running" });
    try {
      put(id, await fn());
    } catch (e) {
      put(id, { status: "fail", detail: errText(e) });
    }
  };

  let keys = { anthropic: false, deepgram: false };
  await step("keys", async () => {
    keys = await deps.keysStatus();
    const missing = [!keys.anthropic && "Claude", !keys.deepgram && "Deepgram"].filter(Boolean);
    return missing.length
      ? { status: "fail", detail: `Missing: ${missing.join(" and ")}. Add it in Settings.`, action: "settings" }
      : { status: "ok" };
  });

  await step("claude", async () => {
    if (!keys.anthropic) return { status: "skipped", detail: "No Claude key." };
    const r = await deps.testKey("anthropic");
    return r.ok
      ? { status: "ok" }
      : { status: "fail", detail: r.message || "Claude rejected the key or couldn't be reached.", action: "settings" };
  });

  await step("deepgram", async () => {
    if (!keys.deepgram) return { status: "skipped", detail: "No Deepgram key." };
    const r = await deps.testKey("deepgram");
    return r.ok
      ? { status: "ok" }
      : { status: "fail", detail: r.message || "Deepgram rejected the key or couldn't be reached.", action: "settings" };
  });

  await step("devices", async () => {
    const d = await deps.devices();
    if (!d.inputs && !d.outputs) return { status: "fail", detail: "No microphone or speakers found." };
    if (!d.inputs) return { status: "fail", detail: "No microphone found." };
    if (!d.outputs) return { status: "fail", detail: "No speakers or headphones found." };
    return { status: "ok" };
  });

  await step("route", async () => {
    const m = await deps.callDevice();
    if (!m) return { status: "ok" };
    return {
      status: "warn",
      detail: `Calls usually play on "${m.call}" (Windows' communications device), but Pluely X listens to "${m.capture}". If the interviewer is on "${m.call}", switch.`,
      action: "use_call_device",
      devices: m,
    };
  });

  await step("mode", async () => {
    const m = deps.modeAutoAnswers();
    return m.autoAnswers
      ? { status: "ok", detail: m.label }
      : {
          status: "warn",
          detail: `${m.label} mode doesn't auto-answer; questions show an Answer button instead. Switch to Interview for automatic answers.`,
        };
  });

  const listening = deps.listening();
  const notListening = { status: "skipped" as const, detail: "Start listening to check this." };

  await step("capture", async () => {
    if (!listening) return notListening;
    const them = await deps.restartSource("them");
    const me = await deps.restartSource("me");
    if (them && me) return { status: "fixed", detail: "Both captures re-opened on the current default devices." };
    const failed = [!them && "computer audio", !me && "microphone"].filter(Boolean).join(" and ");
    return { status: "fail", detail: `Couldn't restart the ${failed}. Stop and start listening.` };
  });

  await step("answers", async () => {
    if (!listening) return notListening;
    const n = deps.clearStuckAnswers();
    return n ? { status: "fixed", detail: `Cleared ${n} stuck answer${n === 1 ? "" : "s"}.` } : { status: "ok" };
  });

  await step("audio", async () => {
    if (!listening) return notListening;
    const v = await deps.runAudioCheck();
    if (v.phase === "ok") return { status: "ok", detail: v.how === "test_tone" ? "Test beep heard." : "Call audio heard." };
    // The beep was heard; where the call plays is the "route" step's warning.
    if (v.phase === "problem" && v.kind === "other_device") return { status: "ok", detail: "Test beep heard." };
    if (v.phase === "problem" && v.kind === "unverified") {
      return { status: "warn", detail: "Couldn't play the test beep; play any sound and watch the Them meter." };
    }
    return {
      status: "fail",
      detail:
        "Computer audio isn't reaching Pluely X. Make sure the call plays through this computer's default speakers and isn't muted.",
    };
  });

  await step("shortcuts", async () => {
    const n = await deps.shortcutFailures();
    return n
      ? { status: "warn", detail: `${n} shortcut${n === 1 ? " is" : "s are"} taken by another app.`, action: "settings" }
      : { status: "ok" };
  });

  return steps;
}

export function scanVerdict(steps: ScanStep[]): "ok" | "warn" | "fail" {
  if (steps.some((s) => s.status === "fail")) return "fail";
  if (steps.some((s) => s.status === "warn")) return "warn";
  return "ok";
}

function errText(e: unknown): string {
  if (e && typeof e === "object" && "message" in e) return String((e as { message: unknown }).message);
  return String(e);
}
