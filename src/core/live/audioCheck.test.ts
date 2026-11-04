import { describe, expect, it } from "vitest";
import type { Level } from "../types";
import { callDeviceMismatch, describeProblem, hasSignal, runAudioCheck, ThemSilenceWatch, type CheckDeps } from "./audioCheck";

const LOUD: Level = { rmsDb: -30, peakDb: -20 };
const QUIET: Level = { rmsDb: -90, peakDb: -90 };

/** Virtual clock: sleeping advances time, and `levelAt` says what the capture hears when. */
function harness(opts: {
  capturingAtMs?: number | null; // null = never
  levelAt: (tMs: number, toneAtMs: number | null) => Level | null;
  toneWorks?: boolean;
}) {
  let t = 0;
  let toneAt: number | null = null;
  let cancelled = false;
  const deps: CheckDeps = {
    themLevel: () => opts.levelAt(t, toneAt),
    themCapturing: () => opts.capturingAtMs !== null && t >= (opts.capturingAtMs ?? 0),
    playTone: async () => {
      if (opts.toneWorks === false) return false;
      toneAt = t;
      t += 450;
      return true;
    },
    sleep: async (ms) => {
      t += ms;
    },
    now: () => t,
    cancelled: () => cancelled,
  };
  return { deps, cancel: () => (cancelled = true), time: () => t };
}

describe("hasSignal", () => {
  it("needs a peak above the floor", () => {
    expect(hasSignal(null)).toBe(false);
    expect(hasSignal(QUIET)).toBe(false);
    expect(hasSignal(LOUD)).toBe(true);
  });
});

describe("runAudioCheck", () => {
  it("passes right away when a call is already playing", async () => {
    const h = harness({ levelAt: () => LOUD });
    expect(await runAudioCheck(h.deps)).toEqual({ phase: "ok", how: "call_audio" });
    expect(h.time()).toBeLessThan(100); // no beep, no waiting
  });

  it("passes via the test tone when the room is quiet", async () => {
    const h = harness({
      levelAt: (t, toneAt) => (toneAt !== null && t >= toneAt + 100 ? LOUD : QUIET),
    });
    expect(await runAudioCheck(h.deps)).toEqual({ phase: "ok", how: "test_tone" });
  });

  it("reports silent when the beep never reaches the capture", async () => {
    const h = harness({ levelAt: () => QUIET });
    expect(await runAudioCheck(h.deps)).toEqual({ phase: "problem", kind: "silent" });
  });

  it("reports not_capturing when the output capture never opens", async () => {
    const h = harness({ capturingAtMs: null, levelAt: () => null });
    expect(await runAudioCheck(h.deps)).toEqual({ phase: "problem", kind: "not_capturing" });
  });

  it("waits for a capture that opens a moment late", async () => {
    const h = harness({ capturingAtMs: 600, levelAt: () => LOUD });
    expect(await runAudioCheck(h.deps)).toEqual({ phase: "ok", how: "call_audio" });
  });

  it("says unverified (not broken) when the beep is blocked and nothing plays", async () => {
    const h = harness({ levelAt: () => QUIET, toneWorks: false });
    expect(await runAudioCheck(h.deps)).toEqual({ phase: "problem", kind: "unverified" });
  });

  it("still passes if a call starts while the blocked beep is being waited out", async () => {
    const h = harness({ levelAt: (t) => (t > 1500 ? LOUD : QUIET), toneWorks: false });
    expect(await runAudioCheck(h.deps)).toEqual({ phase: "ok", how: "call_audio" });
  });

  it("returns null when cancelled mid-check", async () => {
    const h = harness({ levelAt: () => QUIET });
    h.cancel();
    expect(await runAudioCheck(h.deps)).toBeNull();
  });
});

describe("ThemSilenceWatch", () => {
  const words = (n: number) => Array.from({ length: n }, () => "word").join(" ");

  it("warns once your mic has heard plenty of talking and nobody else spoke", () => {
    const w = new ThemSilenceWatch(0);
    expect(w.onMeFinal(words(20), 70_000)).toBe(false); // not enough talking yet
    expect(w.onMeFinal(words(15), 80_000)).toBe(true);
    expect(w.onMeFinal(words(40), 90_000)).toBe(false); // only once
  });

  it("stays quiet if the interviewer has spoken", () => {
    const w = new ThemSilenceWatch(0);
    w.onThemSpeech();
    expect(w.onMeFinal(words(80), 120_000)).toBe(false);
  });

  it("doesn't warn in the first minute", () => {
    const w = new ThemSilenceWatch(0);
    expect(w.onMeFinal(words(60), 20_000)).toBe(false);
    expect(w.onMeFinal(words(1), 61_000)).toBe(true); // enough words, now past a minute
  });
});

describe("callDeviceMismatch", () => {
  const speakers = { id: "out-realtek", name: "Speakers (Realtek)", isDefault: true, isDefaultComms: false };
  const headset = { id: "out-jabra", name: "Headphones (Jabra)", isDefault: false, isDefaultComms: true };

  it("flags a communications device that isn't the one being captured", () => {
    expect(callDeviceMismatch([speakers, headset], "out-realtek")).toEqual({
      callId: "out-jabra",
      call: "Headphones (Jabra)",
      capture: "Speakers (Realtek)",
    });
    // No device known yet: the default output is what gets captured.
    expect(callDeviceMismatch([speakers, headset], null)?.capture).toBe("Speakers (Realtek)");
  });

  it("is quiet when the call device is the captured one", () => {
    expect(callDeviceMismatch([speakers, headset], "out-jabra")).toBeNull();
    const both = { ...speakers, isDefaultComms: true };
    expect(callDeviceMismatch([both, { ...headset, isDefaultComms: false }], null)).toBeNull();
    expect(callDeviceMismatch([speakers], null)).toBeNull(); // no comms default reported
  });

  it("names both devices in the warnings", () => {
    const d = callDeviceMismatch([speakers, headset], null)!;
    expect(describeProblem("other_device", d).detail).toContain('"Headphones (Jabra)"');
    expect(describeProblem("mic_only", d).detail).toContain('"Headphones (Jabra)"');
    expect(describeProblem("mic_only").detail).not.toContain("Jabra");
  });
});
