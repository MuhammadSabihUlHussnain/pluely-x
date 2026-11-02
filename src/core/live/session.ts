// Live session engine + store: consumes the SessionEvent channel, assembles
// the transcript, feeds turns to the question pipeline, persists finals and
// cards, keeps rolling notes, and writes the end-of-session summary.

import { create } from "zustand";
import { extractKeyterms, type ContextLine } from "../ai/context";
import { describeSessionError, mapLlmError } from "../ai/errors";
import { DEFAULT_MODE_PROMPTS } from "../ai/prompts";
import { buildNotes, buildSummary, type HistoryTurn, type PromptContext } from "../ai/requests";
import { runStructured } from "../ai/run";
import { EMPTY_NOTES, type RollingNotes, type SessionSummaryJson } from "../ai/schemas";
import { getRepo, transcriptRowId, type BriefRow } from "../db";
import { MODE_META } from "../defaults";
import { native } from "../native";
import { useSettings } from "../settings";
import type {
  DeviceInfo,
  Level,
  Mode,
  SessionEvent,
  SessionOptions,
  Source,
  SourceState,
  TranscriptEvent,
} from "../types";
import { newId, newRequestId } from "../util";
import {
  AUDIO_CHECK_IDLE,
  playCheckTone,
  runAudioCheck,
  ThemSilenceWatch,
  type AudioCheck,
} from "./audioCheck";
import { cardSink, useCards, type Card } from "./cards";
import { runQuickScan, SCAN_IDLE, type ScanState } from "./quickScan";
import { AnswerPipeline } from "./pipeline";
import {
  applyPatch,
  applyTranscript,
  emptyTranscript,
  finalLines,
  trimTranscript,
  type TranscriptState,
} from "./transcript";

export type ListenStatus = "idle" | "starting" | "live" | "stopping";

export interface SourceInfo {
  state: SourceState;
  device?: DeviceInfo;
  attempt?: number;
}

export interface LiveError {
  code: string;
  title: string;
  message: string;
  fatal: boolean;
  fix?: "mic_privacy" | "settings";
}

export interface LiveState {
  status: ListenStatus;
  /** Database session id of the current overlay conversation (listening or ask-only). */
  conversationId: string | null;
  conversationMode: Mode | null;
  startedAtMs: number | null;
  sources: Record<Source, SourceInfo>;
  levels: { me: Level | null; them: Level | null };
  /** Whether computer audio (the interviewer) actually reaches the "them" capture. */
  audioCheck: AudioCheck;
  /** "Quick scan & fix" progress and results. */
  scan: ScanState;
  transcript: TranscriptState;
  error: LiveError | null;
  notes: RollingNotes | null;
  notesUpdatedAt: number | null;
  notesBusy: boolean;
  summaryBusy: boolean;
  /** Brief chosen for this conversation; undefined = the mode's default brief. */
  briefOverride: string | null | undefined;
  briefs: BriefRow[];
}

const idleSources = (): Record<Source, SourceInfo> => ({ me: { state: "idle" }, them: { state: "idle" } });

export const useLive = create<LiveState>(() => ({
  status: "idle",
  conversationId: null,
  conversationMode: null,
  startedAtMs: null,
  sources: idleSources(),
  levels: { me: null, them: null },
  audioCheck: AUDIO_CHECK_IDLE,
  scan: SCAN_IDLE,
  transcript: emptyTranscript(),
  error: null,
  notes: null,
  notesUpdatedAt: null,
  notesBusy: false,
  summaryBusy: false,
  briefOverride: undefined,
  briefs: [],
}));

const MAX_SEGMENTS = 600;
const NOTES_INTERVAL_MS = 150_000; // ~2.5 minutes
const NOTES_MIN_NEW_CHARS = 280;

let notesTimer: ReturnType<typeof setInterval> | null = null;
let notesMark = { tMs: -1, at: 0 };

const set = useLive.setState;
const get = useLive.getState;
const mode = (): Mode => useSettings.getState().settings.mode;

