import { describe, expect, it } from "vitest";
import { ASK_NOW_INSTRUCTION, DEFAULT_MODE_PROMPTS, NULL_SENTINEL } from "../ai/prompts";
import type { LlmTransport } from "../ai/run";
import { DEFAULT_SETTINGS } from "../defaults";
import type { LlmEvent, Mode, TurnEvent } from "../types";
import { memoryCardSink, type Card } from "./cards";
import { AnswerPipeline } from "./pipeline";

const tick = () => new Promise<void>((r) => setTimeout(r, 0));
const usage = { inputTokens: 10, outputTokens: 5, cacheReadInputTokens: 0, cacheCreationInputTokens: 0 };

function answer(text: string, stopReason: string | null = "end_turn"): LlmEvent[] {
  const chunks = text.match(/.{1,6}/gs) ?? [];
  return [
    { type: "start", requestId: "x", model: "claude-opus-5-5" },
    { type: "thinking" },
    ...chunks.map((delta): LlmEvent => ({ type: "text", delta })),
    { type: "stop", stopReason, usage },
  ];
}

type Step = { events: LlmEvent[]; hang?: boolean };

function fakeTransport(script: (body: { messages: unknown[] }, n: number) => Step) {
  const open = new Map<string, { onEvent: (e: LlmEvent) => void; resolve: () => void }>();
  const t = {
    calls: [] as { requestId: string; body: { messages: unknown[] }; betas?: string[] }[],
    cancelled: [] as string[],
    stream(requestId: string, body: object, betas: string[] | undefined, onEvent: (e: LlmEvent) => void) {
      const n = t.calls.length;
      t.calls.push({ requestId, body: body as { messages: unknown[] }, betas });
      const step = script(body as { messages: unknown[] }, n);
      return new Promise<void>((resolve) => {
        open.set(requestId, { onEvent, resolve });
        void (async () => {
          for (const e of step.events) {
            await tick();
            if (!open.has(requestId)) return;
            onEvent(e);
          }
          if (!step.hang) {
            open.delete(requestId);
            resolve();
          }
        })();
      });
    },
    async cancel(requestId: string) {
      t.cancelled.push(requestId);
      const o = open.get(requestId);
      if (!o) return;
      open.delete(requestId);
      o.onEvent({ type: "error", code: "cancelled", message: "Cancelled", retryable: false });
      o.resolve();
    },
  };
  return t satisfies LlmTransport & Record<string, unknown>;
}

function turn(turnId: string, text: string, source: "them" | "me" = "them", question = false): TurnEvent {
  return { type: "turn", source, turnId, text, t0Ms: 0, t1Ms: 1000, reason: "speechFinal", question };
}

function setup(script: (body: { messages: unknown[] }, n: number) => Step, mode: Mode = "interview") {
  const transport = fakeTransport(script);
  const cards = memoryCardSink();
  const completed: Card[] = [];
  let reqN = 0;
  let cardN = 0;
  const state = { mode };
  const pipeline = new AnswerPipeline({
    transport,
    cards,
    mode: () => state.mode,
    promptContext: async (m) => ({ mode: m, modePrompt: DEFAULT_MODE_PROMPTS[m], brief: null, ai: DEFAULT_SETTINGS.ai }),
    transcript: () => [
      { source: "them", text: "Thanks for joining.", t0Ms: 0, t1Ms: 900 },
      { source: "them", text: "Can you walk me through a project?", t0Ms: 1000, t1Ms: 3000 },
    ],
    onComplete: (c) => void completed.push(c),
    ids: { request: () => `req${++reqN}`, card: () => `card${++cardN}` },
  });
  return { pipeline, transport, cards, completed, state };
}

function lastUserText(body: { messages: unknown[] }): string {
  return JSON.stringify(body.messages[body.messages.length - 1]);
}

