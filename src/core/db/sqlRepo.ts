// SQLite Repo over @tauri-apps/plugin-sql (`sqlite:pluelyx.db`). Migrations
// run in Rust (src-tauri/src/db); this module only queries. Child rows are
// deleted explicitly because foreign_keys is a per-connection pragma.

import Database from "@tauri-apps/plugin-sql";
import type { Mode, Source } from "../types";
import {
  toFtsQuery,
  type BriefRow,
  type CardRow,
  type Repo,
  type SessionListItem,
  type SessionRow,
} from "./types";

export const DB_URL = "sqlite:pluelyx.db";

interface SessionSql {
  id: string;
  mode: string;
  title: string;
  started_at: number;
  ended_at: number | null;
  brief_id: string | null;
  summary_json: string | null;
  card_count?: number;
  snippet?: string | null;
}

const toSession = (r: SessionSql): SessionRow => ({
  id: r.id,
  mode: r.mode as Mode,
  title: r.title,
  startedAt: Number(r.started_at),
  endedAt: r.ended_at == null ? null : Number(r.ended_at),
  briefId: r.brief_id,
  summaryJson: r.summary_json,
});

interface BriefSql {
  id: string;
  name: string;
  mode: string | null;
  content: string;
  files_json: string;
  created_at: number;
  updated_at: number;
}
const toBrief = (r: BriefSql): BriefRow => ({
  id: r.id,
  name: r.name,
  mode: (r.mode as Mode) ?? null,
  content: r.content,
  filesJson: r.files_json,
  createdAt: Number(r.created_at),
  updatedAt: Number(r.updated_at),
});

const LIST_COLUMNS = `s.id, s.mode, s.title, s.started_at, s.ended_at, s.brief_id, s.summary_json,
  (SELECT COUNT(*) FROM cards c WHERE c.session_id = s.id) AS card_count`;

