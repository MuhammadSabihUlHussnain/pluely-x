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

describe("follow on real interview answers", () => {
  // A custom IB prompt answers in paragraphs under "Say this:", not bullets.
  const ANSWER = [
    "**Say this:**",
    "I'll assume the deal is all stock, since funding wasn't given, and I'll ignore synergies and fees. The approach is to compare what our stock costs us to what the target earns relative to the price we pay.",
    "First, our net income. EBT is $100 million, and with a 50% tax rate that's $50 million.",
    "Next, our P/E. Market cap of $200 million divided by $50 million is 4x. The cost of stock is 1 over our P/E, so 1 over 4, or 25%.",
    "Now the target's earnings yield. 1 over its P/E of 4.5 is about 22%. That's the return we get on the price we pay.",
    "So we're paying 25% for stock to buy something yielding about 22%. Our cost is above the yield, so the deal is dilutive.",
    "**If they follow up:**",
  ].join("\n\n");

  it("splits any layout into reading blocks", async () => {
    const { answerBlocks } = await import("./follow");
    expect(answerBlocks(ANSWER)).toHaveLength(7);
    expect(answerBlocks("Line one.\n- point a\n- point b\n\nPara two\ncontinues.")).toEqual(["Line one.", "- point a", "- point b", "Para two continues."]);
  });

  it("follows the user's actual words (numbers transcribed differently, paraphrased)", async () => {
    const { answerBlocks } = await import("./follow");
    const blocks = answerBlocks(ANSWER);
    // What the mic heard in the real session (Deepgram writes 200,000,000 for "200 million").
    const heard = [
      "I'll assume the deal is all stocks since funding wasn't given, and I'll ignore synergies and fees. So the approach I'm gonna take is compare our stock costs what the target earns relative to the price.",
      "And so we'll start off by, with our net income. So EBIT is a 100,000,000, with 50 tax rate, so our net income is 50,000,000.",
      "Market cap of 200,000,000 divided by our shares outstanding of 50 into our PE ratio four x. So the cost of our stock is one over PE or one over four, so 25%.",
      "So now we have to find the target's yield earning. So one over its PE of 4.5 is about 22%.",
    ];
    let said = "";
    const lines = heard.map((h) => {
      said += " " + h;
      return speechProgress(blocks, said).line;
    });
    // Moves forward block by block as each part is said. (The user stopped before the
    // last words of paragraph 1 and of paragraph 4, so those stay current.)
    expect(lines).toEqual([1, 3, 4, 4]);
    for (let i = 1; i < lines.length; i++) expect(lines[i]).toBeGreaterThanOrEqual(lines[i - 1]);
  });

  it("normalises numbers the way people say and transcribe them", () => {
    expect(normalizeWords("$1 billion, 1,000,000,000 and 1 billion")).toEqual(["1000000000", "1000000000", "and", "1000000000"]);
    expect(normalizeWords("5.3x EBITDA, six times, 25%")).toEqual(["5.3", "times", "ebitda", "6", "times", "25", "percent"]);
  });

  it("doesn't count side remarks as reading", () => {
    const blocks = ["Net debt is 600. Enterprise value is 1,400, equity value is 800.", "Equity value is 8 times 100.", "Enterprise value is 4 times 350.", "So net debt is 600."];
    const p = speechProgress(
      blocks,
      "Net debt is 600. Enterprise value is 1,400, and equity value is 800. I think it gives the answer straight up, it needs to build up to the answer, I think that's what the prompt is doing wrong.",
    );
    expect(p.line).toBeLessThanOrEqual(1);
  });
});
