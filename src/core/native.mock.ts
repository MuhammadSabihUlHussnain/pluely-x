// Browser mock of the native layer (used when window.__TAURI_INTERNALS__ is
// absent, e.g. `npm run dev` in a normal browser). It simulates a scripted
// call with interim→final transcripts for both speakers, turns with question
// hints, 10 Hz levels, an echo patch and a reconnect blip; streams canned
// Claude answers that react to the request (including ∅ for non-questions);
// fake screenshots; and key tests that pass for keys starting with
// "sk-ant-" (Anthropic) or "dg" (Deepgram).
//
// Dev controls: window.__pluelyxMock.failNext = "key_invalid" makes the next
// LLM request fail with that code; a question containing "[mock:rate_limited]"
// does the same. Append ?fresh to the URL to reset to a first-run state, and
// ?conflicts to simulate shortcuts another app already owns (as on the dev PC).

import { ASK_NOW_INSTRUCTION, FOLLOWUP_INSTRUCTION } from "./ai/prompts";
import { DEFAULT_SETTINGS } from "./defaults";
import { MOCK_NOTES, MOCK_SUMMARIES, SCREEN_ANSWER_CODING, SCREEN_ANSWER_GENERIC, SCRIPTS, cannedTypedAnswer, type ScriptLine } from "./mock/script";
import { makeFakeScreenshot } from "./mock/screens";
import type { NativeApi, Unlisten } from "./native";
import { acceleratorFromEvent, normalizeAccelerator } from "./shortcuts";
import type {
  AppSettings,
  AudioDevices,
  EventMap,
  EventName,
  KeysStatus,
  LlmEvent,
  Mode,
  ScreenshotMeta,
  SessionEvent,
  ShortcutAction,
  Source,
  WindowState,
} from "./types";
import { deepMerge, sleep } from "./util";

const LS_SETTINGS = "pluelyx.mock.settings";
const LS_KEYS = "pluelyx.mock.keys";
const BROADCAST: EventName[] = ["settings://changed", "window://state", "session://status", "keys://changed"];

export interface MockControls {
  failNext: string | null;
  /** Multiplies every simulated delay (lower = faster). */
  speed: number;
  emit<E extends EventName>(event: E, payload: EventMap[E]): void;
  /** Feeds a SessionEvent into the running mock session (e.g. "me" transcripts in tests). */
  inject(ev: SessionEvent): void;
}

function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}
function writeJson(key: string, value: unknown) {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    /* storage unavailable */
  }
}

const RETRYABLE = new Set(["rate_limited", "overloaded", "server", "network"]);
const ERROR_MESSAGES: Record<string, string> = {
  key_missing: "No Anthropic key is stored.",
  key_invalid: "invalid x-api-key",
  rate_limited: "Number of requests has exceeded your rate limit.",
  overloaded: "Overloaded",
  server: "Internal server error",
  network: "error sending request: connection refused",
  bad_request: "messages: text content blocks must be non-empty",
};

const DEVICES: AudioDevices = {
  inputs: [
    { id: "mic-realtek", name: "Microphone Array (Realtek(R) Audio)", isDefault: true, isDefaultComms: false },
    { id: "mic-jabra", name: "Headset Microphone (Jabra Evolve2 65)", isDefault: false, isDefaultComms: true },
  ],
  outputs: [
    { id: "out-realtek", name: "Speakers (Realtek(R) Audio)", isDefault: true, isDefaultComms: false },
    { id: "out-jabra", name: "Headphones (Jabra Evolve2 65)", isDefault: false, isDefaultComms: true },
  ],
};

type AnyBody = {
  model?: string;
  system?: { text: string }[];
  messages?: { role: string; content: string | { type: string; text?: string }[] }[];
  output_config?: { format?: { schema?: { properties?: Record<string, unknown> } } };
};