export function createSqlRepo(url = DB_URL): Repo {
  let dbp: Promise<Database> | null = null;
  const db = () => (dbp ??= Database.load(url));

  return {
    async createSession(s) {
      await (await db()).execute(
        `INSERT OR IGNORE INTO sessions (id, mode, title, started_at, brief_id) VALUES ($1, $2, $3, $4, $5)`,
        [s.id, s.mode, s.title ?? "", s.startedAt, s.briefId ?? null],
      );
    },

    async updateSession(id, patch) {
      const cols: string[] = [];
      const vals: unknown[] = [];
      const map: Record<string, string> = {
        title: "title",
        endedAt: "ended_at",
        summaryJson: "summary_json",
        mode: "mode",
        briefId: "brief_id",
      };
      for (const [k, v] of Object.entries(patch)) {
        if (v === undefined || !map[k]) continue;
        vals.push(v);
        cols.push(`${map[k]} = $${vals.length}`);
      }
      if (!cols.length) return;
      vals.push(id);
      await (await db()).execute(`UPDATE sessions SET ${cols.join(", ")} WHERE id = $${vals.length}`, vals);
    },

    async getSession(id) {
      const rows = await (await db()).select<SessionSql[]>(`SELECT * FROM sessions WHERE id = $1`, [id]);
      return rows[0] ? toSession(rows[0]) : null;
    },

    async listSessions(opts = {}) {
      const d = await db();
      const limit = opts.limit ?? 200;
      const fts = toFtsQuery(opts.query ?? "");
      let rows: SessionSql[];
      if (!fts) {
        rows = await d.select<SessionSql[]>(
          `SELECT ${LIST_COLUMNS} FROM sessions s ORDER BY s.started_at DESC LIMIT $1`,
          [limit],
        );
      } else {
        try {
          rows = await d.select<SessionSql[]>(
            `SELECT ${LIST_COLUMNS},
               COALESCE(
                 (SELECT snippet(transcript_fts, 0, '', '', '…', 16) FROM transcript_fts f JOIN transcript t ON t.rowid = f.rowid
                   WHERE transcript_fts MATCH $1 AND t.session_id = s.id LIMIT 1),
                 (SELECT snippet(cards_fts, -1, '', '', '…', 16) FROM cards_fts g JOIN cards c ON c.rowid = g.rowid
                   WHERE cards_fts MATCH $1 AND c.session_id = s.id LIMIT 1)
               ) AS snippet
             FROM sessions s
             WHERE s.id IN (
               SELECT t.session_id FROM transcript_fts f JOIN transcript t ON t.rowid = f.rowid WHERE transcript_fts MATCH $1
               UNION
               SELECT c.session_id FROM cards_fts g JOIN cards c ON c.rowid = g.rowid WHERE cards_fts MATCH $1
             ) OR s.title LIKE $2
             ORDER BY s.started_at DESC LIMIT $3`,
            [fts, `%${opts.query}%`, limit],
          );
        } catch {
          // FTS unavailable or the query was rejected: fall back to LIKE.
          const like = `%${opts.query}%`;
          rows = await d.select<SessionSql[]>(
            `SELECT ${LIST_COLUMNS} FROM sessions s
             WHERE s.title LIKE $1
                OR s.id IN (SELECT session_id FROM transcript WHERE text LIKE $1)
                OR s.id IN (SELECT session_id FROM cards WHERE question LIKE $1 OR content LIKE $1)
             ORDER BY s.started_at DESC LIMIT $2`,
            [like, limit],
          );
        }
      }
      return rows.map(
        (r): SessionListItem => ({
          ...toSession(r),
          cardCount: Number(r.card_count ?? 0),
          snippet: r.snippet ?? undefined,
        }),
      );
    },

    async getSessionDetail(id) {
      const d = await db();
      const s = await d.select<SessionSql[]>(`SELECT * FROM sessions WHERE id = $1`, [id]);
      if (!s[0]) return null;
      const transcript = await d.select<
        { id: string; session_id: string; speaker: string; text: string; t0_ms: number; t1_ms: number }[]
      >(`SELECT * FROM transcript WHERE session_id = $1 ORDER BY t0_ms`, [id]);
      const cards = await d.select<
        {
          id: string;
          session_id: string;
          kind: string;
          question: string;
          content: string;
          screenshot_path: string | null;
          status: string;
          created_at: number;
        }[]
      >(`SELECT * FROM cards WHERE session_id = $1 ORDER BY created_at`, [id]);
      const notes = await d.select<{ session_id: string; kind: string; content_json: string; updated_at: number }[]>(
        `SELECT * FROM notes WHERE session_id = $1`,
        [id],
      );
      return {
        session: toSession(s[0]),
        transcript: transcript.map((t) => ({
          id: t.id,
          sessionId: t.session_id,
          speaker: t.speaker as Source,
          text: t.text,
          t0Ms: Number(t.t0_ms),
          t1Ms: Number(t.t1_ms),
        })),
        cards: cards.map(
          (c): CardRow => ({
            id: c.id,
            sessionId: c.session_id,
            kind: c.kind,
            question: c.question,
            content: c.content,
            screenshotPath: c.screenshot_path,
            status: c.status,
            createdAt: Number(c.created_at),
          }),
        ),
        notes: notes.map((n) => ({
          sessionId: n.session_id,
          kind: n.kind,
          contentJson: n.content_json,
          updatedAt: Number(n.updated_at),
        })),
      };
    },

    async deleteSession(id) {
      const d = await db();
      await d.execute(`DELETE FROM transcript WHERE session_id = $1`, [id]);
      await d.execute(`DELETE FROM cards WHERE session_id = $1`, [id]);
      await d.execute(`DELETE FROM notes WHERE session_id = $1`, [id]);
      await d.execute(`DELETE FROM sessions WHERE id = $1`, [id]);
    },

    async deleteAll() {
      const d = await db();
      await d.execute(`DELETE FROM transcript`);
      await d.execute(`DELETE FROM cards`);
      await d.execute(`DELETE FROM notes`);
      await d.execute(`DELETE FROM sessions`);
    },

    async addTranscript(row) {
      await (await db()).execute(
        `INSERT OR REPLACE INTO transcript (id, session_id, speaker, text, t0_ms, t1_ms) VALUES ($1, $2, $3, $4, $5, $6)`,
        [row.id, row.sessionId, row.speaker, row.text, Math.round(row.t0Ms), Math.round(row.t1Ms)],
      );
    },

    async deleteTranscript(id) {
      await (await db()).execute(`DELETE FROM transcript WHERE id = $1`, [id]);
    },

    async addCard(row) {
      await (await db()).execute(
        `INSERT OR REPLACE INTO cards (id, session_id, kind, question, content, screenshot_path, status, created_at)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8)`,
        [row.id, row.sessionId, row.kind, row.question, row.content, row.screenshotPath, row.status, row.createdAt],
      );
    },

    async upsertNotes(row) {
      await (await db()).execute(
        `INSERT INTO notes (session_id, kind, content_json, updated_at) VALUES ($1, $2, $3, $4)
         ON CONFLICT(session_id, kind) DO UPDATE SET content_json = excluded.content_json, updated_at = excluded.updated_at`,
        [row.sessionId, row.kind, row.contentJson, row.updatedAt],
      );
    },

    async listBriefs() {
      const rows = await (await db()).select<BriefSql[]>(`SELECT * FROM briefs ORDER BY updated_at DESC`);
      return rows.map(toBrief);
    },

    async saveBrief(b) {
      await (await db()).execute(
        `INSERT INTO briefs (id, name, mode, content, files_json, created_at, updated_at) VALUES ($1, $2, $3, $4, $5, $6, $7)
         ON CONFLICT(id) DO UPDATE SET name = excluded.name, mode = excluded.mode, content = excluded.content,
           files_json = excluded.files_json, updated_at = excluded.updated_at`,
        [b.id, b.name, b.mode, b.content, b.filesJson, b.createdAt, b.updatedAt],
      );
    },

    async deleteBrief(id) {
      await (await db()).execute(`DELETE FROM briefs WHERE id = $1`, [id]);
    },

    async setDefaultBrief(mode, briefId) {
      const d = await db();
      await d.execute(`UPDATE briefs SET mode = NULL WHERE mode = $1`, [mode]);
      if (briefId) await d.execute(`UPDATE briefs SET mode = $1 WHERE id = $2`, [mode, briefId]);
    },

    async defaultBrief(mode) {
      const rows = await (await db()).select<BriefSql[]>(`SELECT * FROM briefs WHERE mode = $1 LIMIT 1`, [mode]);
      return rows[0] ? toBrief(rows[0]) : null;
    },

    async getModePrompts() {
      const rows = await (await db()).select<{ mode: string; prompt: string }[]>(`SELECT mode, prompt FROM mode_prompts`);
      const out: Partial<Record<Mode, string>> = {};
      for (const r of rows) out[r.mode as Mode] = r.prompt;
      return out;
    },

    async setModePrompt(mode, prompt) {
      await (await db()).execute(
        `INSERT INTO mode_prompts (mode, prompt, updated_at) VALUES ($1, $2, $3)
         ON CONFLICT(mode) DO UPDATE SET prompt = excluded.prompt, updated_at = excluded.updated_at`,
        [mode, prompt, Date.now()],
      );
    },

    async resetModePrompt(mode) {
      await (await db()).execute(`DELETE FROM mode_prompts WHERE mode = $1`, [mode]);
    },
  };
}
