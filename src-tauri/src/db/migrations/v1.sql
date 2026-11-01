-- Pluely X schema v1 (docs/CONTRACT.md §9). Screenshots are files under <appData>/screenshots,
-- never base64 in rows. The FTS tables are external-content indexes kept in sync by triggers.
-- NOTE: transcript/cards rowids are implicit, so after a VACUUM rebuild the indexes with
--   INSERT INTO transcript_fts(transcript_fts) VALUES('rebuild');
--   INSERT INTO cards_fts(cards_fts) VALUES('rebuild');

CREATE TABLE sessions (
  id TEXT PRIMARY KEY,
  mode TEXT NOT NULL,
  title TEXT NOT NULL DEFAULT '',
  started_at INTEGER NOT NULL,
  ended_at INTEGER,
  brief_id TEXT,
  summary_json TEXT
);
CREATE INDEX idx_sessions_started ON sessions(started_at DESC);

CREATE TABLE transcript (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  speaker TEXT NOT NULL CHECK (speaker IN ('me','them')),
  text TEXT NOT NULL,
  t0_ms INTEGER NOT NULL,
  t1_ms INTEGER NOT NULL
);
CREATE INDEX idx_transcript_session ON transcript(session_id, t0_ms);

CREATE TABLE cards (
  id TEXT PRIMARY KEY,
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  question TEXT NOT NULL DEFAULT '',
  content TEXT NOT NULL DEFAULT '',
  screenshot_path TEXT,
  status TEXT NOT NULL DEFAULT 'done',
  created_at INTEGER NOT NULL
);
CREATE INDEX idx_cards_session ON cards(session_id, created_at);

CREATE TABLE notes (
  session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
  kind TEXT NOT NULL,
  content_json TEXT NOT NULL,
  updated_at INTEGER NOT NULL,
  PRIMARY KEY (session_id, kind)
);

CREATE TABLE briefs (
  id TEXT PRIMARY KEY,
  name TEXT NOT NULL,
  mode TEXT,
  content TEXT NOT NULL DEFAULT '',
  files_json TEXT NOT NULL DEFAULT '[]',
  created_at INTEGER NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE TABLE mode_prompts (
  mode TEXT PRIMARY KEY,
  prompt TEXT NOT NULL,
  updated_at INTEGER NOT NULL
);

CREATE VIRTUAL TABLE transcript_fts USING fts5(text, content='transcript', content_rowid='rowid');
CREATE VIRTUAL TABLE cards_fts USING fts5(question, content, content='cards', content_rowid='rowid');

CREATE TRIGGER transcript_ai AFTER INSERT ON transcript BEGIN
  INSERT INTO transcript_fts(rowid, text) VALUES (new.rowid, new.text);
END;
CREATE TRIGGER transcript_ad AFTER DELETE ON transcript BEGIN
  INSERT INTO transcript_fts(transcript_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
END;
CREATE TRIGGER transcript_au AFTER UPDATE ON transcript BEGIN
  INSERT INTO transcript_fts(transcript_fts, rowid, text) VALUES ('delete', old.rowid, old.text);
  INSERT INTO transcript_fts(rowid, text) VALUES (new.rowid, new.text);
END;

CREATE TRIGGER cards_ai AFTER INSERT ON cards BEGIN
  INSERT INTO cards_fts(rowid, question, content) VALUES (new.rowid, new.question, new.content);
END;
CREATE TRIGGER cards_ad AFTER DELETE ON cards BEGIN
  INSERT INTO cards_fts(cards_fts, rowid, question, content) VALUES ('delete', old.rowid, old.question, old.content);
END;
CREATE TRIGGER cards_au AFTER UPDATE ON cards BEGIN
  INSERT INTO cards_fts(cards_fts, rowid, question, content) VALUES ('delete', old.rowid, old.question, old.content);
  INSERT INTO cards_fts(rowid, question, content) VALUES (new.rowid, new.question, new.content);
END;
