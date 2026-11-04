// The question pipeline: decides when to ask Claude, streams answers into
// cards, drops ∅ (not-a-question) results, never answers the same turn twice,
// and maps failures to error cards.
//
// Questions are often spoken in pieces ("If net income is a hundred… P/E eight x…
// calculate the net debt"), and speech-to-text splits them into several turns. The
// other side's turns since the user last spoke are therefore merged into one question.
// A newer turn only replaces an auto-answer that hasn't shown any text yet; an answer
// the user may already be reading is never cancelled by a later remark.
//
// All of this is per conversation: `reset()` must run whenever a new one starts. Turn ids
// restart with every listening session, so a pipeline that kept the previous session's
// answered ids silently ignored the next session's first questions.

import { isDecided, isNullAnswer } from "../ai/answer";
import { lastWindow, type ContextLine } from "../ai/context";
import { isCancellation, mapLlmError } from "../ai/errors";
import { prefilter } from "../ai/prefilter";
import type { FollowupChip } from "../ai/prompts";
import {
  BUDGET,
  buildAskNow,
  buildAutoAnswer,
  buildFollowup,
  buildHeardAnswer,
  buildTypedAsk,
  type HistoryTurn,
  type LlmRequest,
  type PromptContext,
} from "../ai/requests";
import { runStream, type LlmTransport } from "../ai/run";
import { MODE_META } from "../defaults";
import type { Mode, TurnEvent } from "../types";
import { newId, newRequestId } from "../util";
import type { Card, CardKind, CardScreenshot, CardSink } from "./cards";

export interface PipelineDeps {
  transport: LlmTransport;
  cards: CardSink;
  mode(): Mode;
  promptContext(mode: Mode): Promise<PromptContext>;
  /** Final, non-echo transcript lines of the current conversation. */
  transcript(): ContextLine[];
  onComplete?(card: Card): void | Promise<void>;
  ids?: { request(): string; card(): string };
  now?(): number;
  /** Diagnostic log (turn decisions, answer outcomes). Never receives transcript text. */
  log?(message: string): void;
  /** How long to wait for Claude to start replying before retrying (default 8 s). */
  firstResponseMs?: number;
}

export interface AskOptions {
  question: string;
  screenshot?: CardScreenshot | null;
  replyTo?: string | null;
}

/** Pieces of one spoken question are merged when they arrive within this window. */
const FRAGMENT_WINDOW_MS = 60_000;

/**
 * Claude normally starts replying within ~2 s. A request with no reply at all after this
 * long is hung (seen live: 11 s, zero tokens, while the native stall timeout is 120 s), so
 * it is cancelled and sent again once.
 */
const FIRST_RESPONSE_MS = 8_000;
const MAX_ATTEMPTS = 2;

export class AnswerPipeline {
  private answeredTurns = new Set<string>();
  /** cardId → requestId of the stream currently feeding it. */
  private inflight = new Map<string, string>();
  private builders = new Map<string, () => Promise<LlmRequest>>();
  private dropped = new Set<string>();
  private autoCardId: string | null = null;
  /** The other side's turns since the user last spoke (newest last). */
  private fragments: { seq: number; text: string; t1Ms: number }[] = [];
  private fragmentSeq = 0;
  /** Fragments up to this seq already belong to an answer the user has seen. */
  private consumedSeq = 0;
  /** cardId → the last fragment seq its question includes. */
  private cardSeq = new Map<string, number>();
  private running = new Set<Promise<void>>();
  private ids: { request(): string; card(): string };
  private now: () => number;

  constructor(private deps: PipelineDeps) {
    this.ids = deps.ids ?? { request: newRequestId, card: () => newId("card") };
    this.now = deps.now ?? Date.now;
  }

  /**
   * Forgets everything tied to the current conversation: answered turn ids, question
   * pieces, the pending auto-answer. Call it whenever a new conversation starts (every
   * start of listening, "New"). Streams still running are cancelled.
   */
  reset(): void {
    this.stopAll();
    const turns = this.answeredTurns.size;
    const pieces = this.fragments.length;
    this.answeredTurns.clear();
    this.fragments = [];
    this.consumedSeq = this.fragmentSeq;
    this.cardSeq.clear();
    this.builders.clear();
    this.dropped.clear();
    this.autoCardId = null;
    this.log(`reset (forgot ${turns} answered turns, ${pieces} question pieces)`);
  }

