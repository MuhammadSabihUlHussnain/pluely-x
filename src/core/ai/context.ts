// Chronological, speaker-labelled, size-budgeted transcript context.

import type { Source } from "../types";

export interface ContextLine {
  source: Source;
  text: string;
  t0Ms: number;
  t1Ms?: number;
}

export const SPEAKER_LABEL: Record<Source, string> = { them: "Them", me: "You" };

export const OMITTED_MARKER = "[earlier transcript omitted]";

/**
 * Formats transcript lines oldest-first as `Them: …` / `You: …`, merging
 * consecutive lines from the same speaker. When the result exceeds
 * `maxChars`, the oldest lines are dropped (the most recent context wins).
 */
export function formatTranscript(lines: ContextLine[], maxChars: number): string {
  const sorted = [...lines]
    .filter((l) => l.text.trim().length > 0)
    .sort((a, b) => a.t0Ms - b.t0Ms);

  const merged: { source: Source; text: string }[] = [];
  for (const l of sorted) {
    const last = merged[merged.length - 1];
    const text = l.text.trim();
    if (last && last.source === l.source) last.text += " " + text;
    else merged.push({ source: l.source, text });
  }

  const rendered = merged.map((m) => `${SPEAKER_LABEL[m.source]}: ${m.text}`);
  let total = rendered.reduce((n, r) => n + r.length + 1, 0);
  let start = 0;
  while (total > maxChars && start < rendered.length - 1) {
    total -= rendered[start].length + 1;
    start++;
  }
  let kept = rendered.slice(start);
  // A single huge line still has to respect the budget: keep its tail.
  if (kept.length === 1 && kept[0].length > maxChars) {
    const label = kept[0].slice(0, kept[0].indexOf(":") + 2);
    kept = [label + "…" + kept[0].slice(kept[0].length - (maxChars - label.length - 1))];
  }
  const body = kept.join("\n");
  return start > 0 || kept[0] !== rendered[0] ? `${OMITTED_MARKER}\n${body}` : body;
}

/** Lines whose end time is within `windowMs` of the newest line. */
export function lastWindow(lines: ContextLine[], windowMs: number): ContextLine[] {
  if (lines.length === 0) return lines;
  const newest = Math.max(...lines.map((l) => l.t1Ms ?? l.t0Ms));
  return lines.filter((l) => (l.t1Ms ?? l.t0Ms) >= newest - windowMs);
}

/** Pulls likely Deepgram keyterms (names, products, tech terms) out of a brief. */
export function extractKeyterms(text: string, max = 40): string[] {
  const counts = new Map<string, number>();
  const re = /\b([A-Z][a-zA-Z0-9+#.]*(?:\s[A-Z][a-zA-Z0-9+#.]*){0,2}|[a-zA-Z]+[0-9+#][a-zA-Z0-9+#.]*|[A-Z]{2,}[a-z]*)\b/g;
  const stop = new Set([
    "I", "The", "A", "An", "And", "Or", "In", "On", "At", "To", "For", "Of", "With", "My",
    "We", "Our", "This", "That", "It", "As", "By", "From", "Is", "Are", "Was", "Be", "Led",
    "Built", "Worked", "Responsible", "Experience", "Education", "Skills", "Summary",
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
  ]);
  for (const m of text.matchAll(re)) {
    const term = m[1].replace(/[.,]+$/, "").trim();
    if (term.length < 2 || term.length > 40 || stop.has(term)) continue;
    counts.set(term, (counts.get(term) ?? 0) + 1);
  }
  return [...counts.entries()]
    .sort((a, b) => b[1] - a[1] || a[0].localeCompare(b[0]))
    .slice(0, max)
    .map(([t]) => t);
}
