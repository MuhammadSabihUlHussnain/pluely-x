// Splits a streamed answer into the glanceable parts the card renders:
// the opening line to say, the talking points (body), a private note that is
// never said out loud, and collapsible details.

import { NULL_SENTINEL } from "./prompts";

export interface ParsedAnswer {
  headline: string;
  body: string;
  details: string | null;
  /** Private tip for the user ("Note: …"), shown apart from the spoken text. */
  note?: string | null;
}

/** A "Note: …" line, optionally bulleted or bold-labelled. */
const NOTE_LINE = /^\s*(?:[-*•]\s+)?(?:\*\*|__)?note(?:\*\*|__)?\s*:\s*(?:\*\*|__)?\s*(.+)$/i;

/** Pulls "Note:" lines out of the talking points. */
function extractNote(body: string): { body: string; note: string | null } {
  const notes: string[] = [];
  const kept = body.split("\n").filter((line) => {
    const m = NOTE_LINE.exec(line);
    if (m) notes.push(m[1].trim());
    return !m;
  });
  return { body: kept.join("\n").trim(), note: notes.length ? notes.join(" ") : null };
}

/** The talking points as plain sentences (bullet markers and emphasis removed). */
export function talkingPoints(body: string): string[] | null {
  const lines = body.split("\n").filter((l) => l.trim());
  if (!lines.length || !lines.every((l) => /^\s*[-*•]\s+/.test(l))) return null;
  return lines.map((l) => l.replace(/^\s*[-*•]\s+/, "").trim());
}

/** What the user would say: the opening line plus the talking points, as plain text. */
export function speakableText(p: ParsedAnswer): string {
  const strip = (s: string) => s.replace(/\*\*|__|`/g, "").replace(/(^|\s)[*_](\S[^*_]*)[*_](?=\s|$|[.,!?])/g, "$1$2");
  const points = talkingPoints(p.body) ?? (p.body ? [p.body] : []);
  return [p.headline, ...points].filter(Boolean).map(strip).join("\n");
}

const DETAILS_MARKER = /^\s*(?:#{1,6}\s*)?(?:\*\*|__)?details(?:\*\*|__)?\s*:?\s*(?:\*\*|__)?\s*$/i;

function cleanHeadline(line: string): string {
  return line
    .trim()
    .replace(/^#{1,6}\s+/, "")
    .replace(/^(?:\*\*|__)(.*)(?:\*\*|__)$/, "$1")
    .replace(/^(?:answer|say|suggested reply)\s*:\s*/i, "")
    .trim();
}

export function parseAnswer(content: string): ParsedAnswer {
  const text = content.replace(/^\s+/, "");
  if (!text) return { headline: "", body: "", details: null, note: null };

  const nl = text.indexOf("\n");
  const firstLine = nl === -1 ? text : text.slice(0, nl);
  // A response that opens with a code fence or bullet has no headline line.
  if (/^\s*```/.test(firstLine)) return { headline: "", body: text, details: null, note: null };
  if (/^\s*[-*•]\s/.test(firstLine)) return { headline: "", ...extractNote(text), details: null };
  const headline = cleanHeadline(firstLine);
  const rest = nl === -1 ? "" : text.slice(nl + 1);

  const lines = rest.split("\n");
  const markerIdx = lines.findIndex((l) => DETAILS_MARKER.test(l));
  let body: string;
  let details: string | null = null;
  if (markerIdx !== -1) {
    body = lines.slice(0, markerIdx).join("\n");
    details = lines.slice(markerIdx + 1).join("\n").trim();
  } else {
    // No explicit marker: a long code block still belongs under details.
    const fenceIdx = lines.findIndex((l) => /^\s*```/.test(l));
    if (fenceIdx > 0) {
      body = lines.slice(0, fenceIdx).join("\n");
      details = lines.slice(fenceIdx).join("\n").trim();
    } else {
      body = rest;
    }
  }
  const { body: points, note } = extractNote(body);
  return { headline, body: points, details: details || null, note };
}

/** True once the stream so far shows the ∅ "not a question" sentinel. */
export function isNullAnswer(textSoFar: string): boolean {
  return textSoFar.trimStart().startsWith(NULL_SENTINEL);
}

/** Whether enough text arrived to decide between ∅ and a real answer. */
export function isDecided(textSoFar: string): boolean {
  return textSoFar.trimStart().length > 0;
}
