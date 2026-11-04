import { AlertCircle, ChevronRight, CornerDownRight, Lightbulb, ShieldAlert, Square, X } from "lucide-react";
import { memo, useMemo, useState } from "react";
import { CopyButton, IconButton, InlineMd } from "@/components/common";
import { Markdown } from "@/components/Markdown";
import { parseAnswer, speakableText, talkingPoints, type ParsedAnswer } from "@/core/ai/answer";
import type { FollowupChip } from "@/core/ai/prompts";
import { summaryToMarkdown } from "@/core/db/markdown";
import { cardSink, type Card } from "@/core/live/cards";
import { pipeline, retrySummary } from "@/core/live/session";
import { cn } from "@/lib/utils";
import { MODE_META } from "@/core/defaults";
import { openDashboard, replyTo, setMode } from "./controller";

const CHIP_LABEL: Record<FollowupChip, string> = {
  shorter: "Shorter",
  more: "More detail",
  example: "Example",
  continue: "Continued",
};

function kindLabel(c: Card): { text: string; tone: "them" | "you" | "muted" } {
  switch (c.kind) {
    case "auto":
      return { text: "Them", tone: "them" };
    case "ask_now":
      return { text: "What should I say?", tone: "muted" };
    case "typed":
      return { text: "You asked", tone: "you" };
    case "screen":
      return { text: c.question ? "Screen + question" : "Screen", tone: "you" };
    case "followup":
      return { text: c.chip ? CHIP_LABEL[c.chip] : "Follow-up", tone: "muted" };
    case "summary":
      return { text: "Session summary", tone: "muted" };
    case "heard":
      return c.heardReason === "not_question"
        ? { text: "Them · not answered", tone: "muted" }
        : { text: "Them · question heard", tone: "them" };
  }
}

export const AnswerCard = memo(function AnswerCard({ card }: { card: Card }) {
  const label = kindLabel(card);
  const busy = card.status === "pending" || card.status === "thinking" || card.status === "streaming";
  const isError = card.status === "error";

  return (
    <article
      className={cn(
        "card-in rounded-xl border p-3",
        isError ? "border-danger/35 bg-danger/[0.09]" : "glass-raised",
        card.kind === "heard" && card.heardReason === "not_question" && "opacity-75 transition-opacity hover:opacity-100",
      )}
      aria-busy={busy}
    >
      <header className="mb-1.5 flex items-start gap-2">
        {card.screenshot?.thumbDataUrl ? (
          <img
            src={card.screenshot.thumbDataUrl}
            alt="Screenshot sent with this question"
            className="h-12 w-auto max-w-[96px] shrink-0 rounded-md border border-white/10 object-cover"
          />
        ) : null}
        <div className="min-w-0 flex-1">
          <span
            className={cn(
              "text-[12px] font-semibold",
              label.tone === "them" ? "text-them" : label.tone === "you" ? "text-you" : "text-subtle",
            )}
          >
            {label.text}
          </span>
          {card.question && card.kind !== "ask_now" && card.kind !== "summary" ? (
            <p className="line-clamp-2 text-[13px] leading-[18px] text-muted-foreground">{card.question}</p>
          ) : null}
        </div>
        {busy && card.kind === "summary" ? null : busy ? (
          <IconButton size="sm" label="Stop this answer" onClick={() => pipeline.stop(card.id)}>
            <Square />
          </IconButton>
        ) : (
          <IconButton size="sm" label="Dismiss" onClick={() => cardSink.remove(card.id)}>
            <X />
          </IconButton>
        )}
      </header>

      {isError ? (
        <ErrorBody card={card} />
      ) : card.kind === "heard" ? (
        <HeardBody card={card} />
      ) : card.kind === "summary" ? (
        <SummaryBody card={card} />
      ) : (
        <AnswerBody card={card} />
      )}
    </article>
  );
});

/** A question that wasn't answered on its own: the mode doesn't auto-answer, or Claude skipped it. */
function HeardBody({ card }: { card: Card }) {
  const label = MODE_META[card.mode]?.label ?? "This";
  if (card.heardReason === "not_question") {
    return (
      <div className="mt-1 flex flex-wrap items-center gap-2">
        <button
          type="button"
          onClick={() => pipeline.answerHeard(card.id)}
          className="rounded-lg bg-primary px-3 py-1.5 text-[13.5px] font-semibold text-primary-foreground hover:opacity-90"
        >
          Answer it
        </button>
        <span className="text-[12.5px] text-muted-foreground">Skipped: didn't look like a question for you.</span>
      </div>
    );
  }
  return (
    <div className="mt-1 flex flex-wrap items-center gap-2">
      <button
        type="button"
        onClick={() => pipeline.answerHeard(card.id)}
        className="rounded-lg bg-primary px-3 py-1.5 text-[13.5px] font-semibold text-primary-foreground hover:opacity-90"
      >
        Answer it
      </button>
      <span className="text-[12.5px] text-muted-foreground">
        {label} mode doesn't answer on its own.{" "}
        <button
          type="button"
          onClick={() => void setMode("interview")}
          className="font-medium text-foreground underline underline-offset-2 hover:opacity-80"
        >
          Switch to Interview
        </button>
      </span>
    </div>
  );
}