// ---------------------------------------------------------------------------
// Prompt context: mode prompt (editable in Settings) + the active brief.

export async function loadBriefs(): Promise<BriefRow[]> {
  try {
    const briefs = await getRepo().listBriefs();
    set({ briefs });
    return briefs;
  } catch {
    return get().briefs;
  }
}

export async function activeBrief(m: Mode): Promise<BriefRow | null> {
  const override = get().briefOverride;
  const repo = getRepo();
  try {
    if (override === null) return null;
    if (override !== undefined) return (await repo.listBriefs()).find((b) => b.id === override) ?? null;
    return await repo.defaultBrief(m);
  } catch {
    return null;
  }
}

export async function promptContext(m: Mode): Promise<PromptContext> {
  let modePrompt = DEFAULT_MODE_PROMPTS[m];
  try {
    modePrompt = (await getRepo().getModePrompts())[m] ?? modePrompt;
  } catch {
    /* defaults */
  }
  const brief = await activeBrief(m);
  return {
    mode: m,
    modePrompt,
    brief: brief ? { name: brief.name, content: brief.content } : null,
    ai: useSettings.getState().settings.ai,
  };
}

export function transcriptLines(): ContextLine[] {
  return finalLines(get().transcript);
}

// ---------------------------------------------------------------------------
// Conversations & persistence

async function ensureConversation(): Promise<string> {
  const cur = get().conversationId;
  if (cur) return cur;
  const id = newId("ses");
  const m = mode();
  set({ conversationId: id, conversationMode: m });
  const brief = await activeBrief(m);
  try {
    await getRepo().createSession({ id, mode: m, startedAt: Date.now(), briefId: brief?.id ?? null });
  } catch {
    /* the overlay keeps working without the library */
  }
  return id;
}

async function persistCard(card: Card): Promise<void> {
  const sessionId = await ensureConversation();
  let screenshotPath: string | null = card.screenshot?.path ?? null;
  if (card.screenshot && !screenshotPath) {
    try {
      screenshotPath = (await native.screenshotSave(card.screenshot.id, sessionId)).path;
      cardSink.update(card.id, { screenshot: { ...card.screenshot, path: screenshotPath } });
    } catch {
      screenshotPath = null;
    }
  }
  await getRepo().addCard({
    id: card.id,
    sessionId,
    kind: card.chip ? `followup:${card.chip}` : card.kind,
    question: card.question,
    content: card.content,
    screenshotPath,
    status: card.stopNote === "max_tokens" ? "truncated" : "done",
    createdAt: card.createdAt,
  });
  cardSink.update(card.id, { persisted: true });
  // Ask-only conversations take their title from the first question.
  const s = await getRepo().getSession(sessionId);
  if (s && !s.title && card.question && get().status === "idle") {
    await getRepo().updateSession(sessionId, { title: card.question.slice(0, 80) });
  }
}

function persistFinal(sessionId: string, ev: TranscriptEvent) {
  void getRepo()
    .addTranscript({
      id: transcriptRowId(sessionId, ev.segId),
      sessionId,
      speaker: ev.source,
      text: ev.text,
      t0Ms: ev.t0Ms,
      t1Ms: ev.t1Ms,
    })
    .catch(() => {});
}

// ---------------------------------------------------------------------------
// Pipeline

export const pipeline = new AnswerPipeline({
  transport: { stream: native.llmStream, cancel: native.llmCancel },
  cards: cardSink,
  mode,
  promptContext,
  transcript: transcriptLines,
  onComplete: persistCard,
});

/** Starts a fresh conversation: clears cards, transcript and notes. */
export function newConversation(): void {
  if (get().status !== "idle") return;
  pipeline.stopAll();
  useCards.getState().clear();
  set({
    conversationId: null,
    conversationMode: null,
    transcript: emptyTranscript(),
    notes: null,
    notesUpdatedAt: null,
    error: null,
    briefOverride: undefined,
  });
}

// ---------------------------------------------------------------------------
// Session events