function textOf(content: string | { type: string; text?: string }[] | undefined): string {
  if (!content) return "";
  if (typeof content === "string") return content;
  return content
    .filter((b) => b.type === "text")
    .map((b) => b.text ?? "")
    .join("\n");
}

const ALL_LINES: ScriptLine[] = Object.values(SCRIPTS).flat();

function pickStructured(body: AnyBody): string {
  const props = Object.keys(body.output_config?.format?.schema?.properties ?? {});
  const has = (k: string) => props.includes(k);
  if (has("follow_up_email")) return JSON.stringify(MOCK_SUMMARIES.meeting);
  if (has("decisions")) return JSON.stringify(MOCK_NOTES.meeting);
  if (has("study_notes")) return JSON.stringify(MOCK_SUMMARIES.lecture);
  if (has("key_points")) return JSON.stringify(MOCK_NOTES.lecture);
  if (has("tips")) return JSON.stringify(MOCK_SUMMARIES.interview);
  if (has("problems")) return JSON.stringify(MOCK_SUMMARIES.coding);
  return JSON.stringify(MOCK_SUMMARIES.ask);
}

function pickAnswer(body: AnyBody): string {
  if (body.output_config?.format) return pickStructured(body);
  const system = (body.system ?? []).map((b) => b.text).join("\n");
  const mode = (/Mode: (\w+)\./.exec(system)?.[1] ?? "Ask").toLowerCase() as Mode;
  const msgs = body.messages ?? [];
  const last = msgs[msgs.length - 1];
  const lastText = textOf(last?.content);
  const hasImage = Array.isArray(last?.content) && last.content.some((b) => b.type === "image");

  const turn = /<latest_them_turn>\n([\s\S]*?)\n<\/latest_them_turn>/.exec(lastText)?.[1]?.trim();
  if (turn !== undefined) {
    return ALL_LINES.find((l) => l.text === turn)?.answer ?? "∅";
  }
  if (lastText.includes(ASK_NOW_INSTRUCTION.slice(0, 30))) {
    const answered = ALL_LINES.filter((l) => l.answer && lastText.includes(l.text));
    const hit = answered[answered.length - 1];
    return (
      hit?.answer ??
      `I'd say: “That makes sense — can you tell me a bit more about what success looks like for this?”
- It keeps the conversation moving while you gather context.
- It shows you're thinking about outcomes, not just tasks.`
    );
  }
  const followup = (Object.entries(FOLLOWUP_INSTRUCTION) as [string, string][]).find(([, v]) => lastText.includes(v));
  if (followup) {
    const parent = textOf([...msgs].reverse().find((m) => m.role === "assistant")?.content);
    const [head, ...rest] = parent.split("\n");
    const bullets = rest.filter((l) => /^\s*-\s/.test(l));
    switch (followup[0]) {
      case "shorter":
        return [head, ...bullets.slice(0, 2)].join("\n");
      case "more":
        return `${head}\n${bullets.join("\n")}\n\nDetails\nGoing one level deeper: lead with the outcome, then explain the constraint that made the problem hard, and close with what you would do differently with more time. Interviewers listen for the *why* behind each decision, so name the alternative you rejected and the reason.`;
      case "example":
        return `For example: “When our reports were 15 minutes stale, I moved ingestion to a streaming pipeline and got freshness under two seconds.”\n- It shows the problem, your action and the measurable result in one breath.\n- Swap in your own numbers from the brief.`;
      default:
        return "…and that's the rest of the answer: keep it to the result and one lesson learned.";
    }
  }
  if (hasImage) return mode === "coding" ? SCREEN_ANSWER_CODING : SCREEN_ANSWER_GENERIC;
  const q = /<question>\n([\s\S]*?)\n<\/question>/.exec(lastText)?.[1] ?? lastText;
  return cannedTypedAnswer(q);
}

