import { describe, expect, it } from "vitest";
import { followPosition, normalizeWords, scriptTokens, speechProgress } from "./follow";

const LINES = [
  "I'd use a token bucket per API key, with counters in Redis.",
  "Each key gets a bucket that refills at a steady rate.",
  "When a client runs out, they get a 429 with a Retry-After header.",
  "I'd fail open if Redis goes down.",
];

describe("normalizeWords", () => {
  it("lowercases, drops punctuation and apostrophes", () => {
    expect(normalizeWords("I'd use a **token** bucket, per API-key!")).toEqual(["id", "use", "a", "token", "bucket", "per", "api", "key"]);
  });
});

describe("speech follow", () => {
  it("does nothing before the user says anything that matches", () => {
    expect(speechProgress(LINES, "")).toMatchObject({ active: false, line: 0 });
    expect(speechProgress(LINES, "um so yeah okay")).toMatchObject({ active: false, line: 0 });
  });

  it("tracks the line being read, skipping fillers and ad-libs", () => {
    const p = speechProgress(LINES, "um so I'd use like a token bucket per API key with counters in Redis and each key gets a bucket");
    expect(p.active).toBe(true);
    expect(p.line).toBe(1);
  });

  it("tolerates small transcription differences (suffixes, numbers as digits)", () => {
    const spoken =
      "I'd use a token bucket per API key with counters in Redis. Each key get a bucket that refill at a steady rate. When a client run out they get a 429 with a retry after header";
    expect(speechProgress(LINES, spoken).line).toBe(3);
  });

  it("reaches the end when everything is said", () => {
    const all = LINES.join(" ");
    const p = speechProgress(LINES, all);
    expect(p.line).toBe(LINES.length);
    expect(p.spoken).toBe(p.total);
  });

  it("never moves backwards and common words can't cause long jumps", () => {
    const tokens = scriptTokens(LINES);
    const a = followPosition(tokens, normalizeWords("I'd use a token bucket"));
    // "a" appears many times later; on its own it must not jump ahead.
    const b = followPosition(tokens, normalizeWords("I'd use a token bucket a a a the the"));
    expect(b).toBeGreaterThanOrEqual(a);
    expect(b - a).toBeLessThanOrEqual(3);
  });

  it("can skip a few words the user dropped", () => {
    // User skips "per API key, with counters in" and jumps to "Redis".
    const p = speechProgress(LINES, "I'd use a token bucket Redis each key gets");
    expect(p.line).toBe(1);
  });
});
