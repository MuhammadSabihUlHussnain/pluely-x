// "Follow my voice": lines up what the microphone hears with the answer the user is
// reading aloud, so the overlay can scroll to the part they're on and fade what they've
// already said. Works on any answer layout (paragraphs, bullets, headed sections): the
// overlay hands in the answer's visible text blocks in reading order.
//
// Two signals, the further one wins, and the position only ever moves forward:
// 1. Word matching. Numbers are normalised ("1 billion" = "1,000,000,000", "5.3x" =
//    "5.3 times", "six" = "6"), fillers are skipped, and after a skipped or reworded stretch
//    a distinctive word or word pair further ahead resynchronises the position.
// 2. Speaking pace. How many content words the user has said since the answer appeared,
//    so it keeps moving even while they paraphrase rather than read.

export interface ScriptToken {
  w: string;
  line: number;
}

export interface SpeechProgress {
  /** True once the user has said something that counts. */
  active: boolean;
  /** The block the user is on (the next unspoken word's block); lines.length when finished. */
  line: number;
  /** How many script tokens count as said. */
  spoken: number;
  total: number;
}

export const NO_PROGRESS: SpeechProgress = { active: false, line: 0, spoken: 0, total: 0 };

const NUMBER_WORDS: Record<string, string> = {
  zero: "0", one: "1", two: "2", three: "3", four: "4", five: "5", six: "6", seven: "7", eight: "8",
  nine: "9", ten: "10", eleven: "11", twelve: "12", thirteen: "13", fourteen: "14", fifteen: "15",
  sixteen: "16", seventeen: "17", eighteen: "18", nineteen: "19", twenty: "20", thirty: "30",
  forty: "40", fifty: "50", sixty: "60", seventy: "70", eighty: "80", ninety: "90", hundred: "100",
};
const SCALES: Record<string, number> = {
  thousand: 1e3, k: 1e3, million: 1e6, mm: 1e6, m: 1e6, mn: 1e6, billion: 1e9, bn: 1e9, b: 1e9, trillion: 1e12, tn: 1e12,
};

function fmt(n: number): string {
  return Number.isInteger(n) ? String(n) : String(Math.round(n * 1000) / 1000);
}

/** Lowercase words without punctuation, with numbers in one canonical form. */
export function normalizeWords(text: string): string[] {
  const pre = text
    .toLowerCase()
    .replace(/[’'`]/g, "")
    .replace(/(\d),(?=\d{3}\b)/g, "$1") // 1,000,000 → 1000000
    .replace(/(\d)\s*x\b/g, "$1 times") // 5.3x → 5.3 times
    .replace(/%/g, " percent")
    .replace(/\$/g, " ");
  const raw = pre.replace(/[^\p{L}\p{N}.]+/gu, " ").split(" ").filter(Boolean);
  const out: string[] = [];
  for (let i = 0; i < raw.length; i++) {
    let w = raw[i].replace(/^\.+|\.+$/g, ""); // sentence dots, not decimal points
    if (!w) continue;
    if (NUMBER_WORDS[w]) w = NUMBER_WORDS[w];
    if (/^\d+(\.\d+)?$/.test(w)) {
      const next = raw[i + 1]?.replace(/\.+$/, "");
      if (next && SCALES[next] !== undefined) {
        w = fmt(parseFloat(w) * SCALES[next]);
        i++;
      }
      out.push(w);
      continue;
    }
    for (const part of w.split(".")) if (part) out.push(part);
  }
  return out;
}

/** Strips Markdown markers before tokenising. */
const plain = (s: string) =>
  s
    .replace(/\*\*|__|`/g, "")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/^\s*[-*•]\s+/gm, "")
    .replace(/^#{1,6}\s+/gm, "");

export function scriptTokens(lines: string[]): ScriptToken[] {
  const out: ScriptToken[] = [];
  lines.forEach((line, i) => {
    for (const w of normalizeWords(plain(line))) out.push({ w, line: i });
  });
  return out;
}

/** Splits an answer's text into reading blocks: paragraphs and list items. */
export function answerBlocks(content: string): string[] {
  const blocks: string[] = [];
  let cur: string[] = [];
  const flush = () => {
    if (cur.length) blocks.push(cur.join(" "));
    cur = [];
  };
  let inCode = false;
  for (const raw of content.split("\n")) {
    const line = raw.trim();
    if (line.startsWith("```")) {
      flush();
      inCode = !inCode;
      continue;
    }
    if (inCode) continue;
    if (!line) flush();
    else if (/^([-*•]|\d+[.)])\s+/.test(line) || /^#{1,6}\s/.test(line)) {
      flush();
      blocks.push(line);
    } else cur.push(line);
  }
  flush();
  return blocks.filter((b) => normalizeWords(plain(b)).length > 0);
}

