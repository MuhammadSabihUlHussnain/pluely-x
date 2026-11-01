// A brief is optional pasted notes plus any number of documents (PDF, DOCX, TXT, MD).
// The documents' text is kept per file in `briefs.files_json` so files can be listed,
// removed and re-composed; `briefs.content` holds the composed text that is actually sent
// (and indexed), already fitted to the budget so it stays byte-stable for prompt caching.

import type { BriefRow } from "../db/types";

/** Total characters sent as the brief (~4 characters per token, so ~50k tokens). */
export const BRIEF_MAX_CHARS = 200_000;

export interface BriefDoc {
  id: string;
  name: string;
  size: number;
  pages?: number;
  /** Extracted text (full, before any budget trimming). */
  text: string;
}

export interface BriefSources {
  notes: string;
  docs: BriefDoc[];
}

interface StoredV2 {
  v: 2;
  notes: string;
  docs: BriefDoc[];
}

/** Legacy rows stored `[{name,size,chars}]` with all text merged into `content`. */
interface LegacyFile {
  name: string;
  size: number;
  chars: number;
}

export function parseSources(row: Pick<BriefRow, "content" | "filesJson">): BriefSources {
  try {
    const parsed = JSON.parse(row.filesJson) as StoredV2 | LegacyFile[];
    if (parsed && !Array.isArray(parsed) && parsed.v === 2) {
      return { notes: parsed.notes ?? "", docs: Array.isArray(parsed.docs) ? parsed.docs : [] };
    }
  } catch {
    /* fall through to legacy */
  }
  // Legacy: the merged text becomes editable notes.
  return { notes: row.content, docs: [] };
}

/** File names for list views (works for both formats). */
export function sourceNames(row: Pick<BriefRow, "content" | "filesJson">): string[] {
  try {
    const parsed = JSON.parse(row.filesJson) as StoredV2 | LegacyFile[];
    if (Array.isArray(parsed)) return parsed.map((f) => f.name);
    if (parsed?.v === 2) return parsed.docs.map((d) => d.name);
  } catch {
    /* ignore */
  }
  return [];
}

export function serializeSources(s: BriefSources): string {
  const stored: StoredV2 = { v: 2, notes: s.notes, docs: s.docs };
  return JSON.stringify(stored);
}

/**
 * Splits `max` characters fairly between parts: parts that fit their equal share are kept
 * whole and their leftover goes to the others (water-filling). Returns each part's cap.
 */
export function fairCaps(lengths: number[], max: number): number[] {
  const caps = lengths.map(() => 0);
  let remaining = max;
  let open = lengths.map((_, i) => i);
  while (open.length && remaining > 0) {
    const share = Math.floor(remaining / open.length);
    const fits = open.filter((i) => lengths[i] <= share);
    if (!fits.length) {
      for (const i of open) caps[i] = share;
      return caps;
    }
    for (const i of fits) {
      caps[i] = lengths[i];
      remaining -= lengths[i];
    }
    open = open.filter((i) => lengths[i] > share);
  }
  return caps;
}

export interface ComposedBrief {
  content: string;
  /** Characters of each part that didn't fit (notes first, then docs in order). */
  trimmed: { notes: number; docs: number[] };
  totalChars: number;
}

const header = (d: BriefDoc) => `=== ${d.name}${d.pages ? ` (${d.pages} page${d.pages === 1 ? "" : "s"})` : ""} ===`;

/** Composes the text sent to Claude, trimming long parts fairly to fit `max`. */
export function composeBrief(s: BriefSources, max = BRIEF_MAX_CHARS): ComposedBrief {
  const notes = s.notes.trim();
  const parts = [notes, ...s.docs.map((d) => d.text.trim())];
  // Headers and separators are always kept; budget the bodies.
  const overhead = s.docs.reduce((n, d) => n + header(d).length + 2, 0) + (notes && s.docs.length ? 2 : 0);
  const caps = fairCaps(
    parts.map((p) => p.length),
    Math.max(0, max - overhead),
  );
  const cut = (text: string, cap: number) =>
    text.length <= cap ? text : `${text.slice(0, Math.max(0, cap - 60)).trimEnd()}\n[… ${(text.length - cap).toLocaleString("en-US")} more characters not included]`;

  const blocks: string[] = [];
  if (notes) blocks.push(cut(notes, caps[0]));
  s.docs.forEach((d, i) => blocks.push(`${header(d)}\n${cut(parts[i + 1], caps[i + 1])}`));
  const content = blocks.join("\n\n");
  return {
    content,
    trimmed: {
      notes: Math.max(0, notes.length - caps[0]),
      docs: s.docs.map((_, i) => Math.max(0, parts[i + 1].length - caps[i + 1])),
    },
    totalChars: parts.reduce((n, p) => n + p.length, 0),
  };
}

/** Rough token estimate for the UI (~4 characters per token). */
export const approxTokens = (chars: number) => Math.round(chars / 4);