function handleEvent(ev: SessionEvent): void {
  const st = get();
  switch (ev.type) {
    case "state": {
      if (ev.source === null) {
        // Session-level: connecting → live → error (every source failed) → stopping → stopped.
        if (ev.state === "live" && st.status === "starting") set({ status: "live" });
        else if (ev.state === "error") void stopListening({ summarize: true });
        else if (ev.state === "stopped") {
          set({ sources: idleSources(), levels: { me: null, them: null } });
          // Stopped natively (not by us): close the session the same way.
          if (st.status === "live" || st.status === "starting") void finishSession({ summarize: true });
        }
        return;
      }
      const sources = { ...st.sources, [ev.source]: { ...st.sources[ev.source], state: ev.state, attempt: ev.attempt } };
      const patch: Partial<LiveState> = { sources };
      if (ev.state === "live" && st.error && !st.error.fatal) patch.error = null;
      if (st.status === "starting" && ev.state === "live") patch.status = "live";
      set(patch);
      return;
    }
    case "device":
      deviceOpens[ev.source]++;
      set({
        sources: {
          ...st.sources,
          [ev.source]: { ...st.sources[ev.source], device: { deviceId: ev.deviceId, name: ev.name, format: ev.format } },
        },
      });
      return;
    case "transcript": {
      watchTranscript(ev);
      set({ transcript: trimTranscript(applyTranscript(st.transcript, ev), MAX_SEGMENTS) });
      if (ev.isFinal && !ev.echo && st.conversationId) persistFinal(st.conversationId, ev);
      return;
    }
    case "transcriptPatch":
      set({ transcript: applyPatch(st.transcript, ev.segId, ev.echo) });
      if (ev.echo && st.conversationId) void getRepo().deleteTranscript(transcriptRowId(st.conversationId, ev.segId)).catch(() => {});
      return;
    case "levels":
      set({ levels: { me: ev.me, them: ev.them } });
      return;
    case "turn":
      pipeline.onTurn(ev);
      return;
    case "error": {
      // A fatal error ends one source only (a partial session keeps running);
      // the session-level `state: error` above is what ends everything.
      const d = describeSessionError(ev.code, ev.message);
      const sourceName = ev.source === "me" ? "Microphone" : ev.source === "them" ? "Call audio" : null;
      set({
        error: {
          code: ev.code,
          title: ev.fatal && sourceName && !d.fix ? `${sourceName}: ${d.title}` : d.title,
          message: ev.message,
          fatal: ev.fatal,
          fix: d.fix,
        },
      });
      if (ev.fatal && ev.source) {
        set({ sources: { ...get().sources, [ev.source]: { ...get().sources[ev.source], state: "error" } } });
      }
      return;
    }
    case "snapshot": {
      if (!ev.status.active) return;
      set({
        status: "live",
        conversationId: ev.status.sessionId ?? st.conversationId,
        startedAtMs: ev.status.startedAtMs ?? st.startedAtMs,
      });
      for (const e of ev.recentFinals) handleEvent(e);
      return;
    }
    default:
      return;
  }
}

// ---------------------------------------------------------------------------
// Audio check: is the interviewer's side (computer audio) actually being heard?

let checkRun = 0;
let silenceWatch: ThemSilenceWatch | null = null;

function logAudio(level: "info" | "warn", message: string): void {
  void native.logWrite(level, `health: ${message}`).catch(() => {});
}

/** Runs the check (re-runnable). Supersedes any check still in flight. */
export async function runAudioCheckNow(): Promise<void> {
  const run = ++checkRun;
  set({ audioCheck: { phase: "checking" } });
  const sleep = (ms: number) => new Promise<void>((r) => setTimeout(r, ms));
  const verdict = await runAudioCheck({
    themLevel: () => get().levels.them,
    themCapturing: () => {
      const them = get().sources.them;
      return them.state !== "error" && (get().levels.them !== null || !!them.device);
    },
    playTone: playCheckTone,
    sleep,
    now: () => Date.now(),
    cancelled: () => run !== checkRun || get().status === "idle" || get().status === "stopping",
  });
  if (run !== checkRun || !verdict) return;
  set({ audioCheck: verdict });
  if (verdict.phase === "problem") logAudio("warn", `problem=${verdict.kind}`);
  else if (verdict.phase === "ok") logAudio("info", `ok via ${verdict.how}`);
}

