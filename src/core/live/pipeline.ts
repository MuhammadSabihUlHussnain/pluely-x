// The question pipeline: decides when to ask Claude, streams answers into
// cards, drops ∅ (not-a-question) results, never answers the same turn twice,
// and maps failures to error cards.
//
// Questions are often spoken in pieces ("If net income is a hundred… P/E eight x…
// calculate the net debt"), and speech-to-text splits them into several turns. The
// other side's turns since the user last spoke are therefore merged into one question.
// A newer turn only replaces an auto-answer that hasn't shown any text yet; an answer
// the user may already be reading is never cancelled by a later remark.

import { isDecided, isNullAnswer } from "../ai/answer";
import { lastWindow, type ContextLine } from "../ai/context";
import { isCancellation, mapLlmError } from "../ai/errors";
import { isCandidateQuestion } from "../ai/prefilter";
import type { FollowupChip } from "../ai/prompts";
import {
  BUDGET,
  buildAskNow,
  buildAutoAnswer,
  buildFollowup,
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
}

export interface AskOptions {
  question: string;
  screenshot?: CardScreenshot | null;
  replyTo?: string | null;
}

/** Pieces of one spoken question are merged when they arrive within this window. */
const FRAGMENT_WINDOW_MS = 60_000;

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
    if (this.answeredTurns.has(turn.turnId)) return false;
    this.answeredTurns.add(turn.turnId);
    const seq = ++this.fragmentSeq;
    this.fragments = this.fragments.filter((f) => turn.t1Ms - f.t1Ms <= FRAGMENT_WINDOW_MS);
    if (turn.text.trim()) this.fragments.push({ seq, text: turn.text.trim(), t1Ms: turn.t1Ms });

    const mode = this.deps.mode();
    if (!MODE_META[mode].autoAnswers) return false;

    // An auto-answer still deciding (nothing on screen yet) is superseded by a fuller
    // version of its question; one that is already showing text is left alone.
    const pending = this.autoCardId ? this.deps.cards.get(this.autoCardId) : undefined;
    const pendingUnseen = !!pending && !pending.content && this.inflight.has(pending.id);
    const candidate = isCandidateQuestion(turn.text, turn.question);
    // A non-question fragment only matters as the continuation of a question being answered.
    if (!candidate && !pendingUnseen) return false;

    const question = this.fragments
      .filter((f) => f.seq > this.consumedSeq)
      .map((f) => f.text)
      .join(" ");
    if (!question) return false;
    if (pendingUnseen && pending) this.stop(pending.id);

    const card = this.newCard("auto", mode, { question, turnId: turn.turnId, provisional: true });
    this.autoCardId = card.id;
    this.cardSeq.set(card.id, seq);
    this.start(card.id, async () =>
      buildAutoAnswer(await this.deps.promptContext(mode), this.deps.transcript(), question),
    );
    return true;
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

  private start(cardId: string, builder: () => Promise<LlmRequest>): void {
    this.builders.set(cardId, builder);
    const p = this.execute(cardId, builder).finally(() => this.running.delete(p));
    this.running.add(p);
  }

  private async execute(cardId: string, builder: () => Promise<LlmRequest>): Promise<void> {
    const { cards, transport } = this.deps;
    const requestId = this.ids.request();
    this.inflight.set(cardId, requestId);

    let req: LlmRequest;
    try {
      req = await builder();
    } catch (e) {
      this.inflight.delete(cardId);
      const err = e as { code?: string; message?: string };
      cards.update(cardId, { status: "error", error: mapLlmError({ code: err.code ?? "unknown", message: err.message }) });
      return;
    }

    const provisional = !!cards.get(cardId)?.provisional;
    let decided = !provisional;

    const outcome = await runStream(transport, requestId, req, {
      onThinking: () => {
        if (this.dropped.has(cardId)) return;
        const c = cards.get(cardId);
        if (c && !c.content) cards.update(cardId, { status: "thinking" });
      },
      onText: (_delta, full) => {
        if (this.dropped.has(cardId)) return;
        if (!decided) {
          if (!isDecided(full)) return;
          if (isNullAnswer(full)) {
            // Not a question for the user: drop the card and stop paying for tokens.
            this.dropped.add(cardId);
            cards.remove(cardId);
            void transport.cancel(requestId).catch(() => {});
            return;
          }
          decided = true;
          // The user can see this answer now: its question's pieces are used up.
          this.consumedSeq = Math.max(this.consumedSeq, this.cardSeq.get(cardId) ?? 0);
        }
        cards.update(cardId, { content: full, status: "streaming", provisional: false });
      },
    });

    if (this.inflight.get(cardId) === requestId) this.inflight.delete(cardId);
    if (this.autoCardId === cardId) this.autoCardId = null;
    if (this.dropped.has(cardId)) return;
    const card = cards.get(cardId);
    if (!card) return;

    if (outcome.kind === "cancelled") {
      if (card.content.trim()) cards.update(cardId, { status: "stopped" });
      else cards.remove(cardId);
      return;
    }

    if (outcome.kind === "error") {
      if (isCancellation(outcome.code)) {
        cards.remove(cardId);
        return;
      }
      // Even an auto-answer that never showed text gets an error card: the user
      // needs to know answers aren't coming.
      cards.update(cardId, {
        status: "error",
        provisional: false,
        error: mapLlmError({ code: outcome.code, message: outcome.message, retryable: outcome.retryable }),
      });
      return;
    }

    // done
    const text = outcome.text;
    if (provisional && (!text.trim() || isNullAnswer(text))) {
      cards.remove(cardId);
      return;
    }
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
