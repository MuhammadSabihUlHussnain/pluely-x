// Local, zero-latency pre-filter that decides which *Them* turns are worth
// sending to Claude for the single-pass detect+answer call. It is tuned for
// recall: Claude makes the final call (∅ for non-questions).

const WH_WORDS = ["what", "why", "how", "when", "where", "which", "who", "whom", "whose"];

const AUX_STARTS = [
  "can", "could", "would", "will", "do", "does", "did", "have", "has", "is", "are",
  "was", "were", "should", "shall", "may", "might",
];

const REQUEST_STARTS = [
  "tell me", "walk me through", "walk us through", "talk me through", "talk about",
  "describe", "explain", "give me", "give us", "share", "show me", "help me",
  "name", "list", "define", "compare", "imagine", "suppose", "let's say", "say you",
];

const PHRASES = [
  "can you", "could you", "would you", "tell me", "walk me through", "walk us through",
  "how would you", "how do you", "how did you", "what's your", "what is your",
  "what are your", "why did you", "why do you", "do you have", "have you ever",
  "are you familiar", "your experience with", "your thoughts on", "what do you think",
  "any questions for", "how about you", "and you?",
];

function normalise(text: string): string {
  return text
    .toLowerCase()
    .replace(/[’`]/g, "'")
    .replace(/\s+/g, " ")
    .trim();
}

/** Splits into sentence-ish clauses, so "Great. So how would you…" is caught. */
function clauses(text: string): string[] {
  return text
    .split(/(?<=[.!?;:])\s+|\s+(?:so|and|okay|ok|alright|now|then|but)\s*,?\s+/i)
    .map((s) => s.trim().replace(/^[,\-–—\s]+/, ""))
    .filter(Boolean);
}

export interface PrefilterResult {
  candidate: boolean;
  reason: "hint" | "question-mark" | "wh-word" | "auxiliary" | "request" | "phrase" | "none";
}

export function prefilter(text: string, nativeQuestionHint = false): PrefilterResult {
  const t = normalise(text);
  if (!t) return { candidate: false, reason: "none" };
  if (nativeQuestionHint) return { candidate: true, reason: "hint" };
  if (/\?\s*["')\]]*$/.test(t) || t.includes("?")) return { candidate: true, reason: "question-mark" };

  const words = t.split(" ");
  if (words.length < 2) return { candidate: false, reason: "none" };

  for (const c of clauses(t)) {
    const first = c.split(" ")[0]?.replace(/[^a-z']/g, "") ?? "";
    const firstBase = first.replace(/'s$|'re$|'d$|'ll$/, "");
    if (WH_WORDS.includes(firstBase)) return { candidate: true, reason: "wh-word" };
    if (AUX_STARTS.includes(first) && /\byou\b|\byour\b/.test(c))
      return { candidate: true, reason: "auxiliary" };
    if (REQUEST_STARTS.some((r) => c.startsWith(r))) return { candidate: true, reason: "request" };
  }
  if (PHRASES.some((p) => t.includes(p))) return { candidate: true, reason: "phrase" };
  return { candidate: false, reason: "none" };
}

export function isCandidateQuestion(text: string, nativeQuestionHint = false): boolean {
  return prefilter(text, nativeQuestionHint).candidate;
}