describe("question pipeline", () => {
  it("drops cards whose stream starts with ∅ and cancels the request", async () => {
    const { pipeline, transport, cards, completed } = setup(() => ({ events: answer(NULL_SENTINEL) }));
    expect(pipeline.onTurn(turn("t1", "How we do things here is a bit different."))).toBe(true);
    await pipeline.idle();
    expect(cards.cards).toHaveLength(0);
    expect(transport.cancelled).toEqual(["req1"]);
    expect(completed).toHaveLength(0);
  });

  it("drops ∅ even when it arrives after whitespace", async () => {
    const { pipeline, cards } = setup(() => ({ events: answer(`  \n${NULL_SENTINEL}`) }));
    pipeline.onTurn(turn("t1", "What a day."));
    await pipeline.idle();
    expect(cards.cards).toHaveLength(0);
  });

  it("streams a real answer into a card and persists it once", async () => {
    const { pipeline, cards, completed, transport } = setup(() => ({ events: answer("I'd pick the analytics pipeline.\n- fast") }));
    pipeline.onTurn(turn("t1", "Can you walk me through a project?"));
    await pipeline.idle();
    expect(cards.cards).toHaveLength(1);
    expect(cards.cards[0]).toMatchObject({ kind: "auto", status: "done", turnId: "t1", provisional: false });
    expect(cards.cards[0].content).toBe("I'd pick the analytics pipeline.\n- fast");
    expect(completed.map((c) => c.id)).toEqual(["card1"]);
    expect(lastUserText(transport.calls[0].body)).toContain("<latest_them_turn>");
  });

  it("never answers the same turnId twice", async () => {
    const { pipeline, transport } = setup(() => ({ events: answer("Yes.") }));
    expect(pipeline.onTurn(turn("t1", "Can you code?"))).toBe(true);
    expect(pipeline.onTurn(turn("t1", "Can you code?"))).toBe(false);
    await pipeline.idle();
    expect(pipeline.onTurn(turn("t1", "Can you code?"))).toBe(false);
    expect(transport.calls).toHaveLength(1);
  });

  it("only sends candidate Them turns", async () => {
    const { pipeline, transport } = setup(() => ({ events: answer("x") }));
    expect(pipeline.onTurn(turn("m1", "Can you hear me?", "me"))).toBe(false);
    expect(pipeline.onTurn(turn("t2", "Thanks for joining today."))).toBe(false);
    expect(pipeline.onTurn(turn("t3", "mm", "them", true))).toBe(true); // native hint
    await pipeline.idle();
    expect(transport.calls).toHaveLength(1);
  });

  it("does not auto-answer in Lecture or Ask mode", async () => {
    for (const mode of ["lecture", "ask"] as Mode[]) {
      const { pipeline, transport } = setup(() => ({ events: answer("x") }), mode);
      expect(pipeline.onTurn(turn("t1", "What is Dijkstra's algorithm?"))).toBe(false);
      expect(transport.calls).toHaveLength(0);
    }
  });

  it("cancels an in-flight auto-answer when a newer candidate arrives", async () => {
    const { pipeline, transport, cards } = setup((_b, n) =>
      n === 0 ? { events: [{ type: "start", requestId: "x", model: "m" }, { type: "thinking" }], hang: true } : { events: answer("Second answer.") },
    );
    pipeline.onTurn(turn("t1", "What's your background?"));
    await tick();
    await tick();
    pipeline.onTurn(turn("t2", "Actually, why did you leave your last job?"));
    await pipeline.idle();
    expect(transport.cancelled).toContain("req1");
    expect(cards.cards.map((c) => c.turnId)).toEqual(["t2"]);
    expect(cards.cards[0].status).toBe("done");
  });

  it("offers heard questions with an Answer button in modes without auto-answers", async () => {
    const { pipeline, transport, cards, completed } = setup(() => ({ events: answer("A merger combines two companies into one.") }), "ask");
    expect(pipeline.onTurn(turn("t1", "Thanks for joining."))).toBe(false);
    expect(pipeline.onTurn(turn("t2", "What's the difference between a merger and an acquisition?"))).toBe(false);
    expect(transport.calls).toHaveLength(0); // nothing is spent until the user asks
    expect(cards.cards).toHaveLength(1);
    expect(cards.cards[0]).toMatchObject({ kind: "heard", question: "Thanks for joining. What's the difference between a merger and an acquisition?" });
    // A newer heard question replaces the unanswered one.
    pipeline.onTurn(turn("t3", "Can you walk me through a basic merger model?"));
    expect(cards.cards.map((c) => c.question)).toEqual(["Can you walk me through a basic merger model?"]);
    pipeline.answerHeard(cards.cards[0].id);
    await pipeline.idle();
    expect(transport.calls).toHaveLength(1);
    expect(lastUserText(transport.calls[0].body)).toContain("Can you walk me through a basic merger model?");
    expect(cards.cards[0]).toMatchObject({ kind: "auto", status: "done", content: "A merger combines two companies into one." });
    expect(completed).toHaveLength(1);
  });

  it("never cancels an answer that is already showing text", async () => {
    const long = "I'd start with my background in data engineering. ".repeat(6);
    const { pipeline, cards, transport } = setup((_b, n) => (n === 0 ? { events: answer(long) } : { events: answer("Second.") }));
    pipeline.onTurn(turn("t1", "What's your background?"));
    for (let i = 0; i < 6; i++) await tick();
    expect(cards.cards[0].content.length).toBeGreaterThan(0);
    pipeline.onTurn(turn("t2", "We... Let's say third question? Okay."));
    await pipeline.idle();
    expect(transport.cancelled).not.toContain("req1");
    expect(cards.cards.map((c) => [c.turnId, c.status])).toEqual([
      ["t1", "done"],
      ["t2", "done"],
    ]);
  });

  it("merges a question spoken in pieces into one question", async () => {
    const { pipeline, cards, transport } = setup(() => ({ events: answer("Net debt is 50.") }));
    expect(pipeline.onTurn(turn("t1", "If net income"))).toBe(false);
    expect(pipeline.onTurn(turn("t2", "a hundred,"))).toBe(false);
    expect(pipeline.onTurn(turn("t3", "PE ratio is eight x."))).toBe(false);
    expect(pipeline.onTurn(turn("t4", "EBITDA is four x. Calculate the net debt amount."))).toBe(true);
    await pipeline.idle();
    const merged = "If net income a hundred, PE ratio is eight x. EBITDA is four x. Calculate the net debt amount.";
    expect(cards.cards[0].question).toBe(merged);
    expect(lastUserText(transport.calls[0].body)).toContain(merged);
  });

  it("starts fresh after the user speaks", async () => {
    const { pipeline, cards } = setup(() => ({ events: answer("Sure.") }));
    pipeline.onTurn(turn("t1", "Thanks for joining."));
    pipeline.onTurn(turn("m1", "Happy to be here, thanks.", "me"));
    pipeline.onTurn(turn("t2", "Can you walk me through your resume?"));
    await pipeline.idle();
    expect(cards.cards[0].question).toBe("Can you walk me through your resume?");
  });

  it("restarts an answer that hasn't shown text when the question continues", async () => {
    const { pipeline, cards, transport } = setup((_b, n) =>
      n === 0 ? { events: [{ type: "start", requestId: "x", model: "m" }, { type: "thinking" }], hang: true } : { events: answer("Share price is 24.") },
    );
    expect(pipeline.onTurn(turn("t1", "Given paper with info, calculate share price given PE"))).toBe(true);
    await tick();
    await tick();
    // The interviewer keeps reading the inputs: not a question on its own, but it continues one.
    expect(pipeline.onTurn(turn("t2", "EV, EBITDA, revenue, gross margin, tax rate,"))).toBe(true);
    await pipeline.idle();
    expect(transport.cancelled).toContain("req1");
    expect(cards.cards).toHaveLength(1);
    expect(cards.cards[0].question).toBe("Given paper with info, calculate share price given PE EV, EBITDA, revenue, gross margin, tax rate,");
  });

  it("doesn't repeat pieces of a question that was already answered", async () => {
    const { pipeline, cards } = setup(() => ({ events: answer("Answer.") }));
    pipeline.onTurn(turn("t1", "How would you design a rate limiter?"));
    await pipeline.idle();
    pipeline.onTurn(turn("t2", "Great. We use Kafka here."));
    pipeline.onTurn(turn("t3", "How would you handle backpressure?"));
    await pipeline.idle();
    expect(cards.cards.map((c) => c.question)).toEqual([
      "How would you design a rate limiter?",
      "Great. We use Kafka here. How would you handle backpressure?",
    ]);
  });

  it("maps errors to error cards and never persists them", async () => {
    const { pipeline, cards, completed } = setup(() => ({
      events: [{ type: "error", code: "key_invalid", message: "invalid x-api-key", status: 401, retryable: false }],
    }));
    pipeline.ask({ question: "What is STAR?" });
    await pipeline.idle();
    expect(cards.cards[0].status).toBe("error");
    expect(cards.cards[0].error).toMatchObject({ title: "Your Claude key was rejected", action: "settings", retryable: false });
    expect(cards.cards[0].content).toBe("");
    expect(completed).toHaveLength(0);
  });

  it("maps a rejected llm_stream command (no events) to an error card", async () => {
    const cards = memoryCardSink();
    const pipeline = new AnswerPipeline({
      transport: {
        stream: () => Promise.reject({ code: "network", message: "offline" }),
        cancel: async () => {},
      },
      cards,
      mode: () => "ask",
      promptContext: async (m) => ({ mode: m, modePrompt: "", brief: null, ai: DEFAULT_SETTINGS.ai }),
      transcript: () => [],
    });
    pipeline.ask({ question: "hi" });
    await pipeline.idle();
    expect(cards.cards[0].error).toMatchObject({ code: "network", action: "retry", retryable: true });
  });

  it("notes refusals and cut-off answers", async () => {
    const { pipeline, cards, completed } = setup((_b, n) => ({
      events: answer(n === 0 ? "Partial" : "Long code…", n === 0 ? "refusal" : "max_tokens"),
    }));
    pipeline.ask({ question: "one" });
    await pipeline.idle();
    pipeline.ask({ question: "two" });
    await pipeline.idle();
    expect(cards.cards.map((c) => c.stopNote)).toEqual(["refusal", "max_tokens"]);
    expect(completed.map((c) => c.question)).toEqual(["two"]); // refusals are not saved as answers
  });

  it("answers “What should I say?” without the filter and supersedes auto", async () => {
    const { pipeline, transport, cards } = setup((_b, n) =>
      n === 0 ? { events: [{ type: "thinking" }], hang: true } : { events: answer("Say this.") },
    );
    pipeline.onTurn(turn("t1", "Can you start?"));
    await tick();
    pipeline.askNow();
    await pipeline.idle();
    expect(transport.cancelled).toContain("req1");
    expect(lastUserText(transport.calls[1].body)).toContain(ASK_NOW_INSTRUCTION.slice(0, 40));
    expect(cards.cards.map((c) => c.kind)).toEqual(["ask_now"]);
  });

  it("retries a failed card with the same request", async () => {
    const { pipeline, transport, cards } = setup((_b, n) =>
      n === 0 ? { events: [{ type: "error", code: "overloaded", message: "Overloaded", retryable: true }] } : { events: answer("Recovered.") },
    );
    const id = pipeline.ask({ question: "Explain closures" });
    await pipeline.idle();
    expect(cards.get(id)?.error?.action).toBe("retry");
    pipeline.retry(id);
    await pipeline.idle();
    expect(cards.get(id)).toMatchObject({ status: "done", content: "Recovered." });
    expect(JSON.stringify(transport.calls[0].body)).toBe(JSON.stringify(transport.calls[1].body));
  });

  it("builds follow-ups from the parent answer", async () => {
    const { pipeline, transport, cards } = setup(() => ({ events: answer("Short.") }));
    const parent = pipeline.ask({ question: "Tell me about X" });
    await pipeline.idle();
    const child = pipeline.followup(parent, "shorter");
    await pipeline.idle();
    expect(child).not.toBeNull();
    expect(cards.get(child!)).toMatchObject({ kind: "followup", chip: "shorter", parentId: parent, status: "done" });
    const roles = (transport.calls[1].body.messages as { role: string }[]).map((m) => m.role);
    expect(roles).toEqual(["user", "assistant", "user"]);
  });

  it("can stop a running card explicitly", async () => {
    const { pipeline, cards, transport } = setup(() => ({ events: [{ type: "thinking" }], hang: true }));
    const id = pipeline.ask({ question: "slow one" });
    await tick();
    pipeline.stop(id);
    await pipeline.idle();
    expect(transport.cancelled).toEqual(["req1"]);
    expect(cards.get(id)).toBeUndefined(); // nothing shown yet → removed
  });
});
