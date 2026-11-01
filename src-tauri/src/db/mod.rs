//! SQLite schema for `sqlite:pluelyx.db` (contract §9). The frontend queries it directly via
//! `@tauri-apps/plugin-sql`; Rust only owns the migrations.

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
}
