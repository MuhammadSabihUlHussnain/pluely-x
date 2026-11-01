import { describe, expect, it } from "vitest";
import type { Source, TranscriptEvent } from "../types";
import {
  applyPatch,
  applyTranscript,
  emptyTranscript,
  finalLines,
  segments,
  speakerBlocks,
  trimTranscript,
  type TranscriptState,
} from "./transcript";

function ev(source: Source, segId: string, text: string, t0Ms: number, isFinal: boolean, extra: Partial<TranscriptEvent> = {}): TranscriptEvent {
  return {
    type: "transcript",
    source,
    segId,
    conn: 1,
    isFinal,
    speechFinal: isFinal,
    fromFinalize: false,
    text,
    confidence: 0.9,
    t0Ms,
    t1Ms: t0Ms + 500,
    ...extra,
  };
}

function run(events: TranscriptEvent[], start: TranscriptState = emptyTranscript()) {
  return events.reduce(applyTranscript, start);
}

describe("transcript assembly", () => {
  it("replaces interim results in place by segId", () => {
    const s = run([
      ev("them", "a", "can you", 100, false),
      ev("them", "a", "can you walk me", 100, false),
      ev("them", "a", "Can you walk me through it?", 100, true),
    ]);
    const segs = segments(s);
    expect(segs).toHaveLength(1);
    expect(segs[0].text).toBe("Can you walk me through it?");
    expect(segs[0].isFinal).toBe(true);
  });

  it("never lets a late interim overwrite a final", () => {
    const s = run([ev("me", "x", "Final text.", 0, true), ev("me", "x", "stale interim", 0, false)]);
    expect(segments(s)[0].text).toBe("Final text.");
    expect(segments(s)[0].isFinal).toBe(true);
  });

  it("orders segments by session clock across both speakers, regardless of arrival", () => {
    const s = run([
      ev("them", "t2", "third", 3000, true),
      ev("me", "m1", "second", 1500, true),
      ev("them", "t1", "first", 1000, true),
    ]);
    expect(segments(s).map((x) => x.text)).toEqual(["first", "second", "third"]);
  });

  it("breaks start-time ties by arrival order", () => {
    const s = run([ev("them", "a", "one", 500, true), ev("me", "b", "two", 500, true)]);
    expect(segments(s).map((x) => x.segId)).toEqual(["a", "b"]);
  });

  it("hides echo segments flagged on arrival", () => {
    const s = run([ev("them", "t", "walk me through a project", 0, true), ev("me", "m", "through a project", 200, true, { echo: true })]);
    expect(segments(s).map((x) => x.source)).toEqual(["them"]);
    expect(segments(s, { showEcho: true })).toHaveLength(2);
  });

  it("hides echo segments patched later, including patches that arrive first", () => {
    let s = run([ev("them", "t", "hello there", 0, true), ev("me", "m", "hello there", 100, true)]);
    expect(segments(s)).toHaveLength(2);
    s = applyPatch(s, "m", true);
    expect(segments(s)).toHaveLength(1);
    // patch before the segment exists
    let s2 = applyPatch(emptyTranscript(), "early", true);
    s2 = applyTranscript(s2, ev("me", "early", "echoed words", 50, true));
    expect(segments(s2)).toHaveLength(0);
    // an echo patch can be reversed
    s = applyPatch(s, "m", false);
    expect(segments(s)).toHaveLength(2);
  });

  it("finalLines returns only final, non-echo text in order", () => {
    const s = run([
      ev("them", "t1", "Question?", 0, true),
      ev("me", "m1", "Answer", 1000, true),
      ev("me", "m2", "partial", 2000, false),
      ev("me", "m3", "Question?", 100, true, { echo: true }),
    ]);
    expect(finalLines(s)).toEqual([
      { source: "them", text: "Question?", t0Ms: 0, t1Ms: 500 },
      { source: "me", text: "Answer", t0Ms: 1000, t1Ms: 1500 },
    ]);
  });

  it("groups consecutive segments from one speaker into a block", () => {
    const s = run([
      ev("them", "a", "Hi.", 0, true),
      ev("them", "b", "How are you?", 800, true),
      ev("me", "c", "Good", 2000, false),
    ]);
    const blocks = speakerBlocks(segments(s));
    expect(blocks.map((b) => b.source)).toEqual(["them", "me"]);
    expect(blocks[0].parts.map((p) => p.text)).toEqual(["Hi.", "How are you?"]);
    expect(blocks[1].parts[0].isFinal).toBe(false);
  });

  it("trims the oldest segments beyond a cap", () => {
    const s = trimTranscript(
      run(Array.from({ length: 10 }, (_, i) => ev("them", `s${i}`, `line ${i}`, i * 100, true))),
      3,
    );
    expect(segments(s).map((x) => x.text)).toEqual(["line 7", "line 8", "line 9"]);
  });
});