/** Words too common to justify skipping ahead on their own. */
const COMMON = new Set(
  "a an the and or but so to of in on at for with by from as is are was were be been it its i im id ive we our you your they them that this these those there then than just also very really times percent".split(
    " ",
  ),
);
/** Spoken filler that never counts towards progress. */
const FILLER = new Set("um uh er ah like yeah okay ok so well right alright mean know kind sort basically".split(" "));

const LOOKAHEAD = 8;
/** Fraction of on-topic spoken words assumed to be reading (the rest is ad-libbing). */
const PACE = 0.8;
const RESYNC = 60;

function sameWord(a: string, b: string): boolean {
  if (a === b) return true;
  if (/^\d/.test(a) || /^\d/.test(b)) return false; // numbers must match exactly
  // Speech-to-text often drops or adds a suffix: "check" / "checks", "build" / "built".
  if (a.length >= 4 && b.length >= 4) {
    const n = Math.min(a.length, b.length) - 1;
    return n >= 3 && a.slice(0, n) === b.slice(0, n);
  }
  return false;
}

const distinctive = (w: string) => !COMMON.has(w) && !FILLER.has(w) && (w.length >= 6 || /^\d/.test(w));

/** Recent spoken words compared against the script at each step. */
const WINDOW = 6;
/** How far back from a candidate position the window may match. */
const SPAN = 14;

/** How many of the recent spoken words appear, in order, in the script just before `end`. */
function inOrderMatches(win: string[], tokens: ScriptToken[], end: number): number {
  const start = Math.max(0, end - SPAN);
  let j = end - 1;
  let count = 0;
  for (let i = win.length - 1; i >= 0 && j >= start; i--) {
    let k = j;
    while (k >= start && !sameWord(tokens[k].w, win[i])) k--;
    if (k >= start) {
      count++;
      j = k - 1;
    }
  }
  return count;
}

/** Text ahead of the position checked for "is this still about the answer?" */
const AHEAD = 40;

/**
 * How far into the script the spoken words reach (index just past the last token counted
 * as said). Each spoken word is an anchor: the position moves where the last few spoken
 * words line up in order with the script, needing more agreement the further it jumps, so
 * one stray repeated term ("value", "EBITDA") can't move it, and common words only count
 * when they are literally the next word. A pace estimate keeps it moving while the user
 * paraphrases, but only while what they say still shares words with the text just ahead
 * (side remarks don't count).
 */
export function followPosition(tokens: ScriptToken[], spoken: string[]): number {
  let p = 0;
  let pace = 0;
  const win: string[] = [];
  for (const s of spoken) {
    if (p >= tokens.length) break;
    if (FILLER.has(s)) continue;
    win.push(s);
    if (win.length > WINDOW) win.shift();

    let best = -1;
    if (!COMMON.has(s)) {
      const far = Math.min(tokens.length, p + RESYNC);
      for (let e = p + 1; e <= far; e++) {
        if (!sameWord(tokens[e - 1].w, s)) continue; // this word must be the one just said
        const jump = e - p;
        const need = jump <= 3 ? 1 : jump <= LOOKAHEAD ? 2 : distinctive(s) ? 3 : 4;
        if (inOrderMatches(win, tokens, e) >= need) {
          best = e; // nearest position with enough agreement
          break;
        }
      }
    }
    // Reading word by word: the very next word counts even if it's a common one.
    if (best < 0 && sameWord(tokens[p].w, s)) best = p + 1;
    if (best > p) p = best;

    pace = Math.max(pace, p);
    const ahead = tokens.slice(p, p + AHEAD);
    const related = win.filter((w) => !COMMON.has(w) && ahead.some((t) => sameWord(t.w, w))).length >= 2;
    if (related) pace += PACE;
  }
  return Math.min(tokens.length, Math.max(p, Math.floor(pace)));
}


export function speechProgress(lines: string[], spokenText: string): SpeechProgress {
  const tokens = scriptTokens(lines);
  if (!tokens.length) return NO_PROGRESS;
  const pos = followPosition(tokens, normalizeWords(spokenText));
  const line = pos >= tokens.length ? lines.length : tokens[pos].line;
  return { active: pos > 0, line, spoken: pos, total: tokens.length };
}
