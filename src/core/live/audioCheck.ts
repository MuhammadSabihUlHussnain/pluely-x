// Pre-meeting audio check. "Them" is a loopback of the default output device, so it only
// hears the interviewer when their voice plays through this computer. When it doesn't, the
// mic picks the voice up instead, it is labelled "me", and the app never answers. This
// module detects that: on every start of listening it checks that computer audio reaches
// the "them" capture (a quiet test tone if nothing is playing), and afterwards it watches
// for the telltale pattern of your mic hearing speech while no interviewer speech arrives.

import type { Level } from "../types";

/** Peak level (dBFS) above which a capture counts as "hearing something". */
export const SIGNAL_PEAK_DB = -55;

export const hasSignal = (l: Level | null | undefined): boolean => !!l && l.peakDb > SIGNAL_PEAK_DB;

export type AudioProblem = "not_capturing" | "silent" | "unverified" | "mic_only";

export type AudioCheck =
  | { phase: "idle" }
  | { phase: "checking" }
  | { phase: "ok"; how: "call_audio" | "test_tone" }
  | { phase: "problem"; kind: AudioProblem };

export const AUDIO_CHECK_IDLE: AudioCheck = { phase: "idle" };

export interface CheckDeps {
  /** Latest level of the "them" capture (null when it isn't capturing). */
  themLevel(): Level | null;
  /** Whether the "them" capture is open at all. */
  themCapturing(): boolean;
  /** Plays a short quiet tone on the default output. Resolves false when it couldn't. */
  playTone(): Promise<boolean>;
  sleep(ms: number): Promise<void>;
  now(): number;
  /** True once the check was superseded (session stopped, re-run). */
  cancelled(): boolean;
}

export const CHECK_TIMINGS = {
  captureWaitMs: 1500,
  callAudioListenMs: 700,
  toneListenMs: 2500,
  noToneListenMs: 3000,
  pollMs: 40,
};

/** Polls until `ms` elapsed (true if a signal was seen at any point) or the check is cancelled. */
async function listen(deps: CheckDeps, ms: number): Promise<boolean> {
  const end = deps.now() + ms;
  for (;;) {
    if (hasSignal(deps.themLevel())) return true;
    if (deps.cancelled() || deps.now() >= end) return false;
    await deps.sleep(CHECK_TIMINGS.pollMs);
  }
}

/** Returns the verdict, or null if the check was cancelled before it finished. */
export async function runAudioCheck(deps: CheckDeps): Promise<AudioCheck | null> {
  // 1. The output capture must be open.
  const capEnd = deps.now() + CHECK_TIMINGS.captureWaitMs;
  while (!deps.themCapturing()) {
    if (deps.cancelled()) return null;
    if (deps.now() >= capEnd) return { phase: "problem", kind: "not_capturing" };
    await deps.sleep(CHECK_TIMINGS.pollMs);
  }
  // 2. A call may already be playing: that is the real thing, no beep needed.
  if (await listen(deps, CHECK_TIMINGS.callAudioListenMs)) return { phase: "ok", how: "call_audio" };
  if (deps.cancelled()) return null;
  // 3. Nothing playing: send a quiet beep through the default output and see if it comes back.
  const played = await deps.playTone();
  if (deps.cancelled()) return null;
  if (await listen(deps, played ? CHECK_TIMINGS.toneListenMs : CHECK_TIMINGS.noToneListenMs)) {
    return { phase: "ok", how: played ? "test_tone" : "call_audio" };
  }
  if (deps.cancelled()) return null;
  // No beep could be played (blocked by the webview) and nothing else played either: we can't
  // tell the difference between "broken" and "quiet room", so say so rather than cry wolf.
  return { phase: "problem", kind: played ? "silent" : "unverified" };
}

/**
 * Schedules a quiet 660 Hz beep (~0.7 s) and resolves as soon as it is scheduled, so the
 * caller can listen *while* it plays. Measured on Windows: a fresh output stream delivers
 * nothing to loopback for the first ~0.5 s (up to ~1 s cold), so the beep starts after
 * 0.4 s of silence and lasts 0.6 s; a shorter beep could finish before the capture ever
 * saw it. Never throws.
 */
export async function playCheckTone(): Promise<boolean> {
  try {
    const Ctx: typeof AudioContext | undefined =
      window.AudioContext ?? (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext;
    if (!Ctx) return false;
    const ctx = new Ctx();
    await ctx.resume();
    if (ctx.state !== "running") {
      void ctx.close().catch(() => {});
      return false;
    }
    const gain = ctx.createGain();
    gain.connect(ctx.destination);
    const osc = ctx.createOscillator();
    osc.type = "sine";
    osc.frequency.value = 660;
    osc.connect(gain);
    const t = ctx.currentTime + 0.4; // warm-up: let the output stream start
    gain.gain.setValueAtTime(0, ctx.currentTime);
    gain.gain.setValueAtTime(0, t);
    gain.gain.linearRampToValueAtTime(0.12, t + 0.03);
    gain.gain.setValueAtTime(0.12, t + 0.6);
    gain.gain.linearRampToValueAtTime(0, t + 0.68);
    osc.start();
    osc.stop(t + 0.7);
    setTimeout(() => void ctx.close().catch(() => {}), 1800);
    return true;
  } catch {
    return false;
  }
}

/**
 * Watches for "your mic is hearing talking but no interviewer speech has arrived". That is what
 * a silent output capture looks like from inside a session, and it can start mid-session
 * (the output device changed, the call moved to other speakers).
 */
export class ThemSilenceWatch {
  static readonly MIN_ME_WORDS = 30;
  static readonly MIN_ELAPSED_MS = 60_000;

  private themSpoke = false;
  private meWords = 0;
  private warned = false;

  constructor(private startedAtMs: number) {}

  onThemSpeech(): void {
    this.themSpoke = true;
  }

  /** Feed your own finished lines. Returns true exactly once, when the warning should show. */
  onMeFinal(text: string, nowMs: number): boolean {
    this.meWords += text.trim() ? text.trim().split(/\s+/).length : 0;
    if (this.warned || this.themSpoke) return false;
    if (this.meWords < ThemSilenceWatch.MIN_ME_WORDS) return false;
    if (nowMs - this.startedAtMs < ThemSilenceWatch.MIN_ELAPSED_MS) return false;
    this.warned = true;
    return true;
  }
}

export function describeProblem(kind: AudioProblem): { title: string; detail: string } {
  switch (kind) {
    case "not_capturing":
      return {
        title: "Not listening to computer audio",
        detail: "The audio-output capture didn't start, so the interviewer won't be heard. Stop and start listening again.",
      };
    case "silent":
      return {
        title: "Can't hear computer audio",
        detail:
          "A test beep didn't reach the capture. Check that the call plays through this computer's default speakers and isn't muted, then re-run the check.",
      };
    case "unverified":
      return {
        title: "Couldn't verify computer audio",
        detail:
          "No sound is playing and the test beep was blocked. Play any sound, or have them speak, and watch the Them meter move.",
      };
    case "mic_only":
      return {
        title: "Not hearing the interviewer",
        detail:
          "Your mic is picking up talking, but nothing is coming through computer audio, so answers won't appear. If they're on this computer, check the output device and volume.",
      };
  }
}
