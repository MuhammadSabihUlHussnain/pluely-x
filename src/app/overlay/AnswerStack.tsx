import { ArrowDown, KeyRound } from "lucide-react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { parseAnswer, talkingPoints } from "@/core/ai/answer";
import { Kbd } from "@/components/common";
import { MODE_META } from "@/core/defaults";
import { useCards, type Card } from "@/core/live/cards";
import { NO_PROGRESS, speechProgress, type SpeechProgress } from "@/core/live/follow";
import { useLive } from "@/core/live/session";
import { segments } from "@/core/live/transcript";
import { useKeys, useSettings, useShortcutHealth } from "@/core/settings";
import { ModeIcon } from "../shared/modes";
import { AnswerCard } from "./AnswerCard";
import { fitOverlayHeight, openDashboard, setMode } from "./controller";
import { useOverlay } from "./store";

export function AnswerStack() {
  const cards = useCards((s) => s.cards);
  const last = cards[cards.length - 1];
  const progress = useSpeechProgress(last);
  const version = `${cards.length}:${last?.id}:${last?.content.length}:${last?.status}:${progress.line}`;
  const { ref, following, follow } = useFollowNewest(last?.id, version, progress);
  useAutoFit(ref, last, version);

  return (
    <div className="relative min-h-0 flex-1">
      <div ref={ref} className="relative h-full overflow-y-auto px-2.5 py-2.5 [overflow-anchor:none]" aria-label="Answers" role="log">
        {cards.length === 0 ? (
          <EmptyState />
        ) : (
          <div className="flex flex-col gap-2">
            {cards.map((c) => (
              <div key={c.id} data-card-id={c.id}>
                <AnswerCard card={c} progress={c.id === last?.id ? progress : undefined} />
              </div>
            ))}
          </div>
        )}
      </div>
      {!following && cards.length > 0 ? (
        <button
          type="button"
          onClick={follow}
          className="absolute bottom-2 left-1/2 flex -translate-x-1/2 items-center gap-1 rounded-full border border-white/10 bg-popover px-3 py-1 text-[12.5px] text-muted-foreground shadow-lg hover:text-foreground"
        >
          <ArrowDown className="size-3.5" /> Latest answer
        </button>
      ) : null}
    </div>
  );
}

const USER_INTENT_MS = 600;
const EDGE_PAD = 10;

/**
 * Follows the newest answer for reading aloud: shows as much of it as fits but never scrolls
 * past its first line, so the opening sentence stays in view while the rest streams in.
 * A new answer jumps into view; scrolling by hand pauses following until the next answer
 * (or "Latest answer").
 */
/**
 * While listening, follows the user's own voice through the newest answer (what the mic
 * hears since the answer appeared, matched against its lines).
 */
function useSpeechProgress(card: Card | undefined): SpeechProgress {
  const status = useLive((s) => s.status);
  const startedAtMs = useLive((s) => s.startedAtMs);
  const transcript = useLive((s) => s.transcript);
  const id = card?.id;
  const content = card?.content ?? "";
  const kind = card?.kind;
  const createdAt = card?.createdAt ?? 0;
  const lines = useMemo(() => {
    if (!content || kind === "summary") return null;
    const p = parseAnswer(content);
    return p.headline ? [p.headline, ...(talkingPoints(p.body) ?? [])] : null;
  }, [content, kind]);
  return useMemo(() => {
    if (!id || !lines || status !== "live" || !startedAtMs) return NO_PROGRESS;
    // Speech since just before the answer appeared, on the session clock.
    const from = createdAt - startedAtMs - 1500;
    const spoken = segments(transcript)
      .filter((s) => s.source === "me" && s.t1Ms >= from)
      .map((s) => s.text)
      .join(" ");
    return spoken ? speechProgress(lines, spoken) : NO_PROGRESS;
  }, [id, lines, status, startedAtMs, createdAt, transcript]);
}

/** Top of `el` in the scroll container's content coordinates. */
function topIn(scroller: HTMLElement, el: HTMLElement): number {
  return el.getBoundingClientRect().top - scroller.getBoundingClientRect().top + scroller.scrollTop;
}

/**
 * Grows the overlay so the newest answer fits without scrolling (up to the screen height),
 * and returns to the user's own size once it isn't needed.
 */
function useAutoFit(ref: React.RefObject<HTMLDivElement | null>, last: Card | undefined, version: string) {
  const collapsed = useOverlay((s) => s.collapsed);
  useEffect(() => {
    if (collapsed) return;
    const t = setTimeout(() => {
      const el = ref.current;
      const winH = useOverlay.getState().win?.bounds.height;
      if (!el || !winH) return;
      if (!last) {
        fitOverlayHeight(0, true);
        return;
      }
      const card = el.querySelector<HTMLElement>(`[data-card-id="${CSS.escape(last.id)}"]`);
      if (!card) return;
      const needed = winH - el.clientHeight + card.getBoundingClientRect().height + 2 * EDGE_PAD;
      const settled = last.status !== "streaming" && last.status !== "thinking" && last.status !== "pending";
      fitOverlayHeight(needed, settled);
    }, 120);
    return () => clearTimeout(t);
  }, [ref, last, version, collapsed]);
}