  /** Called for every `turn` event. Returns true when an auto-answer started. */
  onTurn(turn: TurnEvent): boolean {
    if (turn.source !== "them") {
      // The user answered or spoke up: the next question starts fresh.
      if (turn.text.trim().split(/\s+/).length >= 3) {
        this.fragments = [];
        this.consumedSeq = this.fragmentSeq;
      }
      return false;
    }
    const words = wordCount(turn.text);
    if (this.answeredTurns.has(turn.turnId)) {
      this.log(`turn ${turn.turnId} words=${words} -> skipped: already handled`);
      return false;
    }
    this.answeredTurns.add(turn.turnId);
    const seq = ++this.fragmentSeq;
    // abs(): a piece from another clock (e.g. a previous session) is never "recent".
    this.fragments = this.fragments.filter((f) => Math.abs(turn.t1Ms - f.t1Ms) <= FRAGMENT_WINDOW_MS);
    if (turn.text.trim()) this.fragments.push({ seq, text: turn.text.trim(), t1Ms: turn.t1Ms });
    const filter = prefilter(turn.text, turn.question);
    const what = `turn ${turn.turnId} words=${words} filter=${filter.reason}`;

    const mode = this.deps.mode();
    if (!MODE_META[mode].autoAnswers) {
      // No auto-answers here (Ask, Lecture): offer a one-click answer instead of staying silent.
      if (filter.candidate && this.offerHeard(mode, seq, turn.turnId)) this.log(`${what} -> question heard card (${mode} mode)`);
      else this.log(`${what} -> skipped: ${filter.candidate ? "empty" : "not a question"} (${mode} mode)`);
      return false;
    }

    // An auto-answer still deciding (nothing on screen yet) is superseded by a fuller
    // version of its question; one that is already showing text is left alone.
    const pending = this.autoCardId ? this.deps.cards.get(this.autoCardId) : undefined;
    const pendingUnseen = !!pending && pending.kind === "auto" && !pending.content && this.inflight.has(pending.id);
    // A non-question fragment only matters as the continuation of a question being answered,
    // and only while that question is still open: once the user has spoken, a stray "mm-hmm"
    // must not replace the pending answer with an answer to itself.
    const continues = !!pending && pendingUnseen && (this.cardSeq.get(pending.id) ?? 0) > this.consumedSeq;
    if (!filter.candidate && !continues) {
      this.log(`${what} -> skipped: not a question`);
      return false;
    }

    const pieces = this.fragments.filter((f) => f.seq > this.consumedSeq);
    const question = pieces.map((f) => f.text).join(" ");
    if (!question) {
      this.log(`${what} -> skipped: empty`);
      return false;
    }
    if (pendingUnseen && pending) this.stop(pending.id);

    const card = this.newCard("auto", mode, { question, turnId: turn.turnId, provisional: true });
    this.autoCardId = card.id;
    this.cardSeq.set(card.id, seq);
    this.log(
      `${what} -> answering ${card.id} pieces=${pieces.length}${pendingUnseen && pending ? ` (replaces ${pending.id})` : ""}`,
    );
    this.start(card.id, async () =>
      buildAutoAnswer(await this.deps.promptContext(mode), this.deps.transcript(), question),
    );
    return true;
  }

  /** Shows a heard question (no auto-answer in this mode) with an Answer button. */
  private offerHeard(mode: Mode, seq: number, turnId: string): boolean {
    const question = this.fragments
      .filter((f) => f.seq > this.consumedSeq)
      .map((f) => f.text)
      .join(" ");
    if (!question) return false;
    // Keep only the latest unanswered one so the panel doesn't fill up.
    for (const c of this.deps.cards.all()) if (c.kind === "heard") this.deps.cards.remove(c.id);
    const card = this.newCard("heard", mode, { question, turnId, status: "done", heardReason: "mode" });
    this.cardSeq.set(card.id, seq);
    this.consumedSeq = seq;
    return true;
  }

