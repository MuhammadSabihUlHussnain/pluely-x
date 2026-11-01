import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

const USER_INTENT_MS = 600;

/**
 * Keeps a scroll container pinned to the bottom while content grows (new
 * text, async markdown/code rendering, images), and pauses only when the
 * *user* scrolls up — layout-driven scroll events never unpin it.
 * `version` should change whenever new content arrives.
 */
export function useStickToBottom<T extends HTMLElement>(version: unknown) {
  const ref = useRef<T | null>(null);
  const [pinned, setPinned] = useState(true);
  const pinnedRef = useRef(true);
  const lastIntent = useRef(0);

  const setPin = useCallback((v: boolean) => {
    if (pinnedRef.current === v) return;
    pinnedRef.current = v;
    setPinned(v);
  }, []);

  const stick = useCallback(() => {
    const el = ref.current;
    if (el && pinnedRef.current) el.scrollTop = el.scrollHeight;
  }, []);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const intent = () => (lastIntent.current = performance.now());
    const onKey = (e: KeyboardEvent) => {
      if (["ArrowUp", "ArrowDown", "PageUp", "PageDown", "Home", "End", " "].includes(e.key)) intent();
    };
    const onScroll = () => {
      const atBottom = el.scrollHeight - el.scrollTop - el.clientHeight < 28;
      if (atBottom) setPin(true);
      else if (performance.now() - lastIntent.current < USER_INTENT_MS) setPin(false);
      else stick(); // layout moved us; stay glued to the latest content
    };
    el.addEventListener("wheel", intent, { passive: true });
    el.addEventListener("touchmove", intent, { passive: true });
    el.addEventListener("pointerdown", intent, { passive: true });
    el.addEventListener("keydown", onKey);
    el.addEventListener("scroll", onScroll, { passive: true });

    // Content that grows after render (Streamdown, shiki, images) re-sticks.
    const ro = new ResizeObserver(() => stick());
    ro.observe(el);
    for (const child of Array.from(el.children)) ro.observe(child);
    const mo = new MutationObserver(() => {
      for (const child of Array.from(el.children)) ro.observe(child);
      stick();
    });
    mo.observe(el, { childList: true, subtree: true, characterData: true });
    return () => {
      el.removeEventListener("wheel", intent);
      el.removeEventListener("touchmove", intent);
      el.removeEventListener("pointerdown", intent);
      el.removeEventListener("keydown", onKey);
      el.removeEventListener("scroll", onScroll);
      ro.disconnect();
      mo.disconnect();
    };
  }, [setPin, stick]);

  useLayoutEffect(() => {
    stick();
  }, [version, stick]);

  const scrollToBottom = useCallback(() => {
    const el = ref.current;
    if (!el) return;
    setPin(true);
    el.scrollTo({ top: el.scrollHeight, behavior: "smooth" });
  }, [setPin]);

  return { ref, pinned, scrollToBottom };
}
