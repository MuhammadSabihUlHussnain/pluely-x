//! SQLite schema for `sqlite:pluelyx.db` (contract §9). The frontend queries it directly via
//! `@tauri-apps/plugin-sql`; Rust only owns the migrations.

use std::path::Path;
use std::sync::OnceLock;

use sha2::{Digest, Sha384};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{ConnectOptions, Connection};
use tauri::plugin::{Builder as PluginBuilder, TauriPlugin};
use tauri::{Manager, Runtime};
use tauri_plugin_sql::{Migration, MigrationKind};

pub const DB_URL: &str = "sqlite:pluelyx.db";
pub const MIGRATION_V1: &str = include_str!("migrations/v1.sql");

pub fn migrations() -> Vec<Migration> {
    vec![Migration {
        version: 1,
        description: "pluelyx_schema_v1",
        sql: MIGRATION_V1,
        kind: MigrationKind::Up,
    }]
}

// ---- line-ending checksum repair --------------------------------------------------------
//
// sqlx stores a SHA-384 of each applied migration's exact text and refuses to start if it
// changes. `v1.sql` comes in through include_str!, so its line endings depend on how the repo
// was checked out (git core.autocrlf turns LF into CRLF on Windows). A database created by a
// CRLF build then fails in an LF build, and the other way round, with "migration 1 was
// previously applied but has been modified": a panic before any window opens. The schema is
// identical, so before the SQL plugin runs we rewrite a stored checksum that differs from this
// build's *only* by line endings. A migration whose content really changed is left alone and
// still fails loudly.

/// Set when a repair ran, so `setup` can log it once tracing is up.
pub static REPAIR_NOTE: OnceLock<String> = OnceLock::new();

fn sha384(bytes: &[u8]) -> Vec<u8> {
    Sha384::digest(bytes).to_vec()
}

/// Checksums of the same SQL with LF and with CRLF line endings.
fn line_ending_variants(sql: &str) -> [Vec<u8>; 2] {
    let lf = sql.replace("\r\n", "\n");
    let crlf = lf.replace('\n', "\r\n");
    [sha384(lf.as_bytes()), sha384(crlf.as_bytes())]
}

/// Rewrites line-ending-only checksum mismatches in an existing database and returns how many
/// were fixed. Never creates the database.
pub async fn repair_checksums(db_path: &Path, migrations: &[Migration]) -> Result<u32, sqlx::Error> {
    if !db_path.exists() {
        return Ok(0);
    }
    let mut conn = SqliteConnectOptions::new()
        .filename(db_path)
        .create_if_missing(false)
        .connect()
        .await?;
    let has_table: Option<(String,)> = sqlx::query_as(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name = '_sqlx_migrations'",
    )
    .fetch_optional(&mut conn)
    .await?;
    let mut fixed = 0;
    if has_table.is_some() {
        for m in migrations {
            let stored: Option<(Vec<u8>,)> =
                sqlx::query_as("SELECT checksum FROM _sqlx_migrations WHERE version = ?")
                    .bind(m.version)
                    .fetch_optional(&mut conn)
                    .await?;
            let Some((stored,)) = stored else { continue };
            let current = sha384(m.sql.as_bytes());
            if stored != current && line_ending_variants(m.sql).contains(&stored) {
                sqlx::query("UPDATE _sqlx_migrations SET checksum = ? WHERE version = ?")
                    .bind(&current)
                    .bind(m.version)
                    .execute(&mut conn)
                    .await?;
                fixed += 1;
            }
        }
    }
    conn.close().await?;
    Ok(fixed)
}

