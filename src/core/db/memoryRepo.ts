// In-memory Repo for the browser mock and unit tests. Same behaviour as the
// SQLite repo, including search across titles, transcript and cards.

import type { Mode } from "../types";
import type {
  BriefRow,
  CardRow,
  NotesRow,
  Repo,
  SessionDetail,
  SessionListItem,
  SessionRow,
  TranscriptRow,
} from "./types";

export interface MemoryData {
  sessions: SessionRow[];
  transcript: TranscriptRow[];
  cards: CardRow[];
  notes: NotesRow[];
  briefs: BriefRow[];
  modePrompts: { mode: Mode; prompt: string; updatedAt: number }[];
}

export const emptyMemoryData = (): MemoryData => ({
  sessions: [],
  transcript: [],
  cards: [],
  notes: [],
  briefs: [],
  modePrompts: [],
});

function words(q: string): string[] {
  return (q.toLowerCase().match(/[\p{L}\p{N}_]+/gu) ?? []).filter(Boolean);
}

function matchesAll(text: string, ws: string[]): boolean {
  const t = text.toLowerCase();
  return ws.every((w) => new RegExp(`(^|[^\\p{L}\\p{N}_])${escapeRe(w)}`, "u").test(t));
}

/** ~110 characters around the first query word, for search results. */
export function excerpt(text: string, ws: string[], width = 110): string {
  const flat = text.replace(/\s+/g, " ").trim();
  const lower = flat.toLowerCase();
  const at = Math.max(0, Math.min(...ws.map((w) => lower.indexOf(w)).filter((i) => i >= 0)));
  if (flat.length <= width) return flat;
  const start = Math.max(0, at - Math.floor(width / 3));
  const end = Math.min(flat.length, start + width);
  return `${start > 0 ? "…" : ""}${flat.slice(start, end).trim()}${end < flat.length ? "…" : ""}`;
}

function escapeRe(s: string): string {
  return s.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
}

/**
 * @param onChange called after every write (the browser mock uses it to
 * persist to localStorage so the overlay and dashboard tabs share data).
 */
export function createMemoryRepo(initial?: MemoryData, onChange?: (d: MemoryData) => void): Repo & { data: MemoryData } {
  const data: MemoryData = initial ?? emptyMemoryData();
  const changed = () => onChange?.(data);

  const repo: Repo & { data: MemoryData } = {
    data,

    async createSession(s) {
      if (data.sessions.some((x) => x.id === s.id)) return;
      data.sessions.push({
        id: s.id,
        mode: s.mode,
        title: s.title ?? "",
        startedAt: s.startedAt,
        endedAt: null,
        briefId: s.briefId ?? null,
        summaryJson: null,
      });
      changed();
    },

    async updateSession(id, patch) {
      const s = data.sessions.find((x) => x.id === id);
      if (!s) return;
      Object.assign(s, Object.fromEntries(Object.entries(patch).filter(([, v]) => v !== undefined)));
      changed();
    },

    async getSession(id) {
      return data.sessions.find((x) => x.id === id) ?? null;
    },

    async listSessions(opts = {}) {
      const ws = words(opts.query ?? "");
      const out: SessionListItem[] = [];
      for (const s of data.sessions) {
        let snippet: string | undefined;
        if (ws.length) {
          const tHit = data.transcript.find((t) => t.sessionId === s.id && matchesAll(t.text, ws));
          const cHit = data.cards.find(
            (c) => c.sessionId === s.id && matchesAll(`${c.question} ${c.content}`, ws),
          );
          const titleHit = matchesAll(s.title, ws);
          if (!tHit && !cHit && !titleHit) continue;
          const hitText = tHit?.text ?? (cHit ? (matchesAll(cHit.question, ws) ? cHit.question : cHit.content) : undefined);
          snippet = hitText ? excerpt(hitText.replace(/[`*#]/g, ""), ws) : undefined;
        }
        out.push({ ...s, cardCount: data.cards.filter((c) => c.sessionId === s.id).length, snippet });
      }
      out.sort((a, b) => b.startedAt - a.startedAt);
      return out.slice(0, opts.limit ?? 200);
    },

    async getSessionDetail(id): Promise<SessionDetail | null> {
      const session = data.sessions.find((x) => x.id === id);
      if (!session) return null;
      return {
        session: { ...session },
        transcript: data.transcript.filter((t) => t.sessionId === id).sort((a, b) => a.t0Ms - b.t0Ms),
        cards: data.cards.filter((c) => c.sessionId === id).sort((a, b) => a.createdAt - b.createdAt),
        notes: data.notes.filter((n) => n.sessionId === id),
      };
    },

    async deleteSession(id) {
      data.sessions = data.sessions.filter((x) => x.id !== id);
      data.transcript = data.transcript.filter((x) => x.sessionId !== id);
      data.cards = data.cards.filter((x) => x.sessionId !== id);
      data.notes = data.notes.filter((x) => x.sessionId !== id);
      changed();
    },

    async deleteAll() {
      data.sessions = [];
      data.transcript = [];
      data.cards = [];
      data.notes = [];
      changed();
    },

    async addTranscript(row) {
      const i = data.transcript.findIndex((x) => x.id === row.id);
      if (i === -1) data.transcript.push({ ...row });
      else data.transcript[i] = { ...row };
      changed();
    },

    async deleteTranscript(id) {
      data.transcript = data.transcript.filter((x) => x.id !== id);
      changed();
    },

    async addCard(row) {
      const i = data.cards.findIndex((x) => x.id === row.id);
      if (i === -1) data.cards.push({ ...row });
      else data.cards[i] = { ...row };
      changed();
    },

    async upsertNotes(row) {
      const i = data.notes.findIndex((n) => n.sessionId === row.sessionId && n.kind === row.kind);
      if (i === -1) data.notes.push({ ...row });
      else data.notes[i] = { ...row };
      changed();
    },

    async listBriefs() {
      return [...data.briefs].sort((a, b) => b.updatedAt - a.updatedAt);
    },

    async saveBrief(b) {
      const i = data.briefs.findIndex((x) => x.id === b.id);
      if (i === -1) data.briefs.push({ ...b });
      else data.briefs[i] = { ...b };
      changed();
    },

    async deleteBrief(id) {
      data.briefs = data.briefs.filter((x) => x.id !== id);
      changed();
    },

    async setDefaultBrief(mode, briefId) {
      for (const b of data.briefs) if (b.mode === mode) b.mode = null;
      const b = briefId ? data.briefs.find((x) => x.id === briefId) : undefined;
      if (b) b.mode = mode;
      changed();
    },

    async defaultBrief(mode) {
      return data.briefs.find((b) => b.mode === mode) ?? null;
    },

    async getModePrompts() {
      const out: Partial<Record<Mode, string>> = {};
      for (const p of data.modePrompts) out[p.mode] = p.prompt;
      return out;
    },

    async setModePrompt(mode, prompt) {
      const i = data.modePrompts.findIndex((p) => p.mode === mode);
      const row = { mode, prompt, updatedAt: Date.now() };
      if (i === -1) data.modePrompts.push(row);
      else data.modePrompts[i] = row;
      changed();
    },

    async resetModePrompt(mode) {
      data.modePrompts = data.modePrompts.filter((p) => p.mode !== mode);
      changed();
    },
  };
  return repo;
}
