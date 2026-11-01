// Answer cards: the data the AnswerStack renders, plus a zustand store.

import { create } from "zustand";
import type { CardError } from "../ai/errors";
import type { FollowupChip } from "../ai/prompts";
import type { SessionSummaryJson } from "../ai/schemas";
import type { Mode } from "../types";

/** "heard": a question from the other side in a mode without auto-answers, shown with an Answer button. */
export type CardKind = "auto" | "ask_now" | "typed" | "screen" | "followup" | "summary" | "heard";
export type CardStatus = "pending" | "thinking" | "streaming" | "done" | "stopped" | "error";

export interface CardScreenshot {
  id: string;
  mime: string;
  thumbDataUrl: string;
  path?: string;
}

export interface Card {
  id: string;
  kind: CardKind;
  mode: Mode;
  question: string;
  turnId?: string;
  parentId?: string;
  chip?: FollowupChip;
  screenshot?: CardScreenshot;
  status: CardStatus;
  content: string;
  /** Set when the stream ended for a reason the user should see. */
  stopNote?: "refusal" | "max_tokens";
  error?: CardError;
  /** Auto-answers stay provisional until the stream proves it isn't ∅. */
  provisional?: boolean;
  createdAt: number;
  summary?: SessionSummaryJson;
  persisted?: boolean;
}

export interface CardSink {
  add(card: Card): void;
  update(id: string, patch: Partial<Card>): void;
  remove(id: string): void;
  get(id: string): Card | undefined;
  all(): Card[];
}

interface CardsState {
  cards: Card[];
  add(card: Card): void;
  update(id: string, patch: Partial<Card>): void;
  remove(id: string): void;
  clear(): void;
}

export const useCards = create<CardsState>((set) => ({
  cards: [],
  add: (card) => set((s) => ({ cards: [...s.cards, card] })),
  update: (id, patch) =>
    set((s) => ({ cards: s.cards.map((c) => (c.id === id ? { ...c, ...patch } : c)) })),
  remove: (id) => set((s) => ({ cards: s.cards.filter((c) => c.id !== id) })),
  clear: () => set({ cards: [] }),
}));

export const cardSink: CardSink = {
  add: (c) => useCards.getState().add(c),
  update: (id, p) => useCards.getState().update(id, p),
  remove: (id) => useCards.getState().remove(id),
  get: (id) => useCards.getState().cards.find((c) => c.id === id),
  all: () => useCards.getState().cards,
};

/** In-memory sink for tests. */
export function memoryCardSink(): CardSink & { cards: Card[] } {
  const sink = {
    cards: [] as Card[],
    add(c: Card) {
      sink.cards.push(c);
    },
    update(id: string, p: Partial<Card>) {
      const i = sink.cards.findIndex((c) => c.id === id);
      if (i !== -1) sink.cards[i] = { ...sink.cards[i], ...p };
    },
    remove(id: string) {
      sink.cards = sink.cards.filter((c) => c.id !== id);
    },
    get(id: string) {
      return sink.cards.find((c) => c.id === id);
    },
    all() {
      return sink.cards;
    },
  };
  return sink;
}
