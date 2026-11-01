// Splits a streamed answer into the glanceable parts the card renders:
// a one-line headline, the main body (bullets), and collapsible details.

import { NULL_SENTINEL } from "./prompts";

export interface ParsedAnswer {
  headline: string;
  body: string;
  details: string | null;
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
  if (!text) return { headline: "", body: "", details: null };

  const nl = text.indexOf("\n");
  const firstLine = nl === -1 ? text : text.slice(0, nl);
  // A response that opens with a code fence or bullet has no headline line.
  if (/^\s*```/.test(firstLine) || /^\s*[-*•]\s/.test(firstLine)) {
    return { headline: "", body: text, details: null };
  }
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
  return { headline, body: body.trim(), details: details || null };
}

/** True once the stream so far shows the ∅ "not a question" sentinel. */
export function isNullAnswer(textSoFar: string): boolean {
  return textSoFar.trimStart().startsWith(NULL_SENTINEL);
}

/** Whether enough text arrived to decide between ∅ and a real answer. */
export function isDecided(textSoFar: string): boolean {
  return textSoFar.trimStart().length > 0;
}