/// Runs the repair. Register it *before* tauri-plugin-sql: plugins set up in registration
/// order, and the SQL plugin applies and verifies migrations in its own setup.
pub fn repair_plugin<R: Runtime>() -> TauriPlugin<R> {
    PluginBuilder::new("pluelyx-db-repair")
        .setup(|app, _api| {
            // tauri-plugin-sql resolves "sqlite:<file>" against the app config dir.
            let Ok(dir) = app.path().app_config_dir() else {
                return Ok(());
            };
            let path = dir.join(DB_URL.trim_start_matches("sqlite:"));
            let note = match tauri::async_runtime::block_on(repair_checksums(&path, &migrations())) {
                Ok(0) => None,
                Ok(n) => Some(format!(
                    "db: repaired {n} migration checksum(s) that differed only by line endings"
                )),
                Err(e) => Some(format!("db: checksum repair skipped: {e}")),
            };
            if let Some(n) = note {
                let _ = REPAIR_NOTE.set(n);
            }
            Ok(())
        })
        .build()
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqliteConnectOptions;
    use sqlx::{ConnectOptions, Row, SqliteConnection};
    use std::str::FromStr;

    async fn db() -> SqliteConnection {
        let mut conn = SqliteConnectOptions::from_str("sqlite::memory:")
            .unwrap()
            .foreign_keys(true)
            .connect()
            .await
            .unwrap();
        sqlx::raw_sql(MIGRATION_V1)
            .execute(&mut conn)
            .await
            .unwrap();
        conn
    }

    async fn search_transcript(conn: &mut SqliteConnection, q: &str) -> Vec<String> {
        sqlx::query(
            "SELECT t.id FROM transcript_fts f JOIN transcript t ON t.rowid = f.rowid \
             WHERE transcript_fts MATCH ? ORDER BY t.t0_ms",
        )
        .bind(q)
        .fetch_all(conn)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>(0))
        .collect()
    }

    async fn search_cards(conn: &mut SqliteConnection, q: &str) -> Vec<String> {
        sqlx::query("SELECT c.id FROM cards_fts f JOIN cards c ON c.rowid = f.rowid WHERE cards_fts MATCH ?")
            .bind(q)
            .fetch_all(conn)
            .await
            .unwrap()
            .iter()
            .map(|r| r.get::<String, _>(0))
            .collect()
    }

    #[tokio::test]
    async fn migration_v1_fts_triggers_and_cascade() {
        let mut c = db().await;
        sqlx::raw_sql(
            "INSERT INTO sessions(id, mode, started_at) VALUES ('s1', 'interview', 1);
             INSERT INTO sessions(id, mode, started_at) VALUES ('s2', 'lecture', 2);
             INSERT INTO transcript VALUES ('t1', 's1', 'them', 'How would you design a hash map?', 0, 2000);
             INSERT INTO transcript VALUES ('t2', 's1', 'me', 'I would use open addressing', 2500, 4000);
             INSERT INTO transcript VALUES ('t3', 's2', 'them', 'Today: eigenvalues', 0, 900);
             INSERT INTO cards(id, session_id, kind, question, content, created_at)
               VALUES ('c1', 's1', 'answer', 'design a hash map', 'Use buckets and resize at 0.75', 3);",
        )
        .execute(&mut c)
        .await
        .unwrap();

        assert_eq!(search_transcript(&mut c, "hash").await, vec!["t1"]);
        assert_eq!(search_transcript(&mut c, "addressing").await, vec!["t2"]);
        assert_eq!(search_cards(&mut c, "buckets").await, vec!["c1"]);
        assert_eq!(search_cards(&mut c, "question:hash").await, vec!["c1"]);

        // update keeps the index in sync
        sqlx::query(
            "UPDATE transcript SET text = 'How would you design an LRU cache?' WHERE id = 't1'",
        )
        .execute(&mut c)
        .await
        .unwrap();
        assert!(search_transcript(&mut c, "hash").await.is_empty());
        assert_eq!(search_transcript(&mut c, "LRU").await, vec!["t1"]);

        // deleting a session cascades to rows and their FTS entries
        sqlx::query("DELETE FROM sessions WHERE id = 's1'")
            .execute(&mut c)
            .await
            .unwrap();
        let left: i64 = sqlx::query("SELECT count(*) FROM transcript")
            .fetch_one(&mut c)
            .await
            .unwrap()
            .get(0);
        assert_eq!(left, 1);
        assert!(search_transcript(&mut c, "LRU").await.is_empty());
        assert!(search_cards(&mut c, "buckets").await.is_empty());
        assert_eq!(search_transcript(&mut c, "eigenvalues").await, vec!["t3"]);

        // speaker is constrained
        let bad = sqlx::query("INSERT INTO transcript VALUES ('t9', 's2', 'robot', 'x', 0, 1)")
            .execute(&mut c)
            .await;
        assert!(bad.is_err());
    }

    #[test]
    fn single_v1_migration() {
        let m = migrations();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].version, 1);
    }

    async fn file_db_with_checksum(checksum: Vec<u8>) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("pluelyx.db");
        let mut conn = SqliteConnectOptions::new()
            .filename(&path)
            .create_if_missing(true)
            .connect()
            .await
            .unwrap();
        sqlx::raw_sql(
            "CREATE TABLE _sqlx_migrations (version BIGINT PRIMARY KEY, description TEXT NOT NULL, \
             installed_on TIMESTAMP NOT NULL DEFAULT CURRENT_TIMESTAMP, success BOOLEAN NOT NULL, \
             checksum BLOB NOT NULL, execution_time BIGINT NOT NULL)",
        )
        .execute(&mut conn)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time) \
             VALUES (1, 'pluelyx_schema_v1', 1, ?, 0)",
        )
        .bind(checksum)
        .execute(&mut conn)
        .await
        .unwrap();
        conn.close().await.unwrap();
        (dir, path)
    }

    async fn stored_checksum(path: &std::path::Path) -> Vec<u8> {
        let mut conn = SqliteConnectOptions::new().filename(path).connect().await.unwrap();
        let (c,): (Vec<u8>,) = sqlx::query_as("SELECT checksum FROM _sqlx_migrations WHERE version = 1")
            .fetch_one(&mut conn)
            .await
            .unwrap();
        c
    }

    #[tokio::test]
    async fn repairs_a_checksum_from_the_other_line_ending() {
        let current = sha384(MIGRATION_V1.as_bytes());
        let other = line_ending_variants(MIGRATION_V1)
            .into_iter()
            .find(|c| *c != current)
            .unwrap();
        let (_dir, path) = file_db_with_checksum(other).await;
        assert_eq!(repair_checksums(&path, &migrations()).await.unwrap(), 1);
        assert_eq!(stored_checksum(&path).await, current);
        assert_eq!(repair_checksums(&path, &migrations()).await.unwrap(), 0); // idempotent
    }

    #[tokio::test]
    async fn leaves_a_really_modified_migration_alone() {
        let edited = sha384(format!("{MIGRATION_V1}\n-- changed").as_bytes());
        let (_dir, path) = file_db_with_checksum(edited.clone()).await;
        assert_eq!(repair_checksums(&path, &migrations()).await.unwrap(), 0);
        assert_eq!(stored_checksum(&path).await, edited);
    }

    #[tokio::test]
    async fn never_creates_a_missing_database() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("absent.db");
        assert_eq!(repair_checksums(&path, &migrations()).await.unwrap(), 0);
        assert!(!path.exists());
    }

    #[test]
    fn matches_the_checksum_a_crlf_build_stored() {
        // The exact value found in a real database created by a CRLF checkout (2026-11-01).
        let crlf = line_ending_variants(MIGRATION_V1)[1].clone();
        let hex: String = crlf.iter().take(12).map(|b| format!("{b:02X}")).collect();
        assert_eq!(hex, "5F79D72FB105F3AB5E078D18");
    }
}
