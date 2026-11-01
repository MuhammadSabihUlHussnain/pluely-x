import { describe, expect, it } from "vitest";
import { DEFAULT_SETTINGS } from "../defaults";
import { formatTranscript, OMITTED_MARKER, type ContextLine } from "./context";
import { AUTO_ANSWER_INSTRUCTION, DEFAULT_MODE_PROMPTS, NULL_SENTINEL } from "./prompts";
import {
  buildAskNow,
  buildAutoAnswer,
  buildFollowup,
  buildNotes,
  buildSummary,
  buildSystem,
  buildTypedAsk,
  FALLBACK_BETA,
  type LlmRequest,
  type PromptContext,
} from "./requests";

const ctx = (over: Partial<PromptContext> = {}): PromptContext => ({
  mode: "interview",
  modePrompt: DEFAULT_MODE_PROMPTS.interview,
  brief: { name: "Résumé", content: "Senior engineer at Northwind. Kafka, Go, Postgres." },
  ai: DEFAULT_SETTINGS.ai,
  ...over,
});

const lines: ContextLine[] = [
  { source: "me", text: "Sure, happy to.", t0Ms: 5000 },
  { source: "them", text: "Thanks for joining.", t0Ms: 1000 },
  { source: "them", text: "Can you walk me through a project?", t0Ms: 2500 },
  { source: "me", text: "Yes.", t0Ms: 7000 },
];

function allRequests(): LlmRequest[] {
  const c = ctx();
  return [
    buildAutoAnswer(c, lines, "Can you walk me through a project?"),
    buildAskNow(c, lines),
    buildTypedAsk(c, { question: "What is STAR?", transcript: lines }),
    buildTypedAsk(c, { question: "", screenshot: { id: "shot_1", mime: "image/png" } }),
    buildFollowup(c, { question: "Q", answer: "A\n- b" }, "shorter"),
    buildNotes(ctx({ mode: "meeting" }), "meeting", null, lines),
    buildSummary(ctx({ mode: "lecture" }), lines, [{ question: "Q", answer: "A" }]),
  ];
}

function textOfLastUser(req: LlmRequest): string {
  const last = req.body.messages[req.body.messages.length - 1];
  if (typeof last.content === "string") return last.content;
  return (last.content ?? []).map((b) => ("text" in b ? (b as { text: string }).text : "")).join("\n");
}

describe("transcript context", () => {
  it("is chronological with Them/You labels and merges same-speaker runs", () => {
    expect(formatTranscript(lines, 10_000)).toBe(
      "Them: Thanks for joining. Can you walk me through a project?\nYou: Sure, happy to. Yes.",
    );
  });

  it("keeps the most recent lines within the budget", () => {
    const many: ContextLine[] = Array.from({ length: 50 }, (_, i) => ({
      source: i % 2 ? "me" : "them",
      text: `line number ${i}`,
      t0Ms: i * 1000,
    }));
    const out = formatTranscript(many, 120);
    expect(out.startsWith(OMITTED_MARKER)).toBe(true);
    expect(out).toContain("line number 49");
    expect(out).not.toContain("line number 1\n");
    expect(out.length).toBeLessThanOrEqual(120 + OMITTED_MARKER.length + 1);
  });
});

