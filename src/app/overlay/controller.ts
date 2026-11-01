// Overlay flows: native events, shortcut actions, screenshots, typing mode,
// and the pill/panel size switch.

import { useCards } from "@/core/live/cards";
import { loadBriefs, onModeChanged, pipeline, reattach, toggleListening, useLive } from "@/core/live/session";
import { native, type Unlisten } from "@/core/native";
import { initSettings, useKeys, useSettings } from "@/core/settings";
import type { Mode, ScreenshotMeta, ShortcutAction } from "@/core/types";
import { showToast, useOverlay } from "./store";

const PILL_SIZE = { width: 248, height: 46 };

let initialised = false;
const handledShots = new Set<string>();

export async function initOverlay(): Promise<() => void> {
  const unlisten: Unlisten[] = [];
  if (initialised) return () => {};
  initialised = true;

  await initSettings();
  void useKeys.getState().refresh();
  void loadBriefs();
  try {
    const win = await native.windowGetState();
    useOverlay.setState({ win, typing: win.typing });
  } catch {
    /* window state arrives with the first event */
  }

  unlisten.push(
    await native.on("window://state", (win) => {
      useOverlay.setState({ win });
      rememberPanelSize(win.bounds.width, win.bounds.height);
    }),
    await native.on("window://typing", ({ active }) => {
      useOverlay.setState({ typing: active });
      if (!active && document.activeElement instanceof HTMLElement) document.activeElement.blur();
    }),
    await native.on("shortcut://action", ({ action }) => void handleShortcut(action)),
    await native.on("screenshot://captured", (meta) => void handleCaptured(meta)),
    await native.on("screenshot://cancelled", () => useOverlay.setState({ capturing: null })),
    await native.on("app://error", (e) => showToast(e.message, "error")),
  );

  // Re-read keys when the dashboard saves one.
  const onFocus = () => void useKeys.getState().refresh();
  window.addEventListener("focus", onFocus);
  unlisten.push(() => window.removeEventListener("focus", onFocus));

  // When listening stops, fold the transcript so the summary has room.
  unlisten.push(
    useLive.subscribe((st, prev) => {
      if (prev.status === "stopping" && st.status === "idle") useOverlay.setState({ transcriptOpen: false });
    }),
  );

  await reattach();
  return () => {
    unlisten.forEach((u) => u());
    initialised = false;
  };
}

export async function handleShortcut(action: ShortcutAction): Promise<void> {
  switch (action) {
    case "toggle_listen":
      await toggleListening();
      return;
    case "ask_now":
      expand();
      if (useLive.getState().status === "idle" && !useLive.getState().transcript.order.length) {
        showToast("Start listening first — “What should I say?” answers from the conversation.");
        return;
      }
      pipeline.askNow();
      return;
    case "screenshot_full":
      await captureFull();
      return;
    case "screenshot_region":
      await captureRegion();
      return;
    case "focus_input":
      expand();
      // Rust already called window_begin_typing for this shortcut.
      useOverlay.setState((s) => ({ typing: true, focusSignal: s.focusSignal + 1 }));
      return;
    default:
      return;
  }
}

// ---------------------------------------------------------------------------
// Screenshots

export async function captureFull(): Promise<void> {
  if (useOverlay.getState().capturing) return;
  useOverlay.setState({ capturing: "full" });
  try {
    const meta = await native.screenshotFull({ target: "cursor" });
    await handleCaptured(meta);
  } catch (e) {
    useOverlay.setState({ capturing: null });
    showToast((e as { message?: string }).message ?? "Couldn't capture the screen", "error");
  }
}

export async function captureRegion(): Promise<void> {
  useOverlay.setState({ capturing: "region" });
  try {
    await native.screenshotRegionStart();
  } catch (e) {
    useOverlay.setState({ capturing: null });
    showToast((e as { message?: string }).message ?? "Couldn't start region capture", "error");
  }
}