export function dismissAudioCheck(): void {
  set({ audioCheck: AUDIO_CHECK_IDLE });
}

function cancelAudioCheck(): void {
  checkRun++;
  silenceWatch = null;
  set({ audioCheck: AUDIO_CHECK_IDLE });
}

/** Feeds transcript events to the silence watch (interviewer spoke / you kept talking alone). */
function watchTranscript(ev: TranscriptEvent): void {
  if (!silenceWatch) return;
  if (ev.source === "them") {
    if (ev.text.trim()) silenceWatch.onThemSpeech();
    return;
  }
  if (!ev.isFinal || ev.echo) return;
  if (silenceWatch.onMeFinal(ev.text, Date.now()) && get().audioCheck.phase !== "problem") {
    set({ audioCheck: { phase: "problem", kind: "mic_only" } });
    logAudio("warn", "problem=mic_only (mic heard talking, no interviewer speech)");
  }
}

// ---------------------------------------------------------------------------
// Quick scan & fix

/** Capture (re)opens per source; a restart waits for this to tick. */
const deviceOpens: Record<Source, number> = { me: 0, them: 0 };
const STUCK_ANSWER_MS = 60_000;

async function restartSource(source: Source): Promise<boolean> {
  const before = deviceOpens[source];
  await native.sessionSetSource({ source, enabled: false });
  await new Promise((r) => setTimeout(r, 300));
  await native.sessionSetSource({ source, enabled: true });
  const end = Date.now() + 6000;
  while (Date.now() < end) {
    if (deviceOpens[source] > before) return true;
    if (get().status !== "live") return false;
    await new Promise((r) => setTimeout(r, 100));
  }
  return false;
}

function clearStuckAnswers(): number {
  const now = Date.now();
  let n = 0;
  for (const c of useCards.getState().cards) {
    const waiting = c.status === "pending" || c.status === "thinking" || c.status === "streaming";
    if (!waiting || c.content.trim() || now - c.createdAt < STUCK_ANSWER_MS) continue;
    if (pipeline.isBusy(c.id)) pipeline.stop(c.id);
    else cardSink.remove(c.id);
    n++;
  }
  return n;
}

export async function runQuickScanNow(): Promise<void> {
  if (get().scan.running) return;
  set({ scan: { running: true, steps: [] } });
  logAudio("info", "quick scan started");
  const steps = await runQuickScan({
    keysStatus: async () => {
      const k = await native.keysStatus();
      return { anthropic: k.anthropic.present, deepgram: k.deepgram.present };
    },
    testKey: async (p) => {
      const r = await native.keysTest(p);
      return { ok: r.ok, message: r.message };
    },
    devices: async () => {
      const d = await native.audioListDevices();
      return { inputs: d.inputs.length, outputs: d.outputs.length };
    },
    listening: () => get().status === "live",
    modeAutoAnswers: () => {
      const meta = MODE_META[mode()];
      return { label: meta.label, autoAnswers: meta.autoAnswers };
    },
    restartSource,
    clearStuckAnswers,
    runAudioCheck: async () => {
      await runAudioCheckNow();
      return get().audioCheck;
    },
    shortcutFailures: async () => (await native.shortcutsStatus()).failures.length,
    onUpdate: (steps) => set({ scan: { running: true, steps } }),
  });
  set({ scan: { running: false, steps, finishedAt: Date.now() } });
  const bad = steps.filter((s) => s.status === "fail" || s.status === "warn").map((s) => `${s.id}=${s.status}`);
  logAudio(bad.length ? "warn" : "info", `quick scan done ${bad.length ? bad.join(" ") : "all ok"}`);
}

export function closeQuickScan(): void {
  if (!get().scan.running) set({ scan: SCAN_IDLE });
}

// ---------------------------------------------------------------------------
// Listening