  /** The user clicked Answer on a heard (or skipped) question. */
  answerHeard(cardId: string): void {
    const card = this.deps.cards.get(cardId);
    if (!card || card.kind !== "heard") return;
    const mode = this.deps.mode();
    const question = card.question;
    // Its pieces are answered now; the next question doesn't repeat them.
    this.consumedSeq = Math.max(this.consumedSeq, this.cardSeq.get(cardId) ?? 0);
    this.dropped.delete(cardId);
    this.deps.cards.update(cardId, {
      kind: "auto",
      mode,
      status: "pending",
      content: "",
      heardReason: undefined,
      provisional: false,
      createdAt: this.now(),
    });
    this.log(`answer ${cardId} -> answering on request (${card.heardReason ?? "heard"})`);
    this.start(cardId, async () =>
      buildHeardAnswer(await this.deps.promptContext(mode), this.deps.transcript(), question),
    );
  }

  /** "What should I say?" — answers from the last ~2 minutes, no filter. */
  askNow(): string {
    if (this.autoCardId) this.stop(this.autoCardId);
    const mode = this.deps.mode();
    const card = this.newCard("ask_now", mode, { question: "What should I say?" });
    this.start(card.id, async () =>
      buildAskNow(
        await this.deps.promptContext(mode),
        lastWindow(this.deps.transcript(), BUDGET.askNowWindowMs),
      ),
    );
    return card.id;
  }

  /** A typed question, a screenshot, or both. */
  ask(opts: AskOptions): string {
    const mode = this.deps.mode();
    const kind: CardKind = opts.screenshot ? "screen" : opts.replyTo ? "followup" : "typed";
    const card = this.newCard(kind, mode, {
      question: opts.question.trim(),
      screenshot: opts.screenshot ?? undefined,
      parentId: opts.replyTo ?? undefined,
    });
    const history = opts.replyTo ? this.historyFor(opts.replyTo) : this.history(card.id);
    const shot = opts.screenshot ? { id: opts.screenshot.id, mime: opts.screenshot.mime } : null;
    const question = opts.question;
    this.start(card.id, async () =>
      buildTypedAsk(await this.deps.promptContext(mode), {
        question,
        screenshot: shot,
        transcript: this.deps.transcript(),
        history,
      }),
    );
    return card.id;
  }

  /** Shorter / More detail / Example / continue on an existing answer. */
  followup(parentId: string, chip: FollowupChip): string | null {
    const parent = this.deps.cards.get(parentId);
    if (!parent || !parent.content.trim()) return null;
    const mode = this.deps.mode();
    const card = this.newCard("followup", mode, { question: parent.question, parentId, chip });
    const turn = this.toHistory(parent);
    this.start(card.id, async () => buildFollowup(await this.deps.promptContext(mode), turn, chip));
    return card.id;
  }

  retry(cardId: string): void {
    const builder = this.builders.get(cardId);
    const card = this.deps.cards.get(cardId);
    if (!builder || !card) return;
    this.deps.cards.update(cardId, {
      status: "pending",
      content: "",
      error: undefined,
      stopNote: undefined,
      createdAt: this.now(),
    });
    this.start(cardId, builder);
  }

  stop(cardId: string): void {
    const requestId = this.inflight.get(cardId);
    if (!requestId) return;
    void this.deps.transport.cancel(requestId).catch(() => {});
  }

  stopAll(): void {
    for (const cardId of [...this.inflight.keys()]) this.stop(cardId);
  }

  isBusy(cardId: string): boolean {
    return this.inflight.has(cardId);
  }

  /** Resolves when every running stream has finished (tests, shutdown). */
  async idle(): Promise<void> {
    while (this.running.size) await Promise.all([...this.running]);
  }

  // -------------------------------------------------------------------------

  private log(message: string): void {
    try {
      this.deps.log?.(message);
    } catch {
      /* logging never breaks answers */
    }
  }

  /** An auto-answer Claude judged "not a question for you": keep it as a skipped card with an Answer button. */
  private toSkipped(cardId: string): void {
    const { cards } = this.deps;
    if (!cards.get(cardId)) return;
    // Only the latest skipped/heard question stays, so the panel doesn't fill up.
    for (const c of cards.all()) if (c.kind === "heard" && c.id !== cardId) cards.remove(c.id);
    cards.update(cardId, { kind: "heard", heardReason: "not_question", status: "done", content: "", provisional: false });
  }

