import { ArrowDown, KeyRound } from "lucide-react";
import { Kbd } from "@/components/common";
import { MODE_META } from "@/core/defaults";
import { useCards } from "@/core/live/cards";
import { useLive } from "@/core/live/session";
import { useKeys, useSettings, useShortcutHealth } from "@/core/settings";
import { ModeIcon } from "../shared/modes";
import { useStickToBottom } from "../shared/useStickToBottom";
import { AnswerCard } from "./AnswerCard";
import { openDashboard } from "./controller";

export function AnswerStack() {
  const cards = useCards((s) => s.cards);
  const last = cards[cards.length - 1];
  const version = `${cards.length}:${last?.id}:${last?.content.length}:${last?.status}`;
  const { ref, pinned, scrollToBottom } = useStickToBottom<HTMLDivElement>(version);

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={ref} className="h-full overflow-y-auto px-2.5 py-2.5 [overflow-anchor:none]" aria-label="Answers" role="log">
        {cards.length === 0 ? (
          <EmptyState />
        ) : (
          <div className="flex flex-col gap-2">
            {cards.map((c) => (
              <AnswerCard key={c.id} card={c} />
            ))}
          </div>
        )}
      </div>
      {!pinned && cards.length > 0 ? (
        <button
          type="button"
          onClick={scrollToBottom}
          className="absolute bottom-2 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-full border border-white/10 bg-popover px-3 py-1 text-[12.5px] text-muted-foreground shadow-lg hover:text-foreground"
        >
          <ArrowDown className="size-3.5" /> Latest answer
        </button>
      ) : null}
    </div>
  );
}

function EmptyState() {
  const mode = useSettings((s) => s.settings.mode);
  const sc = useSettings((s) => s.settings.shortcuts);
  const failures = useShortcutHealth((s) => s.failures);
  const keys = useKeys((s) => s.status);
  const status = useLive((s) => s.status);
  const briefs = useLive((s) => s.briefs);
  const override = useLive((s) => s.briefOverride);
  const brief = override === null ? null : override ? briefs.find((b) => b.id === override) : briefs.find((b) => b.mode === mode);

  if (keys && !keys.anthropic.present) {
    return (
      <div className="flex h-full flex-col items-center justify-center px-6 text-center">
        <KeyRound className="mb-3 size-6 text-muted-foreground" />
        <p className="text-[15px] font-semibold text-foreground">Add your Claude key to start</p>
        <p className="mt-1 max-w-[300px] text-[13.5px] text-muted-foreground">
          Setup takes about a minute: a Claude key, an optional Deepgram key for listening, and a quick audio check.
        </p>
        <button
          type="button"
          onClick={() => openDashboard("/dashboard/onboarding")}
          className="mt-4 rounded-lg bg-primary px-4 py-1.5 text-[13.5px] font-medium text-primary-foreground hover:opacity-90"
        >
          Set up Pluely X
        </button>
      </div>
    );
  }

  if (status !== "idle") {
    const text =
      mode === "lecture"
        ? "Listening. Key points collect under Notes — ask anything below."
        : MODE_META[mode].autoAnswers
          ? "Listening. When someone asks you something, the answer appears here."
          : "Listening.";
    return (
      <div className="flex h-full flex-col items-center justify-center px-8 text-center">
        <p className="text-[14px] text-muted-foreground">{text}</p>
        <p className="mt-2 flex items-center gap-1.5 text-[13px] text-subtle">
          Stuck? <span className="text-muted-foreground">What should I say?</span> <Kbd keys={sc.ask_now.key} />
        </p>
      </div>
    );
  }

  const listenKey = sc.toggle_listen.key;
  return (
    <div className="flex h-full flex-col justify-center px-4">
      <div className="mb-3 flex items-center gap-2 text-muted-foreground">
        <ModeIcon mode={mode} className="size-[18px]" />
        <span className="text-[13px] font-medium">{MODE_META[mode].label} mode</span>
      </div>
      <p className="text-[15px] font-semibold leading-snug text-foreground">
        {MODE_META[mode].listens ? (
          <>
            Press <Kbd keys={listenKey} className="mx-0.5" /> to start listening or ask anything below.
          </>
        ) : (
          <>Ask anything below, or capture your screen.</>
        )}
      </p>
      <p className="mt-1.5 text-[13.5px] leading-snug text-muted-foreground">{MODE_META[mode].blurb}.</p>
      <dl className="mt-4 grid grid-cols-[1fr_auto] gap-x-4 gap-y-1.5 text-[13px]">
        {(
          [
            ["What should I say?", "ask_now"],
            ["Ask about screen", "screenshot_full"],
            ["Ask about a region", "screenshot_region"],
            ["Type a question", "focus_input"],
            ["Show / hide overlay", "toggle_overlay"],
          ] as const
        ).map(([label, action]) => {
          const taken = failures.some((f) => f.action === action);
          return (
            <div key={label} className="contents">
              <dt className="text-muted-foreground">{label}</dt>
              <dd className="flex items-center gap-1.5">
                {taken ? <span className="text-[12px] text-warn">taken</span> : null}
                <Kbd keys={sc[action].key} className={taken ? "opacity-50" : undefined} />
              </dd>
            </div>
          );
        })}
      </dl>
      {MODE_META[mode].listens ? (
        <p className="mt-4 text-[12.5px] text-subtle">
          {brief ? (
            <>
              Brief: <span className="text-muted-foreground">{brief.name}</span>
            </>
          ) : (
            <>No brief — add a résumé, job description or agenda in Settings for sharper answers.</>
          )}
        </p>
      ) : null}
    </div>
  );
}