export async function buildSessionOptions(): Promise<SessionOptions> {
  const s = useSettings.getState().settings;
  const brief = await activeBrief(s.mode);
  return {
    me: { enabled: true, deviceId: s.audio.micDeviceId },
    them: { enabled: true, deviceId: s.audio.outputDeviceId },
    stt: {
      language: s.stt.language,
      keyterms: brief ? extractKeyterms(brief.content) : [],
      endpointingMs: s.stt.endpointingMs,
      utteranceEndMs: s.stt.utteranceEndMs,
      diarizeThem: s.stt.diarizeThem,
    },
    aec: s.audio.aec,
    echoDedup: true,
  };
}

export async function startListening(): Promise<void> {
  if (get().status !== "idle") return;
  const m = mode();
  if (!MODE_META[m].listens) await useSettings.getState().setMode("meeting").catch(() => {});
  // Listening always starts a fresh conversation.
  pipeline.stopAll();
  useCards.getState().clear();
  set({
    status: "starting",
    error: null,
    audioCheck: AUDIO_CHECK_IDLE,
    transcript: emptyTranscript(),
    notes: null,
    notesUpdatedAt: null,
    conversationId: null,
    sources: { me: { state: "connecting" }, them: { state: "connecting" } },
  });
  try {
    const opts = await buildSessionOptions();
    const info = await native.sessionStart(opts, handleEvent);
    const sm = mode();
    set({
      status: get().status === "starting" ? "live" : get().status,
      conversationId: info.sessionId,
      conversationMode: sm,
      startedAtMs: info.startedAtMs,
    });
    const brief = await activeBrief(sm);
    await getRepo()
      .createSession({ id: info.sessionId, mode: sm, startedAt: info.startedAtMs, briefId: brief?.id ?? null })
      .catch(() => {});
    notesMark = { tMs: -1, at: Date.now() };
    startNotesTimer();
    // Every time listening starts: confirm the interviewer's audio reaches us before the meeting.
    silenceWatch = new ThemSilenceWatch(Date.now());
    void runAudioCheckNow();
  } catch (e) {
    const err = e as { code?: string; message?: string };
    const d = describeSessionError(err.code ?? "unknown", err.message ?? "");
    set({
      status: "idle",
      sources: idleSources(),
      error: { code: err.code ?? "unknown", title: d.title, message: err.message ?? "", fatal: true, fix: d.fix },
    });
  }
}

export async function stopListening(opts: { summarize?: boolean } = { summarize: true }): Promise<void> {
  const st = get();
  if (st.status !== "live" && st.status !== "starting") return;
  set({ status: "stopping" });
  cancelAudioCheck();
  stopNotesTimer();
  try {
    await native.sessionStop(); // can take ~4 s while Deepgram flushes the last words
  } catch {
    /* already stopped (no_session) */
  }
  await finishSession(opts);
}

/** Marks the session ended, persists ended_at and writes the summary. */
async function finishSession(opts: { summarize?: boolean }): Promise<void> {
  stopNotesTimer();
  cancelAudioCheck();
  const sessionId = get().conversationId;
  set({ status: "idle", sources: idleSources(), levels: { me: null, them: null } });
  if (sessionId) {
    await getRepo().updateSession(sessionId, { endedAt: Date.now() }).catch(() => {});
    if (opts.summarize !== false) void summarize(sessionId);
  }
}

export async function toggleListening(): Promise<void> {
  const s = get().status;
  if (s === "idle") await startListening();
  else if (s === "live" || s === "starting") await stopListening();
}

/** Re-attaches to a running native session after a webview reload. */
export async function reattach(): Promise<void> {
  try {
    const status = await native.sessionStatus();
    if (!status.active) return;
    set({
      status: "live",
      conversationId: status.sessionId ?? null,
      conversationMode: mode(),
      startedAtMs: status.startedAtMs ?? null,
    });
    await native.sessionReattach(handleEvent);
    startNotesTimer();
  } catch {
    /* nothing to attach to */
  }
}

// ---------------------------------------------------------------------------
// Rolling notes (Meeting, Lecture)