  /** Removes skipped cards whose pieces are part of an answer now on screen. */
  private removeSkippedUpTo(seq: number, exceptId: string): void {
    const { cards } = this.deps;
    for (const c of cards.all()) {
      if (c.id === exceptId || c.kind !== "heard" || c.heardReason !== "not_question") continue;
      if ((this.cardSeq.get(c.id) ?? Infinity) <= seq) cards.remove(c.id);
    }
  }

  private newCard(kind: CardKind, mode: Mode, extra: Partial<Card>): Card {
    const card: Card = {
      id: this.ids.card(),
      kind,
      mode,
      question: "",
      status: "pending",
      content: "",
      createdAt: this.now(),
      ...extra,
    };
    this.deps.cards.add(card);
    return card;
  }

  private toHistory(c: Card): HistoryTurn {
    return { question: c.question, answer: c.content, hadScreenshot: !!c.screenshot };
  }

  /** Completed answers in this conversation, oldest first. */
  private history(excludeId?: string): HistoryTurn[] {
    return this.deps.cards
      .all()
      .filter((c) => c.id !== excludeId && c.kind !== "summary" && c.status === "done" && c.content.trim())
      .map((c) => this.toHistory(c));
  }

  private historyFor(cardId: string): HistoryTurn[] {
    const c = this.deps.cards.get(cardId);
    return c && c.content.trim() ? [this.toHistory(c)] : this.history();
  }

  private start(cardId: string, builder: () => Promise<LlmRequest>, attempt = 1): void {
    this.builders.set(cardId, builder);
    const p = this.execute(cardId, builder, attempt).finally(() => this.running.delete(p));
    this.running.add(p);
  }