/** Coding auto-solves; other modes answer now if a question is typed, else attach and focus. */
export async function handleCaptured(meta: ScreenshotMeta): Promise<void> {
  if (handledShots.has(meta.id)) return;
  handledShots.add(meta.id);
  useOverlay.setState({ capturing: null });
  expand();
  const shot = { id: meta.id, mime: meta.mime, thumbDataUrl: meta.thumbDataUrl };
  const mode = useSettings.getState().settings.mode;
  const { draft, replyTo } = useOverlay.getState();
  if (mode === "coding") {
    pipeline.ask({ question: draft.trim(), screenshot: shot, replyTo });
    if (draft.trim()) clearDraft();
    return;
  }
  if (draft.trim()) {
    pipeline.ask({ question: draft.trim(), screenshot: shot, replyTo });
    clearDraft();
    await endTyping(true);
    return;
  }
  useOverlay.setState({ attachment: shot });
  await focusAskBox();
}

// ---------------------------------------------------------------------------
// Ask box

function clearDraft() {
  useOverlay.setState({ draft: "", attachment: null, replyTo: null });
}

export function submitAsk(): boolean {
  const { draft, attachment, replyTo } = useOverlay.getState();
  const q = draft.trim();
  if (!q && !attachment) return false;
  pipeline.ask({ question: q, screenshot: attachment, replyTo });
  clearDraft();
  void endTyping(true);
  return true;
}

export async function focusAskBox(): Promise<void> {
  if (!useOverlay.getState().typing) {
    useOverlay.setState({ typing: true });
    try {
      await native.windowBeginTyping();
    } catch {
      /* focus still works in most cases */
    }
  }
  useOverlay.setState((s) => ({ focusSignal: s.focusSignal + 1 }));
}

export async function endTyping(restoreFocus: boolean): Promise<void> {
  if (!useOverlay.getState().typing) return;
  useOverlay.setState({ typing: false });
  try {
    await native.windowEndTyping(restoreFocus);
  } catch {
    /* ignore */
  }
}

export function replyTo(cardId: string) {
  useOverlay.setState({ replyTo: cardId });
  void focusAskBox();
}

// ---------------------------------------------------------------------------
// Panel ↔ pill

let sizeTimer: ReturnType<typeof setTimeout> | null = null;
/**
 * window_set_size isn't persisted natively; the launch size comes from
 * settings.overlay.width/height. Save the expanded panel size after the user
 * resizes it (never the pill size).
 */
function rememberPanelSize(width: number, height: number) {
  // Heights set by auto-fit are temporary; never save them as the user's size.
  if (autoFit || autoFitBusy) return;
  if (useOverlay.getState().collapsed || width < 300 || height < 200) return;
  if (sizeTimer) clearTimeout(sizeTimer);
  sizeTimer = setTimeout(() => {
    if (useOverlay.getState().collapsed) return;
    const o = useSettings.getState().settings.overlay;
    const w = Math.round(width);
    const h = Math.round(height);
    if (Math.abs(o.width - w) > 2 || Math.abs(o.height - h) > 2) {
      void useSettings.getState().update({ overlay: { width: w, height: h } }).catch(() => {});
    }
  }, 900);
}

// ---------------------------------------------------------------------------
// Auto-fit: grow the panel so the newest answer fits without scrolling, then
// return to the user's own size (and position) once it isn't needed.

let autoFit: { baseHeight: number; baseY: number } | null = null;
let autoFitBusy = false;
let autoFitQueued: { needed: number; allowShrink: boolean } | null = null;

const SCREEN_MARGIN = 12;

/**
 * `needed` is the window height (logical px) that would show the newest answer in full,
 * or 0 to go back to the user's size. Grows immediately; shrinks only when allowed
 * (not while an answer is still streaming) to avoid the panel jumping around.
 */
export function fitOverlayHeight(needed: number, allowShrink: boolean): void {
  autoFitQueued = { needed, allowShrink };
  if (!autoFitBusy) void drainAutoFit();
}

async function drainAutoFit(): Promise<void> {
  autoFitBusy = true;
  try {
    while (autoFitQueued) {
      const { needed, allowShrink } = autoFitQueued;
      autoFitQueued = null;
      await applyAutoFit(needed, allowShrink);
    }
  } finally {
    autoFitBusy = false;
  }
}