function startNotesTimer() {
  stopNotesTimer();
  notesTimer = setInterval(() => {
    const m = mode();
    if (!MODE_META[m].rollingNotes || get().notesBusy || get().status !== "live") return;
    if (Date.now() - notesMark.at < NOTES_INTERVAL_MS) return;
    const fresh = transcriptLines().filter((l) => l.t0Ms > notesMark.tMs);
    if (fresh.reduce((n, l) => n + l.text.length, 0) < NOTES_MIN_NEW_CHARS) return;
    void refreshNotes();
  }, 20_000);
}

function stopNotesTimer() {
  if (notesTimer) clearInterval(notesTimer);
  notesTimer = null;
}

/** Updates the rolling notes from the transcript since the last update. */
export async function refreshNotes(): Promise<void> {
  const m = mode();
  if (m !== "meeting" && m !== "lecture") return;
  if (get().notesBusy) return;
  const lines = transcriptLines();
  const fresh = lines.filter((l) => l.t0Ms > notesMark.tMs);
  if (!fresh.length) return;
  set({ notesBusy: true });
  const lastT = Math.max(...fresh.map((l) => l.t0Ms));
  try {
    const ctx = await promptContext(m);
    const current = get().notes ?? EMPTY_NOTES[m];
    const res = await runStructured<RollingNotes>(
      { stream: native.llmStream, cancel: native.llmCancel },
      newRequestId(),
      buildNotes(ctx, m, current, fresh),
    );
    if (res.ok) {
      notesMark = { tMs: lastT, at: Date.now() };
      set({ notes: res.value, notesUpdatedAt: Date.now() });
      const sessionId = get().conversationId;
      if (sessionId)
        await getRepo()
          .upsertNotes({ sessionId, kind: m, contentJson: JSON.stringify(res.value), updatedAt: Date.now() })
          .catch(() => {});
    }
  } finally {
    set({ notesBusy: false });
  }
}

// ---------------------------------------------------------------------------
// End-of-session summary

async function summarize(sessionId: string): Promise<void> {
  const m = get().conversationMode ?? mode();
  const lines = transcriptLines();
  const answers: HistoryTurn[] = useCards
    .getState()
    .cards.filter((c) => c.kind !== "summary" && c.status === "done")
    .map((c) => ({ question: c.question, answer: c.content, hadScreenshot: !!c.screenshot }));
  if (!lines.length && !answers.length) return;

  const cardId = newId("card");
  cardSink.add({
    id: cardId,
    kind: "summary",
    mode: m,
    question: "Session summary",
    status: "thinking",
    content: "",
    createdAt: Date.now(),
  });
  set({ summaryBusy: true });
  try {
    const ctx = await promptContext(m);
    const res = await runStructured<Omit<SessionSummaryJson, "mode">>(
      { stream: native.llmStream, cancel: native.llmCancel },
      newRequestId(),
      buildSummary(ctx, lines, answers),
    );
    if (!res.ok) {
      cardSink.update(cardId, {
        status: "error",
        error: mapLlmError({ code: res.code, message: res.message }),
      });
      return;
    }
    const summary = { ...res.value, mode: m } as SessionSummaryJson;
    cardSink.update(cardId, { status: "done", summary });
    await getRepo()
      .updateSession(sessionId, { summaryJson: JSON.stringify(summary), title: summary.title || undefined })
      .catch(() => {});
  } finally {
    set({ summaryBusy: false });
  }
}

/** Re-runs a failed summary card. */
export async function retrySummary(cardId: string): Promise<void> {
  const sessionId = get().conversationId;
  cardSink.remove(cardId);
  if (sessionId) await summarize(sessionId);
}

/** Mode changes while live are recorded on the session. */
export async function onModeChanged(m: Mode): Promise<void> {
  set({ briefOverride: undefined });
  const sessionId = get().conversationId;
  if (sessionId && get().status === "live") {
    set({ conversationMode: m });
    await getRepo().updateSession(sessionId, { mode: m }).catch(() => {});
  }
}