  private async execute(cardId: string, builder: () => Promise<LlmRequest>, attempt = 1): Promise<void> {
    const { cards, transport } = this.deps;
    const requestId = this.ids.request();
    const kind = cards.get(cardId)?.kind ?? "?";
    const startedAt = this.now();
    const ms = () => this.now() - startedAt;
    this.inflight.set(cardId, requestId);
    // A newer stream for the same card (Answer it, Retry) takes over its events.
    const current = () => this.inflight.get(cardId) === requestId;

    let req: LlmRequest;
    try {
      req = await builder();
    } catch (e) {
      if (current()) this.inflight.delete(cardId);
      const err = e as { code?: string; message?: string };
      cards.update(cardId, { status: "error", error: mapLlmError({ code: err.code ?? "unknown", message: err.message }) });
      this.log(`answer ${cardId} ${kind} -> error:${err.code ?? "unknown"} (request not built)`);
      return;
    }

    const provisional = !!cards.get(cardId)?.provisional;
    let decided = !provisional;
    let nulled = false;
    // No reply at all for a while: the request is hung. Cancel it (and retry below).
    let replied = false;
    let stalled = false;
    const firstResponseMs = this.deps.firstResponseMs ?? FIRST_RESPONSE_MS;
    const watchdog = setTimeout(() => {
      if (replied || !current()) return;
      stalled = true;
      void transport.cancel(requestId).catch(() => {});
    }, firstResponseMs);

    const outcome = await runStream(transport, requestId, req, {
      onStart: () => {
        replied = true;
      },
      onThinking: () => {
        replied = true;
        if (this.dropped.has(cardId) || !current()) return;
        const c = cards.get(cardId);
        if (c && !c.content) cards.update(cardId, { status: "thinking" });
      },
      onText: (_delta, full) => {
        replied = true;
        if (this.dropped.has(cardId) || !current()) return;
        if (!decided) {
          if (!isDecided(full)) return;
          if (isNullAnswer(full)) {
            // Claude says it isn't a question for the user: stop paying for tokens, but
            // keep it visible as a skipped question the user can still answer.
            nulled = true;
            this.dropped.add(cardId);
            void transport.cancel(requestId).catch(() => {});
            this.toSkipped(cardId);
            return;
          }
          decided = true;
          // The user can see this answer now: its question's pieces are used up, and any
          // skipped piece it includes no longer needs its own card.
          const seq = this.cardSeq.get(cardId) ?? 0;
          this.consumedSeq = Math.max(this.consumedSeq, seq);
          this.removeSkippedUpTo(seq, cardId);
          this.log(`answer ${cardId} ${kind} -> shown first_text_ms=${ms()}`);
        }
        if (isNullAnswer(full)) return; // never flash a bare ∅ (handled when the stream ends)
        cards.update(cardId, { content: full, status: "streaming", provisional: false });
      },
    });

    clearTimeout(watchdog);
    const wasCurrent = current();
    if (stalled && wasCurrent && !nulled && !replied && cards.get(cardId)) {
      const secs = Math.round(firstResponseMs / 1000);
      if (attempt < MAX_ATTEMPTS) {
        // Same card, same question, fresh request; it stays the pending auto-answer.
        this.log(`answer ${cardId} ${kind} -> no reply in ${secs}s, retrying (attempt ${attempt + 1})`);
        this.start(cardId, builder, attempt + 1);
        return;
      }
      this.inflight.delete(cardId);
      if (this.autoCardId === cardId) this.autoCardId = null;
      cards.update(cardId, {
        status: "error",
        provisional: false,
        error: {
          code: "no_response",
          title: "Claude didn't respond",
          message: `No reply within ${secs} seconds, twice. Check your connection, then retry.`,
          action: "retry",
          retryable: true,
        },
      });
      this.log(`answer ${cardId} ${kind} -> error:no_response after ${attempt} attempts ms=${ms()}`);
      return;
    }
    if (wasCurrent) this.inflight.delete(cardId);
    if (this.autoCardId === cardId) this.autoCardId = null;
    if (nulled) {
      this.log(`answer ${cardId} ${kind} -> not a question (null) ms=${ms()}`);
      return;
    }
    if (!wasCurrent) return;
    if (this.dropped.has(cardId)) return;
    const card = cards.get(cardId);
    if (!card) {
      this.log(`answer ${cardId} ${kind} -> gone (conversation cleared) ms=${ms()}`);
      return;
    }

    if (outcome.kind === "cancelled") {
      if (card.content.trim()) cards.update(cardId, { status: "stopped" });
      else cards.remove(cardId);
      this.log(`answer ${cardId} ${kind} -> cancelled ${card.content.trim() ? "after text" : "before text"} ms=${ms()}`);
      return;
    }

    if (outcome.kind === "error") {
      if (isCancellation(outcome.code)) {
        cards.remove(cardId);
        this.log(`answer ${cardId} ${kind} -> cancelled before text ms=${ms()}`);
        return;
      }
      // Even an auto-answer that never showed text gets an error card: the user
      // needs to know answers aren't coming.
      cards.update(cardId, {
        status: "error",
        provisional: false,
        error: mapLlmError({ code: outcome.code, message: outcome.message, retryable: outcome.retryable }),
      });
      this.log(`answer ${cardId} ${kind} -> error:${outcome.code} ms=${ms()}`);
      return;
    }

    // done
    const text = outcome.text;
    if (provisional && (!text.trim() || isNullAnswer(text))) {
      if (text.trim()) this.toSkipped(cardId);
      else cards.remove(cardId);
      this.log(`answer ${cardId} ${kind} -> ${text.trim() ? "not a question (null)" : "empty"} ms=${ms()}`);
      return;
    }
    if (isNullAnswer(text)) {
      // Asked for explicitly and still "not a question": say so instead of showing a bare ∅.
      cards.update(cardId, {
        status: "error",
        content: "",
        error: {
          code: "no_answer",
          title: "Nothing to answer",
          message: "Claude judged this isn't something you need to answer.",
          action: "retry",
          retryable: true,
        },
      });
      this.log(`answer ${cardId} ${kind} -> not a question (null) on request ms=${ms()}`);
      return;
    }
    const stopNoteLog = outcome.stopReason && outcome.stopReason !== "end_turn" ? ` stop=${outcome.stopReason}` : "";
    this.log(`answer ${cardId} ${kind} -> done ms=${ms()} chars=${text.length}${stopNoteLog}`);
    const stopNote =
      outcome.stopReason === "refusal" ? "refusal" : outcome.stopReason === "max_tokens" ? "max_tokens" : undefined;
    cards.update(cardId, { status: "done", content: text, stopNote, provisional: false });
    const done = cards.get(cardId);
    if (done && done.content.trim() && stopNote !== "refusal") {
      try {
        await this.deps.onComplete?.(done);
      } catch {
        // Persistence failures never break the overlay.
      }
    }
  }
}

function wordCount(text: string): number {
  const t = text.trim();
  return t ? t.split(/\s+/).length : 0;
}
