// Builds Messages API request bodies. Bodies are typed with @anthropic-ai/sdk
// beta types but are never sent from the webview: Rust's llm_stream forwards
// them to api.anthropic.com. Rules (see docs/PLAN.md "AI layer"):
// - no `thinking` field (always on for claude-opus-5-5), effort via output_config
// - no temperature / top_p / top_k
// - refusal fallbacks: fallbacks "default" + beta server-side-fallback-2026-07-01
// - cached, byte-stable system prefix: base contract + mode prompt + brief,
//   cache_control on the last stable block; volatile content goes after it.

import type {
  BetaContentBlockParam,
  BetaImageBlockParam,
  BetaMessageParam,
  BetaTextBlockParam,
  MessageCreateParamsBase,
} from "@anthropic-ai/sdk/resources/beta/messages/messages";
import type { AppSettings, Effort, Mode } from "../types";
import { formatTranscript, type ContextLine } from "./context";
import {
  ASK_NOW_INSTRUCTION,
  AUTO_ANSWER_INSTRUCTION,
  BASE_SYSTEM,
  BRIEF_PREAMBLE,
  FOLLOWUP_INSTRUCTION,
  NOTES_INSTRUCTION,
  SCREEN_INSTRUCTION,
  SUMMARY_INSTRUCTION,
  TYPED_INSTRUCTION,
  type FollowupChip,
} from "./prompts";
import { NOTES_SCHEMAS, SUMMARY_SCHEMAS } from "./schemas";

export const FALLBACK_BETA = "server-side-fallback-2026-07-01";

/** The JSON body handed to llm_stream (Rust forces `stream: true`). */
export type LlmBody = Omit<MessageCreateParamsBase, "betas" | "stream">;

export interface LlmRequest {
  body: LlmBody;
  betas: string[];
}

/** Context budgets, in characters (≈4 chars per token). */
export const BUDGET = {
  autoTranscript: 6_000,
  askNowTranscript: 8_000,
  askNowWindowMs: 120_000,
  typedTranscript: 6_000,
  history: 12_000,
  notesTranscript: 24_000,
  summaryTranscript: 200_000,
  briefMax: 60_000,
} as const;

export interface PromptContext {
  mode: Mode;
  modePrompt: string;
  brief?: { name: string; content: string } | null;
  ai: AppSettings["ai"];
}

export interface HistoryTurn {
  question: string;
  answer: string;
  hadScreenshot?: boolean;
}

export interface ScreenshotRef {
  id: string;
  mime: string;
}

// ---------------------------------------------------------------------------
// Stable prefix

/**
 * System blocks: base contract, mode prompt, optional brief. Identical inputs
 * always produce identical bytes, and the cache breakpoint sits on the last
 * block. Nothing volatile (time, ids, transcript) may ever be added here.
 */
export function buildSystem(ctx: Pick<PromptContext, "modePrompt" | "brief">): BetaTextBlockParam[] {
  const blocks: BetaTextBlockParam[] = [
    { type: "text", text: BASE_SYSTEM },
    { type: "text", text: ctx.modePrompt.trim() },
  ];
  const brief = ctx.brief?.content?.trim();
  if (brief) {
    const content = brief.length > BUDGET.briefMax ? brief.slice(0, BUDGET.briefMax) : brief;
    blocks.push({
      type: "text",
      text: `${BRIEF_PREAMBLE}\n<brief name="${escapeAttr(ctx.brief!.name)}">\n${content}\n</brief>`,
    });
  }
  blocks[blocks.length - 1] = { ...blocks[blocks.length - 1], cache_control: { type: "ephemeral" } };
  return blocks;
}

