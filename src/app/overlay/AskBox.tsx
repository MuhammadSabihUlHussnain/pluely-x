import { ArrowUp, CornerDownRight, Crop, Loader2, MessageSquareQuote, Monitor, X } from "lucide-react";
import { useEffect, useLayoutEffect, useRef } from "react";
import { Hint, Kbd } from "@/components/common";
import { parseAnswer } from "@/core/ai/answer";
import { useCards } from "@/core/live/cards";
import { pipeline, useLive } from "@/core/live/session";
import { useSettings, useShortcutHealth } from "@/core/settings";
import { cn } from "@/lib/utils";
import { captureFull, captureRegion, endTyping, focusAskBox, submitAsk } from "./controller";
import { useOverlay } from "./store";

export function AskBox() {
  const draft = useOverlay((s) => s.draft);
  const attachment = useOverlay((s) => s.attachment);
  const replyToId = useOverlay((s) => s.replyTo);
  const focusSignal = useOverlay((s) => s.focusSignal);
  const typing = useOverlay((s) => s.typing);
  const capturing = useOverlay((s) => s.capturing);
  const sc = useSettings((s) => s.settings.shortcuts);
  const mode = useSettings((s) => s.settings.mode);
  const focusKeyTaken = useShortcutHealth((s) => s.failures.some((f) => f.action === "focus_input"));
  const liveStatus = useLive((s) => s.status);
  const hasTranscript = useLive((s) => s.transcript.order.length > 0);
  const replyCard = useCards((s) => (replyToId ? s.cards.find((c) => c.id === replyToId) : undefined));
  const ref = useRef<HTMLTextAreaElement>(null);

  // Focus requests from shortcuts / screenshot flows.
  useEffect(() => {
    if (focusSignal > 0) ref.current?.focus();
  }, [focusSignal]);

  // Auto-size between 1 and 5 lines.
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, 5 * 20 + 16)}px`;
  }, [draft]);

  const canSend = draft.trim().length > 0 || !!attachment;
  const showAskNow = liveStatus !== "idle" || hasTranscript;
  const placeholder = attachment
    ? "Add a question about this screenshot, or press Enter"
    : replyCard
      ? "Ask a follow-up…"
      : mode === "coding"
        ? "Paste a problem or ask anything…"
        : "Ask anything…";

  return (
    <footer className="shrink-0 border-t border-white/[0.07] p-2.5">
      {(attachment || replyCard) && (
        <div className="mb-2 flex flex-wrap items-center gap-1.5">
          {attachment ? (
            <span className="flex items-center gap-2 rounded-lg border border-white/10 bg-foreground/[0.05] py-1 pl-1 pr-1.5">
              <img src={attachment.thumbDataUrl} alt="Attached screenshot" className="h-8 w-auto rounded" />
              <span className="text-[12.5px] text-muted-foreground">Screenshot attached</span>
              <button
                type="button"
                aria-label="Remove screenshot"
                onClick={() => useOverlay.setState({ attachment: null })}
                className="rounded p-0.5 text-subtle hover:bg-foreground/10 hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            </span>
          ) : null}
          {replyCard ? (
            <span className="flex min-w-0 max-w-full items-center gap-1.5 rounded-lg border border-white/10 bg-foreground/[0.05] py-1 pl-2 pr-1.5">
              <CornerDownRight className="size-3.5 shrink-0 text-subtle" />
              <span className="truncate text-[12.5px] text-muted-foreground">
                Follow-up to: {(parseAnswer(replyCard.content).headline || replyCard.question).replace(/[`*]/g, "")}
              </span>
              <button
                type="button"
                aria-label="Cancel follow-up"
                onClick={() => useOverlay.setState({ replyTo: null })}
                className="rounded p-0.5 text-subtle hover:bg-foreground/10 hover:text-foreground"
              >
                <X className="size-3.5" />
              </button>
            </span>
          ) : null}
        </div>
      )}

      <div
        className={cn(
          "rounded-xl border bg-foreground/[0.045] transition-colors",
          typing ? "border-ring/60 bg-foreground/[0.07]" : "border-white/10 hover:border-white/[0.16]",
        )}
        onPointerDown={(e) => {
          // A non-activating overlay can't take keyboard focus until typing mode starts.
          if (!typing && e.target === ref.current) {
            e.preventDefault();
            void focusAskBox();
          }
        }}
      >
        <label htmlFor="ask-input" className="sr-only">
          Ask Claude
        </label>
        <textarea
          id="ask-input"
          ref={ref}
          rows={1}
          value={draft}
          placeholder={placeholder}
          onChange={(e) => useOverlay.setState({ draft: e.target.value })}
          onFocus={() => {
            if (!useOverlay.getState().typing) void focusAskBox();
          }}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.shiftKey && !e.nativeEvent.isComposing) {
              e.preventDefault();
              if (submitAsk()) ref.current?.blur();
            } else if (e.key === "Escape") {
              e.preventDefault();
              ref.current?.blur();
              void endTyping(true);
            }
          }}
          className="block max-h-[116px] w-full resize-none bg-transparent px-3 pb-1 pt-2.5 text-[14px] leading-5 text-foreground outline-none focus-visible:outline-none"
        />
        <div className="flex items-center gap-1 px-1.5 pb-1.5">
          <Hint label="Ask about screen — captures the monitor under the cursor" keys={sc.screenshot_full.key} side="top">
            <button
              type="button"
              onClick={() => void captureFull()}
              disabled={!!capturing}
              className="flex h-7 items-center gap-1.5 whitespace-nowrap rounded-lg px-2 text-[12.5px] font-medium text-muted-foreground transition-colors hover:bg-foreground/[0.08] hover:text-foreground disabled:opacity-50"
            >
              {capturing === "full" ? <Loader2 className="size-3.5 animate-spin" /> : <Monitor className="size-3.5" />}
              Screen
            </button>
          </Hint>
          <Hint label="Ask about a region — drag a box on screen" keys={sc.screenshot_region.key} side="top">
            <button
              type="button"
              onClick={() => void captureRegion()}
              disabled={!!capturing}
              className="flex h-7 items-center gap-1.5 whitespace-nowrap rounded-lg px-2 text-[12.5px] font-medium text-muted-foreground transition-colors hover:bg-foreground/[0.08] hover:text-foreground disabled:opacity-50"
            >
              {capturing === "region" ? <Loader2 className="size-3.5 animate-spin" /> : <Crop className="size-3.5" />}
              Region
            </button>
          </Hint>
          {showAskNow ? (
            <Hint label="Answer from the last two minutes of the conversation" keys={sc.ask_now.key} side="top">
              <button
                type="button"
                onClick={() => pipeline.askNow()}
                className="flex h-7 items-center gap-1.5 whitespace-nowrap rounded-lg px-2 text-[12.5px] font-medium text-muted-foreground transition-colors hover:bg-foreground/[0.08] hover:text-foreground"
              >
                <MessageSquareQuote className="size-3.5" />
                What should I say?
              </button>
            </Hint>
          ) : null}
          <span className="flex-1" />
          {!typing && !draft && !showAskNow && !focusKeyTaken ? (
            <span className="mr-1 hidden text-[12px] text-subtle min-[400px]:inline-flex">
              <Kbd keys={sc.focus_input.key} />
            </span>
          ) : null}
          <Hint label="Send (Enter) · new line (Shift+Enter)" side="top">
            <button
              type="button"
              aria-label="Send"
              disabled={!canSend}
              onClick={() => submitAsk()}
              className="flex size-7 items-center justify-center rounded-lg bg-primary text-primary-foreground transition-opacity hover:opacity-90 disabled:bg-foreground/10 disabled:text-subtle"
            >
              <ArrowUp className="size-4" />
            </button>
          </Hint>
        </div>
      </div>
    </footer>
  );
}