function useFollowNewest(lastId: string | undefined, version: string, progress: SpeechProgress) {
  const ref = useRef<HTMLDivElement | null>(null);
  const [following, setFollowing] = useState(true);
  const followRef = useRef(true);
  const lastIntent = useRef(0);
  const progressRef = useRef(progress);
  progressRef.current = progress;

  const setFollow = useCallback((v: boolean) => {
    if (followRef.current === v) return;
    followRef.current = v;
    setFollowing(v);
  }, []);

  const align = useCallback(() => {
    const el = ref.current;
    if (!el || !followRef.current || !lastId) return;
    const card = el.querySelector<HTMLElement>(`[data-card-id="${CSS.escape(lastId)}"]`);
    if (!card) return;
    const cardTop = topIn(el, card);
    const top = cardTop - EDGE_PAD;
    const bottom = cardTop + card.offsetHeight + EDGE_PAD;
    let target = Math.max(0, Math.min(top, bottom - el.clientHeight));
    // Reading aloud: keep the line being said about a quarter of the way down.
    const prog = progressRef.current;
    const current = prog.active ? card.querySelector<HTMLElement>("[data-say-current]") : null;
    if (prog.active && prog.spoken >= prog.total) {
      // Finished reading: stay at the end of the answer (follow-up chips), don't jump back up.
      target = Math.max(0, Math.max(top, bottom - el.clientHeight));
    } else if (current) {
      const want = topIn(el, current) - el.clientHeight * 0.25;
      target = Math.max(top, Math.min(want, bottom - el.clientHeight));
      target = Math.max(0, Math.max(target, Math.min(top, bottom - el.clientHeight)));
    }
    if (Math.abs(el.scrollTop - target) > 1) el.scrollTop = target;
  }, [lastId]);

  // A new answer always comes into view.
  useEffect(() => {
    if (!lastId) return;
    followRef.current = true;
    setFollowing(true);
  }, [lastId]);

  useEffect(() => {
    align();
  }, [version, align]);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const intent = () => (lastIntent.current = performance.now());
    const onKey = (e: KeyboardEvent) => {
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) intent();
    };
    const onScroll = () => {
      if (performance.now() - lastIntent.current < USER_INTENT_MS) setFollow(false);
      else align(); // layout moved; keep the newest answer's start in view
    };
    el.addEventListener("wheel", intent, { passive: true });
    el.addEventListener("touchmove", intent, { passive: true });
    el.addEventListener("pointerdown", intent, { passive: true });
    el.addEventListener("keydown", onKey);
    el.addEventListener("scroll", onScroll, { passive: true });
    // Content that grows after render (Streamdown, shiki, images) re-aligns.
    const ro = new ResizeObserver(() => align());
    ro.observe(el);
    const mo = new MutationObserver(() => {
      for (const child of Array.from(el.children)) ro.observe(child);
      align();
    });
    mo.observe(el, { childList: true, subtree: true, characterData: true });
    for (const child of Array.from(el.children)) ro.observe(child);
    return () => {
      el.removeEventListener("wheel", intent);
      el.removeEventListener("touchmove", intent);
      el.removeEventListener("pointerdown", intent);
      el.removeEventListener("keydown", onKey);
      el.removeEventListener("scroll", onScroll);
      ro.disconnect();
      mo.disconnect();
    };
  }, [align, setFollow]);

  const follow = useCallback(() => {
    followRef.current = true;
    setFollowing(true);
    requestAnimationFrame(() => align());
  }, [align]);

  return { ref, following, follow };
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
    const auto = MODE_META[mode].autoAnswers;
    const text =
      mode === "lecture"
        ? "Listening. Key points collect under Notes. Questions you hear show up here with an Answer button."
        : auto
          ? "Listening. When someone asks you something, the answer appears here."
          : `${MODE_META[mode].label} mode doesn't answer on its own. Questions you hear show up here with an Answer button.`;
    return (
      <div className="flex h-full flex-col items-center justify-center px-8 text-center">
        <p className="text-[14px] text-muted-foreground">{text}</p>
        {!auto && mode !== "lecture" ? (
          <div className="mt-3 flex gap-2">
            {(["interview", "meeting"] as const).map((m) => (
              <button
                key={m}
                type="button"
                onClick={() => void setMode(m)}
                className="rounded-lg border border-white/12 px-3 py-1.5 text-[13px] font-medium text-foreground hover:bg-foreground/[0.08]"
              >
                Switch to {MODE_META[m].label}
              </button>
            ))}
          </div>
        ) : null}
        <p className="mt-3 flex items-center gap-1.5 text-[13px] text-subtle">
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
