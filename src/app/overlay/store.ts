// Overlay UI store: panel/pill, transcript strip, ask box draft, window state.

import { create } from "zustand";
import type { CardScreenshot } from "@/core/live/cards";
import type { WindowState } from "@/core/types";

export interface OverlayUiState {
  collapsed: boolean;
  /** Panel size to restore when the pill expands. */
  savedSize: { width: number; height: number } | null;
  transcriptOpen: boolean;
  panelTab: "transcript" | "notes";
  win: WindowState | null;
  typing: boolean;
  draft: string;
  attachment: CardScreenshot | null;
  replyTo: string | null;
  /** Bumped to ask the AskBox to focus its textarea. */
  focusSignal: number;
  capturing: "full" | "region" | null;
  toast: { id: number; text: string; tone: "info" | "error" } | null;
}

export const useOverlay = create<OverlayUiState>(() => ({
  collapsed: false,
  savedSize: null,
  transcriptOpen: true,
  panelTab: "transcript",
  win: null,
  typing: false,
  draft: "",
  attachment: null,
  replyTo: null,
  focusSignal: 0,
  capturing: null,
  toast: null,
}));

let toastTimer: ReturnType<typeof setTimeout> | null = null;
export function showToast(text: string, tone: "info" | "error" = "info") {
  if (toastTimer) clearTimeout(toastTimer);
  useOverlay.setState({ toast: { id: Date.now(), text, tone } });
  toastTimer = setTimeout(() => useOverlay.setState({ toast: null }), 3200);
}