function Thinking() {
  return (
    <p className="thinking-text py-1 text-[14px] font-medium" role="status">
      Thinking…
    </p>
  );
}

function AnswerBody({ card }: { card: Card }) {
  const parsed = useMemo(() => parseAnswer(card.content), [card.content]);
  const streaming = card.status === "streaming";
  const detailsDefaultOpen = card.mode === "coding";
  const [detailsOpen, setDetailsOpen] = useState<boolean | null>(null);
  const showDetails = detailsOpen ?? detailsDefaultOpen;

  if (!card.content) return <Thinking />;

  return (
    <div data-answer>
      <SpokenAnswer parsed={parsed} streaming={streaming} />
      {parsed.details ? (
        <div className="mt-2">
          <button
            type="button"
            aria-expanded={showDetails}
            onClick={() => setDetailsOpen(!showDetails)}
            className="flex items-center gap-1 rounded-md py-0.5 pr-1.5 text-[13px] font-medium text-muted-foreground hover:text-foreground"
          >
            <ChevronRight className={cn("size-4 transition-transform duration-150", showDetails && "rotate-90")} />
            Details
          </button>
          {showDetails ? (
            <Markdown isStreaming={streaming} className="mt-1">
              {parsed.details}
            </Markdown>
          ) : null}
        </div>
      ) : null}

      {card.stopNote === "refusal" ? (
        <p className="mt-2 flex items-center gap-1.5 text-[13px] text-muted-foreground">
          <ShieldAlert className="size-4" /> Claude declined this one.
        </p>
      ) : null}
      {card.stopNote === "max_tokens" ? (
        <p className="mt-2 flex items-center gap-1.5 text-[13px] text-warn">
          Answer cut off —
          <button
            type="button"
            className="font-medium underline underline-offset-2 hover:opacity-80"
            onClick={() => pipeline.followup(card.id, "continue")}
          >
            More
          </button>
        </p>
      ) : null}
      {card.status === "stopped" ? <p className="mt-2 text-[12.5px] text-subtle">Stopped</p> : null}

      {card.status === "done" || card.status === "stopped" ? <Chips card={card} /> : null}
    </div>
  );
}

function Chips({ card }: { card: Card }) {
  const chip = (c: FollowupChip, label: string, hint: string) => (
    <button
      type="button"
      title={hint}
      onClick={() => pipeline.followup(card.id, c)}
      className="rounded-full border border-white/10 px-2.5 py-[3px] text-[12.5px] text-muted-foreground transition-colors hover:border-white/20 hover:bg-foreground/[0.07] hover:text-foreground"
    >
      {label}
    </button>
  );
  return (
    <div className="mt-2.5 flex flex-wrap items-center gap-1.5">
      {chip("shorter", "Shorter", "Rewrite this answer in one sentence and two bullets")}
      {chip("more", "More detail", "Go deeper, with more depth under Details")}
      {chip("example", "Example", "Give a concrete example")}
      <button
        type="button"
        title="Ask a follow-up about this answer"
        onClick={() => replyTo(card.id)}
        className="inline-flex items-center gap-1 rounded-full border border-white/10 px-2.5 py-[3px] text-[12.5px] text-muted-foreground transition-colors hover:border-white/20 hover:bg-foreground/[0.07] hover:text-foreground"
      >
        <CornerDownRight className="size-3.5" />
        Follow-up
      </button>
      <span className="flex-1" />
      <CopyButton text={copyText(card.content)} />
    </div>
  );
}

/** Copy what the user would say (no Markdown, no private note); fall back to the raw answer. */
function copyText(content: string): string {
  const p = parseAnswer(content);
  return p.headline ? speakableText(p) : content;
}

/**
 * The part of an answer meant to be read out: a large opening line, then the talking points
 * as roomy, high-contrast lines (not muted prose), then the private note set apart so it is
 * never read out by accident.
 */