export function createMockNative(): NativeApi {
  const hasWindow = typeof window !== "undefined";
  const fresh = hasWindow && new URLSearchParams(window.location.search).has("fresh");
  const conflicts = hasWindow && new URLSearchParams(window.location.search).has("conflicts");
  if (fresh) {
    try {
      localStorage.removeItem(LS_SETTINGS);
      localStorage.removeItem(LS_KEYS);
      localStorage.removeItem("pluelyx.mock.db");
    } catch {
      /* ignore */
    }
  }

  // ---- state ---------------------------------------------------------------
  // A normal (non-?fresh) visit behaves like a returning user: onboarded, keys saved.
  let settings: AppSettings = deepMerge(
    deepMerge(DEFAULT_SETTINGS, fresh ? {} : { onboarded: true }),
    hasWindow ? readJson<Partial<AppSettings>>(LS_SETTINGS, {}) : {},
  );
  let keys: KeysStatus = hasWindow
    ? readJson<KeysStatus>(
        LS_KEYS,
        fresh
          ? { anthropic: { present: false }, deepgram: { present: false } }
          : { anthropic: { present: true, hint: "…a1b2" }, deepgram: { present: true, hint: "…9f3c" } },
      )
    : { anthropic: { present: false }, deepgram: { present: false } };

  let win: WindowState = {
    visible: true,
    clickThrough: settings.overlay.clickThrough,
    contentProtected: settings.overlay.contentProtection,
    alwaysOnTop: settings.overlay.alwaysOnTop,
    typing: false,
    bounds: { x: 740, y: 40, width: settings.overlay.width, height: settings.overlay.height },
  };

  const label = (): string => {
    if (!hasWindow) return "main";
    const p = window.location.pathname;
    if (p.startsWith("/dashboard")) return "dashboard";
    if (p.startsWith("/capture")) return "capture-overlay-0";
    return "main";
  };

  // ---- events --------------------------------------------------------------
  const listeners = new Map<string, Set<(p: unknown) => void>>();
  const bc = hasWindow && "BroadcastChannel" in window ? new BroadcastChannel("pluelyx-mock") : null;
  bc?.addEventListener("message", (m: MessageEvent<{ event: string; payload: unknown }>) => {
    if (m.data.event === "settings://changed") settings = m.data.payload as AppSettings;
    if (m.data.event === "window://state") win = m.data.payload as WindowState;
    listeners.get(m.data.event)?.forEach((h) => h(m.data.payload));
  });
  const emit = <E extends EventName>(event: E, payload: EventMap[E]) => {
    listeners.get(event)?.forEach((h) => h(payload));
    if (BROADCAST.includes(event)) bc?.postMessage({ event, payload });
  };

  const controls: MockControls = {
    failNext: null,
    speed: 1,
    emit,
    inject: (ev) => {
      if (session && !session.stopped) session.onEvent(ev);
    },
  };
  if (hasWindow) (window as unknown as { __pluelyxMock: MockControls }).__pluelyxMock = controls;
  const wait = (ms: number) => sleep(ms * controls.speed);

  const setWin = (patch: Partial<WindowState>): WindowState => {
    win = { ...win, ...patch };
    emit("window://state", win);
    return win;
  };

  // ---- LLM -----------------------------------------------------------------
  const streams = new Map<string, { cancelled: boolean; onEvent: (e: LlmEvent) => void; resolve: () => void }>();
  const seenSystems = new Set<string>();

  async function runLlm(requestId: string, body: AnyBody) {
    const st = streams.get(requestId)!;
    const lastText = textOf(body.messages?.[body.messages.length - 1]?.content);
    const tagged = /\[mock:([a-z_]+)\]/.exec(lastText)?.[1] ?? null;
    const fail = controls.failNext ?? tagged;
    await wait(250);
    if (st.cancelled) return;
    if (fail) {
      controls.failNext = null;
      st.onEvent({
        type: "error",
        code: fail,
        message: ERROR_MESSAGES[fail] ?? "Mock failure",
        status: fail === "key_invalid" ? 401 : fail === "rate_limited" ? 429 : undefined,
        retryable: RETRYABLE.has(fail),
      });
      return;
    }
    st.onEvent({ type: "start", requestId, model: body.model ?? "claude-opus-5-5" });
    const structured = !!body.output_config?.format;
    for (let i = 0; i < (structured ? 6 : 3); i++) {
      await wait(160);
      if (st.cancelled) return;
      st.onEvent({ type: "thinking" });
    }
    const text = pickAnswer(body);
    const tokens = text.split(/(\s+)/);
    for (let i = 0; i < tokens.length; i += 3) {
      await wait(structured ? 8 : 28);
      if (st.cancelled) return;
      st.onEvent({ type: "text", delta: tokens.slice(i, i + 3).join("") });
    }
    const sysKey = JSON.stringify(body.system ?? "");
    const cached = seenSystems.has(sysKey);
    seenSystems.add(sysKey);
    const sysTokens = Math.round(sysKey.length / 4);
    st.onEvent({
      type: "stop",
      stopReason: "end_turn",
      usage: {
        inputTokens: Math.round(JSON.stringify(body.messages ?? "").length / 4),
        outputTokens: Math.round(text.length / 4),
        cacheReadInputTokens: cached ? sysTokens : 0,
        cacheCreationInputTokens: cached ? 0 : sysTokens,
      },
    });
  }

  // ---- session ---------------------------------------------------------------
  interface MockSession {
    id: string;
    startedAt: number;
    t0: number;
    onEvent: (e: SessionEvent) => void;
    timers: number[];
    stopped: boolean;
    speaking: Record<Source, boolean>;
    finals: SessionEvent[];
    turnN: number;
  }
  let session: MockSession | null = null;

  const send = (s: MockSession, e: SessionEvent) => {
    if (e.type === "transcript" && e.isFinal) s.finals.push(e);
    if (e.type === "transcriptPatch") s.finals.push(e);
    s.onEvent(e);
  };
  const clock = (s: MockSession) => Math.round(performance.now() - s.t0);

  function level(speaking: boolean) {
    if (!speaking) {
      const rms = -64 + Math.random() * 6;
      return { rmsDb: rms, peakDb: rms + 4 };
    }
    const rms = -30 + Math.random() * 16;
    return { rmsDb: rms, peakDb: Math.min(-2, rms + 6 + Math.random() * 6) };
  }

  async function speak(s: MockSession, line: ScriptLine) {
    const sentences = line.text.split(/(?<=[.?!])\s+/);
    let lastSegId = "";
    for (const sentence of sentences) {
      if (s.stopped) return;
      const t0 = clock(s);
      const segId = `${line.source}-1-${t0}`;
      lastSegId = segId;
      const words = sentence.split(" ");
      s.speaking[line.source] = true;
      send(s, { type: "speechStarted", source: line.source, tMs: t0 });
      for (let i = 2; i < words.length; i += 2) {
        await wait(170);
        if (s.stopped) return;
        send(s, {
          type: "transcript",
          source: line.source,
          segId,
          conn: 1,
          isFinal: false,
          speechFinal: false,
          fromFinalize: false,
          text: words.slice(0, i).join(" ").replace(/[.,?!]$/, ""),
          confidence: 0.82,
          t0Ms: t0,
          t1Ms: clock(s),
        });
      }
      await wait(260);
      if (s.stopped) return;
      send(s, {
        type: "transcript",
        source: line.source,
        segId,
        conn: 1,
        isFinal: true,
        speechFinal: sentence === sentences[sentences.length - 1],
        fromFinalize: false,
        text: sentence,
        confidence: 0.96,
        t0Ms: t0,
        t1Ms: clock(s),
      });
    }
    s.speaking[line.source] = false;
    await wait(220);
    if (s.stopped) return;
    s.turnN += 1;
    send(s, {
      type: "turn",
      source: line.source,
      turnId: `turn-${line.source}-${s.turnN}`,
      text: line.text,
      t0Ms: Math.max(0, clock(s) - line.text.length * 55),
      t1Ms: clock(s),
      reason: "speechFinal",
      question: line.text.trim().endsWith("?"),
    });
    if (line.echo && lastSegId) {
      // The mic picks up the speaker audio: an echo final, patched shortly after.
      await wait(250);
      if (s.stopped) return;
      const t0 = clock(s) - 1200;
      const echoId = `me-1-${t0}`;
      const tail = line.text.split(" ").slice(-6).join(" ");
      send(s, {
        type: "transcript",
        source: "me",
        segId: echoId,
        conn: 1,
        isFinal: true,
        speechFinal: true,
        fromFinalize: false,
        text: tail,
        confidence: 0.6,
        t0Ms: t0,
        t1Ms: clock(s),
      });
      await wait(600);
      if (!s.stopped) send(s, { type: "transcriptPatch", segId: echoId, echo: true });
    }
  }

  async function playScript(s: MockSession, lines: ScriptLine[]) {
    for (const line of lines) {
      await wait(line.pauseMs);
      if (s.stopped) return;
      await speak(s, line);
    }
  }

  function startTimers(s: MockSession) {
    const every = (ms: number, fn: () => void) => s.timers.push(window.setInterval(fn, ms));
    const after = (ms: number, fn: () => void) => s.timers.push(window.setTimeout(fn, ms * controls.speed));
    every(100, () =>
      send(s, { type: "levels", tMs: clock(s), me: level(s.speaking.me), them: level(s.speaking.them) }),
    );
    every(5000, () => {
      for (const source of ["me", "them"] as Source[])
        send(s, { type: "stats", source, sentMs: clock(s), droppedMs: 0, reconnects: 0, discontinuities: 0 });
    });
    after(16000, () => {
      send(s, {
        type: "error",
        source: "them",
        code: "dg_network",
        message: "Connection to Deepgram dropped",
        fatal: false,
        retryInMs: 800,
      });
      send(s, { type: "state", source: "them", state: "reconnecting", attempt: 1, retryInMs: 800 });
      after(1400, () => send(s, { type: "state", source: "them", state: "live" }));
    });
  }

  // ---- screenshots -----------------------------------------------------------
  const shots = new Map<string, { meta: ScreenshotMeta; dataUrl: string }>();
  const saved = new Map<string, string>();
  const makeShot = (region: boolean): ScreenshotMeta => {
    const fake = makeFakeScreenshot(region);
    const id = `shot_${Date.now()}_${Math.random().toString(36).slice(2, 7)}`;
    const meta: ScreenshotMeta = {
      id,
      mime: "image/jpeg",
      width: fake.width,
      height: fake.height,
      sourceWidth: region ? 1140 : 2560,
      sourceHeight: region ? 780 : 1440,
      monitor: "\\\\.\\DISPLAY1",
      bytes: Math.round((fake.dataUrl.length * 3) / 4),
      thumbDataUrl: fake.thumbDataUrl,
    };
    shots.set(id, { meta, dataUrl: fake.dataUrl });
    return meta;
  };
  const b64 = (dataUrl: string) => dataUrl.slice(dataUrl.indexOf(",") + 1);

  // ---- settings side effects --------------------------------------------------
  const applySettings = (next: AppSettings, prev: AppSettings) => {
    settings = next;
    writeJson(LS_SETTINGS, settings);
    emit("settings://changed", settings);
    const o = next.overlay;
    if (
      o.clickThrough !== prev.overlay.clickThrough ||
      o.contentProtection !== prev.overlay.contentProtection ||
      o.alwaysOnTop !== prev.overlay.alwaysOnTop
    ) {
      setWin({ clickThrough: o.clickThrough, contentProtected: o.contentProtection, alwaysOnTop: o.alwaysOnTop });
    }
    // Simulate a registration failure for a combo another app owns.
    const failures = (Object.keys(next.shortcuts) as ShortcutAction[])
      .filter((a) => next.shortcuts[a].enabled && normalizeAccelerator(next.shortcuts[a].key) === "ctrl+shift+m")
      .map((a) => ({ action: a, key: next.shortcuts[a].key, error: "Already registered by another application (Teams)" }));
    if (failures.length) setTimeout(() => emit("shortcut://error", { failures }), 50);
    return settings;
  };

  // ---- simulated global shortcuts (overlay tab only) ----------------------------
  if (hasWindow && label() === "main") {
    window.addEventListener(
      "keydown",
      (e) => {
        const acc = acceleratorFromEvent(e);
        if (!acc || !(e.ctrlKey || e.altKey)) return;
        const action = (Object.keys(settings.shortcuts) as ShortcutAction[]).find(
          (a) => settings.shortcuts[a].enabled && normalizeAccelerator(settings.shortcuts[a].key) === acc,
        );
        if (!action) return;
        e.preventDefault();
        if (action === "toggle_click_through") {
          setWin({ clickThrough: !win.clickThrough });
          return;
        }
        if (action === "toggle_overlay" || action.startsWith("move_")) return;
        if (action === "toggle_dashboard") {
          window.open("/dashboard", "pluelyx-dashboard");
          return;
        }
        if (action === "focus_input") {
          setWin({ typing: true });
          emit("window://typing", { active: true });
        }
        emit("shortcut://action", { action });
      },
      true,
    );
  }

  // ---- the API ----------------------------------------------------------------
  const api: NativeApi = {
    isMock: true,
    windowLabel: label,

    keysStatus: async () => {
      await wait(60);
      return keys;
    },
    keysSet: async (provider, value) => {
      await wait(120);
      const v = value.trim();
      if (!v || /\s/.test(v) || v.length > 512) throw { code: "key_invalid", message: "That doesn't look like an API key." };
      keys = { ...keys, [provider]: { present: true, hint: `…${v.slice(-4)}` } };
      writeJson(LS_KEYS, keys);
      emit("keys://changed", keys);
    },
    keysDelete: async (provider) => {
      await wait(80);
      keys = { ...keys, [provider]: { present: false } };
      writeJson(LS_KEYS, keys);
      emit("keys://changed", keys);
    },
    keysTest: async (provider, candidate) => {
      const latency = provider === "anthropic" ? 280 + Math.round(Math.random() * 200) : 420 + Math.round(Math.random() * 200);
      await wait(latency);
      const value = candidate?.trim();
      if (value === undefined) {
        return keys[provider].present
          ? { ok: true, latencyMs: latency }
          : { ok: false, code: "key_missing", message: "No key is stored yet.", latencyMs: 0 };
      }
      const good = provider === "anthropic" ? value.startsWith("sk-ant-") : value.toLowerCase().startsWith("dg");
      return good
        ? { ok: true, latencyMs: latency }
        : {
            ok: false,
            code: "key_invalid",
            message: provider === "anthropic" ? "Anthropic rejected this key (401)." : "Deepgram rejected this key (401).",
            latencyMs: latency,
          };
    },

    llmStream: (requestId, body, _betas, onEvent) =>
      new Promise<void>((resolve) => {
        streams.set(requestId, { cancelled: false, onEvent, resolve });
        runLlm(requestId, body as AnyBody).finally(() => {
          streams.delete(requestId);
          resolve();
        });
      }),
    llmCancel: async (requestId) => {
      const st = streams.get(requestId);
      if (!st || st.cancelled) return;
      st.cancelled = true;
      st.onEvent({ type: "error", code: "cancelled", message: "Cancelled", retryable: false });
      streams.delete(requestId);
      st.resolve();
    },

    audioListDevices: async () => {
      await wait(80);
      return DEVICES;
    },
    audioOpenMicPrivacySettings: async () => {},

    sessionStart: async (opts, onEvent) => {
      if (session && !session.stopped) throw { code: "session_active", message: "A session is already running." };
      if (!keys.deepgram.present) throw { code: "key_missing", message: "Add a Deepgram key to start listening." };
      await wait(350);
      const s: MockSession = {
        id: `ses_${Date.now()}_${Math.random().toString(36).slice(2, 9)}`,
        startedAt: Date.now(),
        t0: performance.now(),
        onEvent,
        timers: [],
        stopped: false,
        speaking: { me: false, them: false },
        finals: [],
        turnN: 0,
      };
      session = s;
      const mic = DEVICES.inputs.find((d) => d.id === opts.me.deviceId) ?? DEVICES.inputs[1];
      const out = DEVICES.outputs.find((d) => d.id === opts.them.deviceId) ?? DEVICES.outputs[0];
      for (const source of ["me", "them"] as Source[]) {
        send(s, { type: "state", source, state: "connecting" });
        const d = source === "me" ? mic : out;
        send(s, { type: "device", source, reason: "opened", deviceId: d.id, name: d.name, format: "16 kHz mono" });
      }
      s.timers.push(
        window.setTimeout(() => {
          send(s, { type: "state", source: "me", state: "live" });
          send(s, { type: "state", source: "them", state: "live" });
        }, 450),
      );
      startTimers(s);
      const script = !settings.onboarded
        ? SCRIPTS.audioCheck
        : SCRIPTS[(settings.mode === "ask" ? "interview" : settings.mode) as keyof typeof SCRIPTS];
      void playScript(s, script);
      emit("session://status", { active: true, sessionId: s.id });
      return {
        sessionId: s.id,
        startedAtMs: s.startedAt,
        me: { deviceId: mic.id, name: mic.name, format: "16 kHz mono f32" },
        them: { deviceId: out.id, name: out.name, format: "16 kHz mono f32 (loopback)" },
        aec: "unsupported",
      };
    },
    sessionStop: async () => {
      const s = session;
      if (!s || s.stopped) return { sessionId: s?.id ?? "", durationMs: 0, reconnects: 0, droppedMs: 0 };
      send(s, { type: "state", source: null, state: "stopping" });
      s.stopped = true;
      s.timers.forEach((t) => {
        clearInterval(t);
        clearTimeout(t);
      });
      await wait(300);
      s.onEvent({ type: "state", source: null, state: "stopped" });
      emit("session://status", { active: false });
      return { sessionId: s.id, durationMs: Date.now() - s.startedAt, reconnects: 1, droppedMs: 0 };
    },
    sessionSetSource: async () => {},
    sessionStatus: async () =>
      session && !session.stopped
        ? { active: true, sessionId: session.id, startedAtMs: session.startedAt }
        : { active: false },
    sessionReattach: async (onEvent) => {
      const s = session;
      if (s && !s.stopped) {
        s.onEvent = onEvent;
        onEvent({
          type: "snapshot",
          status: { active: true, sessionId: s.id, startedAtMs: s.startedAt },
          recentFinals: s.finals.slice(-80),
        });
      } else {
        onEvent({ type: "snapshot", status: { active: false }, recentFinals: [] });
      }
    },

    windowShow: async () => setWin({ visible: true }),
    windowHide: async () => setWin({ visible: false }),
    windowToggle: async () => setWin({ visible: !win.visible }),
    windowGetState: async () => win,
    windowSetSize: async ({ width, height }) =>
      setWin({ bounds: { ...win.bounds, width: width ?? win.bounds.width, height: height ?? win.bounds.height } }),
    windowSetClickThrough: async (enabled) => {
      settings = deepMerge(settings, { overlay: { clickThrough: enabled } });
      return setWin({ clickThrough: enabled });
    },
    windowSetContentProtection: async (enabled) => {
      applySettings(deepMerge(settings, { overlay: { contentProtection: enabled } }), settings);
      return setWin({ contentProtected: enabled });
    },
    windowSetAlwaysOnTop: async (enabled) => setWin({ alwaysOnTop: enabled }),
    windowBeginTyping: async () => {
      if (!win.typing) emit("window://typing", { active: true });
      return setWin({ typing: true });
    },
    windowEndTyping: async () => {
      if (win.typing) emit("window://typing", { active: false });
      return setWin({ typing: false });
    },
    windowMoveBy: async (dx, dy) => setWin({ bounds: { ...win.bounds, x: win.bounds.x + dx, y: win.bounds.y + dy } }),
    windowResetPosition: async () => setWin({ bounds: { ...win.bounds, x: 740, y: 40 } }),
    dashboardOpen: async (route) => {
      if (hasWindow) window.open(route ?? "/dashboard", "pluelyx-dashboard");
    },
    dashboardToggle: async (route) => {
      if (hasWindow) window.open(route ?? "/dashboard", "pluelyx-dashboard");
    },
    appQuit: async () => {},
    appOpenLogs: async () => {},
    appVersion: async () => "0.1.0 (browser mock)",
    startDragging: async () => {},
    startResizeDragging: async () => {},

    screenshotFull: async () => {
      await wait(260);
      return makeShot(false);
    },
    screenshotRegionStart: async () => {
      // No capture windows in a browser: pretend the user dragged a box.
      setTimeout(() => emit("screenshot://captured", makeShot(true)), 900 * controls.speed);
    },
    screenshotRegionCommit: async () => {
      const meta = makeShot(true);
      emit("screenshot://captured", meta);
      return meta;
    },
    screenshotRegionCancel: async () => emit("screenshot://cancelled", {}),
    screenshotGet: async (id) => {
      const s = shots.get(id);
      if (!s) throw { code: "not_found", message: "Screenshot expired" };
      return { mime: s.meta.mime, dataBase64: b64(s.dataUrl) };
    },
    screenshotSave: async (id, sessionId) => {
      const s = shots.get(id);
      const path = `mock/screenshots/${sessionId}/${id}.jpg`;
      if (s) saved.set(path, s.dataUrl);
      return { path };
    },
    screenshotLoad: async (path) => {
      const dataUrl = saved.get(path) ?? makeFakeScreenshot(path.includes("region")).dataUrl;
      return { mime: "image/jpeg", dataBase64: b64(dataUrl) };
    },
    screenshotDiscard: async (id) => {
      shots.delete(id);
    },

    settingsGet: async () => settings,
    settingsUpdate: async (patch) => applySettings(deepMerge(settings, patch), settings),
    settingsReset: async (section) => {
      const next = section
        ? ({ ...settings, [section]: DEFAULT_SETTINGS[section] } as AppSettings)
        : { ...DEFAULT_SETTINGS, onboarded: settings.onboarded };
      return applySettings(next, settings);
    },

    shortcutsStatus: async () => {
      const all = Object.keys(settings.shortcuts) as ShortcutAction[];
      const taken = conflicts ? ["ctrl+backslash", "ctrl+alt+q"] : [];
      const failures = all
        .filter((a) => settings.shortcuts[a].enabled && taken.includes(normalizeAccelerator(settings.shortcuts[a].key)))
        .map((a) => ({ action: a, key: settings.shortcuts[a].key, error: "HotKey already registered" }));
      return { registered: all.filter((a) => settings.shortcuts[a].enabled && !failures.some((f) => f.action === a)), failures };
    },
    logWrite: async () => {},

    shortcutsValidate: async (key) => /^(ctrl|alt|shift|super)(\+(ctrl|alt|shift|super))*\+[a-z0-9]+$/.test(normalizeAccelerator(key)),

    on: async (event, handler) => {
      const set = listeners.get(event) ?? new Set();
      set.add(handler as (p: unknown) => void);
      listeners.set(event, set);
      const un: Unlisten = () => set.delete(handler as (p: unknown) => void);
      return un;
    },

    openUrl: async (url) => {
      if (hasWindow) window.open(url, "_blank", "noopener");
    },
  };
  return api;
}