async function applyAutoFit(needed: number, allowShrink: boolean): Promise<void> {
  const { collapsed, win } = useOverlay.getState();
  if (collapsed || !win) return;
  const userH = useSettings.getState().settings.overlay.height;
  const availTop = (window.screen as Screen & { availTop?: number }).availTop ?? 0;
  const availH = window.screen.availHeight;
  const maxH = Math.max(userH, availH - 2 * SCREEN_MARGIN);
  const cur = win.bounds.height;
  const target = Math.round(Math.min(maxH, Math.max(userH, needed)));

  if (target > cur + 6) {
    if (!autoFit) autoFit = { baseHeight: cur, baseY: win.bounds.y };
    const st = await native.windowSetSize({ height: target }).catch(() => null);
    if (!st) return;
    useOverlay.setState({ win: st });
    // Keep the bottom on screen: move up if the taller panel would run off it.
    const bottom = st.bounds.y + st.bounds.height;
    const limit = availTop + availH - SCREEN_MARGIN;
    if (bottom > limit) {
      const dy = Math.round(limit - bottom);
      const moved = await native.windowMoveBy(0, dy).catch(() => null);
      if (moved) useOverlay.setState({ win: moved });
    }
  } else if (autoFit && allowShrink && target <= autoFit.baseHeight + 6 && cur > autoFit.baseHeight + 6) {
    const back = autoFit;
    autoFit = null;
    const st = await native.windowSetSize({ height: back.baseHeight }).catch(() => null);
    if (st) {
      useOverlay.setState({ win: st });
      // Back to where it was before it grew (it may have moved up to stay on screen).
      const dy = Math.round(back.baseY - st.bounds.y);
      if (Math.abs(dy) > 1) {
        const moved = await native.windowMoveBy(0, dy).catch(() => null);
        if (moved) useOverlay.setState({ win: moved });
      }
    }
  } else if (autoFit && allowShrink && target < cur - 24) {
    // Still taller than the user's size, but less is needed now.
    const st = await native.windowSetSize({ height: target }).catch(() => null);
    if (st) useOverlay.setState({ win: st });
  }
}

/** Forget auto-fit (e.g. before collapsing) and report the user's own height. */
function endAutoFit(): number | null {
  const base = autoFit?.baseHeight ?? null;
  autoFit = null;
  autoFitQueued = null;
  return base;
}

export async function collapse(): Promise<void> {
  const win = useOverlay.getState().win;
  const s = useSettings.getState().settings.overlay;
  const baseHeight = endAutoFit();
  const savedSize = win
    ? { width: win.bounds.width, height: baseHeight ?? win.bounds.height }
    : { width: s.width, height: s.height };
  useOverlay.setState({ collapsed: true, savedSize });
  await native.windowSetSize(PILL_SIZE).catch(() => {});
}

export function expand(): void {
  const { collapsed, savedSize } = useOverlay.getState();
  if (!collapsed) return;
  useOverlay.setState({ collapsed: false });
  const s = useSettings.getState().settings.overlay;
  void native.windowSetSize(savedSize ?? { width: s.width, height: s.height }).catch(() => {});
}

export async function setMode(mode: Mode): Promise<void> {
  await useSettings.getState().setMode(mode);
  await onModeChanged(mode);
  if (mode !== "meeting" && mode !== "lecture") useOverlay.setState({ panelTab: "transcript" });
}

export async function setClickThrough(enabled: boolean): Promise<void> {
  await native.windowSetClickThrough(enabled).catch(() => {});
}

export async function setContentProtection(enabled: boolean): Promise<void> {
  // Rust applies it to every window and persists settings.overlay.contentProtection.
  await native.windowSetContentProtection(enabled).catch(() => {});
}

export function openDashboard(route = "/dashboard") {
  void native.dashboardOpen(route).catch(() => {});
}

export function hasCards(): boolean {
  return useCards.getState().cards.length > 0;
}