export function SpokenAnswer({ parsed, streaming }: { parsed: ParsedAnswer; streaming: boolean }) {
  const points = useMemo(() => talkingPoints(parsed.body), [parsed.body]);
  return (
    <>
      {parsed.headline ? (
        <p
          className="text-[17px] font-semibold leading-[1.45] tracking-[-0.005em] text-foreground"
        >
          <InlineMd text={parsed.headline} />
        </p>
      ) : null}
      {points ? (
        <ul className="mt-2 space-y-1.5">
          {points.map((pt, i) => (
            <li
              key={i}
              className="flex gap-2.5 text-[15px] leading-[1.5] text-foreground/90"
            >
              <span aria-hidden className="mt-[0.6em] size-1.5 shrink-0 rounded-full bg-foreground/35" />
              <span className="min-w-0">
                <InlineMd text={pt} />
              </span>
            </li>
          ))}
        </ul>
      ) : parsed.body ? (
        <Markdown isStreaming={streaming && !parsed.details} className="mt-1.5">
          {parsed.body}
        </Markdown>
      ) : null}
      {parsed.note ? (
        <p className="mt-2.5 flex gap-1.5 text-[12.5px] leading-snug text-subtle">
          <Lightbulb className="mt-px size-3.5 shrink-0" aria-hidden />
          <span>
            <span className="font-medium">Note to you, not to say: </span>
            {parsed.note}
          </span>
        </p>
      ) : null}
    </>
  );
}

function ErrorBody({ card }: { card: Card }) {
  const e = card.error;
  if (!e) return null;
  return (
    <div className="flex gap-2.5" role="alert">
      <AlertCircle className="mt-0.5 size-[18px] shrink-0 text-danger" />
      <div className="min-w-0 flex-1">
        <p className="text-[14px] font-semibold text-foreground">{e.title}</p>
        <p className="mt-0.5 text-[13px] leading-snug text-muted-foreground">{e.message}</p>
        <div className="mt-2 flex gap-1.5">
          {e.action === "settings" ? (
            <button
              type="button"
              onClick={() => openDashboard("/dashboard/settings#keys")}
              className="rounded-lg bg-primary px-3 py-1 text-[13px] font-medium text-primary-foreground hover:opacity-90"
            >
              Open settings
            </button>
          ) : null}
          {e.retryable ? (
            <button
              type="button"
              onClick={() => (card.kind === "summary" ? void retrySummary(card.id) : pipeline.retry(card.id))}
              className="rounded-lg bg-primary px-3 py-1 text-[13px] font-medium text-primary-foreground hover:opacity-90"
            >
              Retry
            </button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

function summaryLead(sum: NonNullable<Card["summary"]>): string {
  switch (sum.mode) {
    case "meeting":
      return sum.summary;
    case "interview":
      return sum.overview;
    case "lecture":
      return sum.study_notes
        .replace(/^#+\s.*$/gm, "")
        .replace(/[*_`]/g, "")
        .split("\n")
        .map((l) => l.replace(/^\s*-\s*/, ""))
        .filter(Boolean)
        .slice(0, 3)
        .join(" · ");
    case "coding":
      return sum.problems.map((p) => p.problem).join(", ");
    case "ask":
      return sum.summary;
  }
}

function SummaryBody({ card }: { card: Card }) {
  const md = useMemo(() => (card.summary ? summaryToMarkdown(card.summary) : ""), [card.summary]);
  const [open, setOpen] = useState(false);
  if (!card.summary) {
    return (
      <p className="thinking-text py-1 text-[14px] font-medium" role="status">
        Writing the session summary…
      </p>
    );
  }
  return (
    <div>
      <p className="text-[16px] font-semibold leading-[1.4] text-foreground">{card.summary.title}</p>
      <p className="mt-1 text-[14px] leading-[1.55] text-foreground/90">{summaryLead(card.summary)}</p>
      <button
        type="button"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
        className="mt-2 flex items-center gap-1 rounded-md py-0.5 pr-1.5 text-[13px] font-medium text-muted-foreground hover:text-foreground"
      >
        <ChevronRight className={cn("size-4 transition-transform duration-150", open && "rotate-90")} />
        Full summary
      </button>
      {open ? <Markdown className="mt-1">{md}</Markdown> : null}
      <div className="mt-2.5 flex items-center gap-1.5">
        <button
          type="button"
          onClick={() => openDashboard("/dashboard/sessions")}
          className="rounded-full border border-white/10 px-2.5 py-[3px] text-[12.5px] text-muted-foreground hover:bg-foreground/[0.07] hover:text-foreground"
        >
          Open in library
        </button>
        <span className="flex-1" />
        <CopyButton text={md} label="Copy summary" />
      </div>
    </div>
  );
}