describe("request builders", () => {
  it("never sends sampling params or a thinking field", () => {
    for (const r of allRequests()) {
      const json = JSON.stringify(r.body);
      for (const banned of ['"temperature"', '"top_p"', '"top_k"', '"thinking"', '"stream"']) {
        expect(json).not.toContain(banned);
      }
    }
  });

  it("always opts into refusal fallbacks with the beta header", () => {
    for (const r of allRequests()) {
      expect(r.body.fallbacks).toBe("default");
      expect(r.betas).toContain(FALLBACK_BETA);
      expect(FALLBACK_BETA).toBe("server-side-fallback-2026-07-01");
    }
  });

  it("uses the configured model and max_tokens", () => {
    for (const r of allRequests()) {
      expect(r.body.model).toBe("claude-opus-5-5");
      expect(r.body.max_tokens).toBe(16000);
    }
  });

  it("uses live effort for live answers and deep effort for solve/notes/summary", () => {
    const [auto, askNow, typed, screen, follow, notes, summary] = allRequests();
    expect(auto.body.output_config?.effort).toBe("low");
    expect(askNow.body.output_config?.effort).toBe("low");
    expect(typed.body.output_config?.effort).toBe("low");
    expect(follow.body.output_config?.effort).toBe("low");
    expect(screen.body.output_config?.effort).toBe("low"); // interview mode screen = live
    expect(notes.body.output_config?.effort).toBe("medium");
    expect(summary.body.output_config?.effort).toBe("medium");
    const solve = buildTypedAsk(ctx({ mode: "coding", modePrompt: DEFAULT_MODE_PROMPTS.coding }), {
      question: "",
      screenshot: { id: "s", mime: "image/jpeg" },
    });
    expect(solve.body.output_config?.effort).toBe("medium");
  });

  it("keeps the cached system prefix byte-identical across calls and transcripts", () => {
    const a = buildAutoAnswer(ctx(), lines, "Can you walk me through a project?");
    const b = buildTypedAsk(ctx(), { question: "Something else", transcript: lines.slice(0, 1) });
    const c = buildAskNow(ctx(), []);
    expect(JSON.stringify(a.body.system)).toBe(JSON.stringify(b.body.system));
    expect(JSON.stringify(a.body.system)).toBe(JSON.stringify(c.body.system));
    expect(JSON.stringify(buildSystem(ctx()))).toBe(JSON.stringify(buildSystem(ctx())));
  });

  it("puts cache_control on the last stable system block only, and nothing volatile in it", () => {
    const sys = buildSystem(ctx());
    expect(sys).toHaveLength(3);
    expect(sys.slice(0, -1).every((b) => !b.cache_control)).toBe(true);
    expect(sys[sys.length - 1].cache_control).toEqual({ type: "ephemeral" });
    expect(sys[2].text).toContain("Senior engineer at Northwind");
    const joined = sys.map((b) => b.text).join("\n");
    expect(joined).not.toMatch(/\b20\d\d-\d\d-\d\d\b/);
    expect(joined).not.toContain("Them: ");
    // without a brief the mode prompt carries the breakpoint
    const noBrief = buildSystem(ctx({ brief: null }));
    expect(noBrief).toHaveLength(2);
    expect(noBrief[1].cache_control).toEqual({ type: "ephemeral" });
  });

  it("places the transcript and question after the cached prefix", () => {
    const r = buildAutoAnswer(ctx(), lines, "Can you walk me through a project?");
    const user = textOfLastUser(r);
    expect(user).toContain("<transcript>\nThem: Thanks for joining.");
    expect(user).toContain("<latest_them_turn>\nCan you walk me through a project?\n</latest_them_turn>");
    expect(user).toContain(AUTO_ANSWER_INSTRUCTION);
    expect(AUTO_ANSWER_INSTRUCTION).toContain(`output exactly ${NULL_SENTINEL}`);
    // ∅ instruction only on auto-answers
    expect(textOfLastUser(buildAskNow(ctx(), lines))).not.toContain(NULL_SENTINEL);
    expect(textOfLastUser(buildTypedAsk(ctx(), { question: "hi" }))).not.toContain(NULL_SENTINEL);
  });

  it("sends history oldest first and ends on a user turn", () => {
    const r = buildTypedAsk(ctx(), {
      question: "third",
      history: [
        { question: "first", answer: "A1" },
        { question: "second", answer: "A2" },
      ],
    });
    const roles = r.body.messages.map((m) => m.role);
    expect(roles).toEqual(["user", "assistant", "user", "assistant", "user"]);
    expect(JSON.stringify(r.body.messages[0])).toContain("first");
    expect(JSON.stringify(r.body.messages[2])).toContain("second");
  });

  it("references screenshots by id instead of shipping bytes", () => {
    const r = buildTypedAsk(ctx(), { question: "what is this", screenshot: { id: "shot_9", mime: "image/png" } });
    const json = JSON.stringify(r.body);
    expect(json).toContain('"data":"@screenshot:shot_9"');
    expect(json).toContain('"media_type":"image/png"');
  });

  it("requests JSON-schema structured output for notes and summaries", () => {
    const notes = buildNotes(ctx({ mode: "meeting" }), "meeting", { decisions: [] }, lines);
    const fmt = notes.body.output_config?.format;
    expect(fmt?.type).toBe("json_schema");
    const schema = fmt!.schema as { additionalProperties: boolean; required: string[] };
    expect(schema.additionalProperties).toBe(false);
    expect(schema.required).toEqual(["decisions", "action_items", "open_questions"]);
    const summary = buildSummary(ctx({ mode: "meeting" }), lines, []);
    expect((summary.body.output_config?.format?.schema as { required: string[] }).required).toContain("follow_up_email");
  });
});