function escapeAttr(s: string): string {
  return s.replace(/["<>]/g, "");
}

function baseBody(
  ctx: PromptContext,
  effort: Effort,
  messages: BetaMessageParam[],
  format?: { schema: Record<string, unknown> },
): LlmBody {
  const body: LlmBody = {
    model: ctx.ai.model,
    max_tokens: ctx.ai.maxTokens,
    system: buildSystem(ctx),
    messages,
    output_config: format
      ? { effort, format: { type: "json_schema", schema: format.schema } }
      : { effort },
    fallbacks: "default",
  };
  return body;
}

function request(body: LlmBody): LlmRequest {
  return { body, betas: [FALLBACK_BETA] };
}

// ---------------------------------------------------------------------------
// Message helpers

function historyMessages(history: HistoryTurn[] | undefined): BetaMessageParam[] {
  if (!history?.length) return [];
  // Newest pairs win the budget, but they are emitted oldest first.
  const kept: HistoryTurn[] = [];
  let used = 0;
  for (let i = history.length - 1; i >= 0; i--) {
    const h = history[i];
    const size = h.question.length + h.answer.length;
    if (kept.length > 0 && used + size > BUDGET.history) break;
    kept.unshift(h);
    used += size;
  }
  const out: BetaMessageParam[] = [];
  for (const h of kept) {
    const q = h.question.trim() || (h.hadScreenshot ? "(asked about a screenshot)" : "(question)");
    out.push({
      role: "user",
      content: `<question>\n${q}\n</question>${h.hadScreenshot ? "\n[A screenshot was attached to this question.]" : ""}`,
    });
    out.push({ role: "assistant", content: h.answer.trim() || "(no answer)" });
  }
  return out;
}

function transcriptBlock(lines: ContextLine[] | undefined, maxChars: number): BetaTextBlockParam[] {
  if (!lines?.length) return [];
  const text = formatTranscript(lines, maxChars);
  if (!text) return [];
  return [{ type: "text", text: `<transcript>\n${text}\n</transcript>` }];
}

export function screenshotBlock(shot: ScreenshotRef): BetaImageBlockParam {
  return {
    type: "image",
    source: {
      type: "base64",
      media_type: (shot.mime === "image/png" ? "image/png" : "image/jpeg") as "image/png" | "image/jpeg",
      // Rust swaps "@screenshot:<id>" for the stored image bytes.
      data: `@screenshot:${shot.id}`,
    },
  };
}

function userMessage(blocks: BetaContentBlockParam[]): BetaMessageParam {
  return { role: "user", content: blocks };
}

// ---------------------------------------------------------------------------
// Request kinds

/** Auto-answer for a candidate Them turn (single-pass detect + answer). */
export function buildAutoAnswer(
  ctx: PromptContext,
  transcript: ContextLine[],
  latestThemTurn: string,
): LlmRequest {
  const blocks: BetaContentBlockParam[] = [
    ...transcriptBlock(transcript, BUDGET.autoTranscript),
    {
      type: "text",
      text: `<latest_them_turn>\n${latestThemTurn.trim()}\n</latest_them_turn>\n${AUTO_ANSWER_INSTRUCTION}`,
    },
  ];
  return request(baseBody(ctx, ctx.ai.liveEffort, [userMessage(blocks)]));
}

/** Manual "What should I say?": last ~2 minutes, no filter. */
export function buildAskNow(ctx: PromptContext, transcriptLastWindow: ContextLine[]): LlmRequest {
  const blocks: BetaContentBlockParam[] = [
    ...transcriptBlock(transcriptLastWindow, BUDGET.askNowTranscript),
    { type: "text", text: ASK_NOW_INSTRUCTION },
  ];
  if (blocks.length === 1) {
    blocks.unshift({ type: "text", text: "<transcript>\n(nothing has been said yet)\n</transcript>" });
  }
  return request(baseBody(ctx, ctx.ai.liveEffort, [userMessage(blocks)]));
}

/** A typed question, optionally with a screenshot and the live transcript. */
export function buildTypedAsk(
  ctx: PromptContext,
  opts: {
    question: string;
    screenshot?: ScreenshotRef | null;
    transcript?: ContextLine[];
    history?: HistoryTurn[];
  },
): LlmRequest {
  const q = opts.question.trim();
  const blocks: BetaContentBlockParam[] = [...transcriptBlock(opts.transcript, BUDGET.typedTranscript)];
  if (opts.screenshot) blocks.push(screenshotBlock(opts.screenshot));
  if (q) {
    blocks.push({ type: "text", text: `<question>\n${q}\n</question>\n${TYPED_INSTRUCTION}` });
  } else {
    blocks.push({ type: "text", text: SCREEN_INSTRUCTION[ctx.mode] });
  }
  // Solving a screen in Coding mode is deep work; everything else is live.
  const effort = opts.screenshot && ctx.mode === "coding" ? ctx.ai.deepEffort : ctx.ai.liveEffort;
  return request(
    baseBody(ctx, effort, [...historyMessages(opts.history), userMessage(blocks)]),
  );
}

/** Follow-up chip (Shorter / More detail / Example) on an existing card. */
export function buildFollowup(
  ctx: PromptContext,
  parent: HistoryTurn,
  chip: FollowupChip,
  earlier: HistoryTurn[] = [],
): LlmRequest {
  const messages = [
    ...historyMessages([...earlier, parent]),
    userMessage([{ type: "text", text: FOLLOWUP_INSTRUCTION[chip] }]),
  ];
  return request(baseBody(ctx, ctx.ai.liveEffort, messages));
}

/** Rolling notes (Meeting: decisions/actions, Lecture: key points/terms). */
export function buildNotes(
  ctx: PromptContext,
  kind: "meeting" | "lecture",
  currentNotes: unknown,
  newTranscript: ContextLine[],
): LlmRequest {
  const blocks: BetaContentBlockParam[] = [
    { type: "text", text: `<current_notes>\n${JSON.stringify(currentNotes ?? {})}\n</current_notes>` },
    ...transcriptBlock(newTranscript, BUDGET.notesTranscript),
    { type: "text", text: NOTES_INSTRUCTION[kind] },
  ];
  return request(
    baseBody(ctx, ctx.ai.deepEffort, [userMessage(blocks)], { schema: NOTES_SCHEMAS[kind] }),
  );
}

/** End-of-session summary for the session's mode. */
export function buildSummary(
  ctx: PromptContext,
  transcript: ContextLine[],
  answers: HistoryTurn[],
): LlmRequest {
  const blocks: BetaContentBlockParam[] = [...transcriptBlock(transcript, BUDGET.summaryTranscript)];
  if (answers.length) {
    const list = answers
      .map((a, i) => `${i + 1}. Q: ${a.question || "(screenshot)"}\n   A: ${a.answer.split("\n")[0]}`)
      .join("\n");
    blocks.push({ type: "text", text: `<answer_cards>\n${list}\n</answer_cards>` });
  }
  blocks.push({ type: "text", text: SUMMARY_INSTRUCTION[ctx.mode] });
  return request(
    baseBody(ctx, ctx.ai.deepEffort, [userMessage(blocks)], { schema: SUMMARY_SCHEMAS[ctx.mode] }),
  );
}
