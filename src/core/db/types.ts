// Row shapes and the repository interface over the schema in CONTRACT.md §9.
// `sqlRepo` implements it with @tauri-apps/plugin-sql; `memoryRepo` implements
// it in memory for browser/mock mode and tests.

import type { Mode, Source } from "../types";

export interface SessionRow {
  id: string;
  mode: Mode;
  title: string;
  startedAt: number;
  endedAt: number | null;
  briefId: string | null;
  summaryJson: string | null;
}

export interface SessionListItem extends SessionRow {
  cardCount: number;
  /** Matching text for a search hit. */
  snippet?: string;
}

export interface TranscriptRow {
  id: string;
  sessionId: string;
  speaker: Source;
  text: string;
  t0Ms: number;
  t1Ms: number;
}

export interface CardRow {
  id: string;
  sessionId: string;
  kind: string;
  question: string;
  content: string;
  screenshotPath: string | null;
  status: string;
  createdAt: number;
}

export interface NotesRow {
  sessionId: string;
  kind: string;
  contentJson: string;
  updatedAt: number;
}

export interface BriefRow {
  id: string;
  name: string;
  /** The mode this brief is the default for (one default per mode), or null. */
  mode: Mode | null;
  content: string;
  filesJson: string;
  createdAt: number;
  updatedAt: number;
}

export interface BriefFile {
  name: string;
  size: number;
  chars: number;
}

export interface SessionDetail {
  session: SessionRow;
  transcript: TranscriptRow[];
  cards: CardRow[];
  notes: NotesRow[];
}

export interface Repo {
  createSession(s: { id: string; mode: Mode; title?: string; startedAt: number; briefId?: string | null }): Promise<void>;
  updateSession(
    id: string,
    patch: Partial<Pick<SessionRow, "title" | "endedAt" | "summaryJson" | "mode" | "briefId">>,
  ): Promise<void>;
  getSession(id: string): Promise<SessionRow | null>;
  listSessions(opts?: { query?: string; limit?: number }): Promise<SessionListItem[]>;
  getSessionDetail(id: string): Promise<SessionDetail | null>;
  deleteSession(id: string): Promise<void>;
  deleteAll(): Promise<void>;

  addTranscript(row: TranscriptRow): Promise<void>;
  deleteTranscript(id: string): Promise<void>;
  addCard(row: CardRow): Promise<void>;
  upsertNotes(row: NotesRow): Promise<void>;

  listBriefs(): Promise<BriefRow[]>;
  saveBrief(b: BriefRow): Promise<void>;
  deleteBrief(id: string): Promise<void>;
  setDefaultBrief(mode: Mode, briefId: string | null): Promise<void>;
  defaultBrief(mode: Mode): Promise<BriefRow | null>;

  getModePrompts(): Promise<Partial<Record<Mode, string>>>;
  setModePrompt(mode: Mode, prompt: string): Promise<void>;
  resetModePrompt(mode: Mode): Promise<void>;
}

/** Turns free text into a safe FTS5 query: every word must match, as a prefix. */
export function toFtsQuery(q: string): string | null {
  const words = q.match(/[\p{L}\p{N}_]+/gu);
  if (!words?.length) return null;
  return words.map((w) => `"${w.replace(/"/g, "")}"*`).join(" ");
}

/** Transcript row ids are unique across sessions. */
export function transcriptRowId(sessionId: string, segId: string): string {
  return `${sessionId}:${segId}`;
}
