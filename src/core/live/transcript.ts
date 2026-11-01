// Pure transcript assembly: interim results replace each other in place by
// segId, segments stay in session-clock order across both speakers, and echo
// segments (mic audio duplicating "them") are hidden by default.

import type { ContextLine } from "../ai/context";
import type { Source, TranscriptEvent } from "../types";

export interface Segment {
  segId: string;
  source: Source;
  text: string;
  isFinal: boolean;
  t0Ms: number;
  t1Ms: number;
  echo: boolean;
  /** Arrival order; breaks ties between segments that start at the same ms. */
  seq: number;
}

export interface TranscriptState {
  segs: Record<string, Segment>;
  order: string[];
  seq: number;
  /** Echo patches that arrived before their segment. */
  pendingEcho: Record<string, boolean>;
}

export const emptyTranscript = (): TranscriptState => ({ segs: {}, order: [], seq: 0, pendingEcho: {} });

function before(a: Segment, b: Segment): boolean {
  return a.t0Ms < b.t0Ms || (a.t0Ms === b.t0Ms && a.seq < b.seq);
}

function insertSorted(order: string[], segs: Record<string, Segment>, seg: Segment): string[] {
  let lo = 0;
  let hi = order.length;
  while (lo < hi) {
    const mid = (lo + hi) >> 1;
    if (before(segs[order[mid]], seg)) lo = mid + 1;
    else hi = mid;
  }
  const next = order.slice();
  next.splice(lo, 0, seg.segId);
  return next;
}

export function applyTranscript(state: TranscriptState, ev: TranscriptEvent): TranscriptState {
  const existing = state.segs[ev.segId];
  if (existing?.isFinal && !ev.isFinal) return state; // a late interim never overwrites a final

  const echo = !!ev.echo || !!state.pendingEcho[ev.segId] || !!existing?.echo;
  const seg: Segment = {
    segId: ev.segId,
    source: ev.source,
    text: ev.text,
    isFinal: ev.isFinal,
    t0Ms: ev.t0Ms,
    t1Ms: ev.t1Ms,
    echo,
    seq: existing ? existing.seq : state.seq,
  };
  const segs = { ...state.segs, [ev.segId]: seg };
  let order = state.order;
  if (!existing) {
    order = insertSorted(state.order, segs, seg);
  } else if (existing.t0Ms !== seg.t0Ms) {
    order = insertSorted(
      state.order.filter((id) => id !== seg.segId),
      segs,
      seg,
    );
  }
  let pendingEcho = state.pendingEcho;
  if (pendingEcho[ev.segId] !== undefined) {
    pendingEcho = { ...pendingEcho };
    delete pendingEcho[ev.segId];
  }
  return { segs, order, seq: existing ? state.seq : state.seq + 1, pendingEcho };
}

export function applyPatch(state: TranscriptState, segId: string, echo: boolean): TranscriptState {
  const seg = state.segs[segId];
  if (!seg) return { ...state, pendingEcho: { ...state.pendingEcho, [segId]: echo } };
  if (seg.echo === echo) return state;
  return { ...state, segs: { ...state.segs, [segId]: { ...seg, echo } } };
}

/** Drops the oldest segments beyond `max` (the database keeps everything). */
export function trimTranscript(state: TranscriptState, max: number): TranscriptState {
  if (state.order.length <= max) return state;
  const drop = state.order.slice(0, state.order.length - max);
  const segs = { ...state.segs };
  for (const id of drop) delete segs[id];
  return { ...state, segs, order: state.order.slice(-max) };
}

export function segments(state: TranscriptState, opts: { showEcho?: boolean } = {}): Segment[] {
  const out: Segment[] = [];
  for (const id of state.order) {
    const s = state.segs[id];
    if (!s || (!opts.showEcho && s.echo)) continue;
    if (!s.text.trim()) continue;
    out.push(s);
  }
  return out;
}

export interface SpeakerBlock {
  key: string;
  source: Source;
  t0Ms: number;
  parts: { segId: string; text: string; isFinal: boolean }[];
}

/** Consecutive segments from one speaker render as one block. */
export function speakerBlocks(segs: Segment[]): SpeakerBlock[] {
  const blocks: SpeakerBlock[] = [];
  for (const s of segs) {
    const last = blocks[blocks.length - 1];
    if (last && last.source === s.source) {
      last.parts.push({ segId: s.segId, text: s.text, isFinal: s.isFinal });
    } else {
      blocks.push({
        key: s.segId,
        source: s.source,
        t0Ms: s.t0Ms,
        parts: [{ segId: s.segId, text: s.text, isFinal: s.isFinal }],
      });
    }
  }
  return blocks;
}

/** Final, non-echo lines for prompts (chronological). */
export function finalLines(state: TranscriptState): ContextLine[] {
  return segments(state)
    .filter((s) => s.isFinal)
    .map((s) => ({ source: s.source, text: s.text, t0Ms: s.t0Ms, t1Ms: s.t1Ms }));
}
