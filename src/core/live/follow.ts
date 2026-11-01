// "Follow my voice": lines up what the microphone hears with the answer the user is
// reading aloud, so the overlay can scroll to the line they're on and dim what they've
// already said. Pure and monotonic: the position only ever moves forward, filler words
// and ad-libs are skipped, and common short words can't cause long jumps.

export interface ScriptToken {
  w: string;
  line: number;
}

export interface SpeechProgress {
  /** True once anything the user said matched the answer. */
  active: boolean;
  /** The line the user is on (the next unspoken word's line); lines.length when finished. */
  line: number;
  /** How many script tokens have been spoken. */
  spoken: number;
  total: number;
}

export const NO_PROGRESS: SpeechProgress = { active: false, line: 0, spoken: 0, total: 0 };

/** Lowercase words without punctuation; "I'd" → "id", "Retry-After" → "retry", "after". */
export function normalizeWords(text: string): string[] {
  return text
    .toLowerCase()
    .replace(/[’'`]/g, "")
    .replace(/[^\p{L}\p{N}]+/gu, " ")
    .split(" ")
    .filter(Boolean);
}

/** Strips Markdown emphasis and code markers before tokenizing. */
const plain = (s: string) => s.replace(/\*\*|__|`/g, "").replace(/\[([^\]]*)\]\([^)]*\)/g, "$1");

export function scriptTokens(lines: string[]): ScriptToken[] {
  const out: ScriptToken[] = [];
  lines.forEach((line, i) => {
    for (const w of normalizeWords(plain(line))) out.push({ w, line: i });
  });
  return out;
}

/** Words too common to justify skipping ahead on their own. */
const COMMON = new Set(
  "a an the and or but so to of in on at for with by from as is are was were be been it its i im id ive we our you your they them that this these those there then than just also very really".split(" "),
);

const LOOKAHEAD = 8;

function sameWord(a: string, b: string): boolean {
  if (a === b) return true;
  // Speech-to-text often drops or adds a suffix: "check" / "checks", "build" / "built".
  if (a.length >= 4 && b.length >= 4) {
    const n = Math.min(a.length, b.length) - 1;
    return n >= 3 && a.slice(0, n) === b.slice(0, n);
  }
  return false;
}

/** How far into the script the spoken words reach (index just past the last matched token). */
export function followPosition(tokens: ScriptToken[], spoken: string[]): number {
  let p = 0;
  for (const s of spoken) {
    if (p >= tokens.length) break;
    // Common words only match the very next token or the one after it.
    const limit = Math.min(tokens.length, p + (COMMON.has(s) ? 2 : LOOKAHEAD));
    for (let j = p; j < limit; j++) {
      if (sameWord(tokens[j].w, s)) {
        p = j + 1;
        break;
      }
    }
  }
  return p;
}

export function speechProgress(lines: string[], spokenText: string): SpeechProgress {
  const tokens = scriptTokens(lines);
  if (!tokens.length) return NO_PROGRESS;
  const spoken = followPosition(tokens, normalizeWords(spokenText));
  const line = spoken >= tokens.length ? lines.length : tokens[spoken].line;
  return { active: spoken > 0, line, spoken, total: tokens.length };
}
