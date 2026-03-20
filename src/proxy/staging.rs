//! SQLite-backed staging for proxy capture data.
//!
//! Queries captured by the proxy are written to a `capture_staging` table
//! so they survive process restarts and can be converted to a `.wkl`
//! workload profile after the capture session ends.

use std::path::Path;
use std::sync::Arc;

use anyhow::Result;
use rusqlite::Connection;
use tokio::sync::Mutex;

/// One row in the `capture_staging` table.
#[derive(Debug, Clone)]
pub struct StagingRow {
    pub capture_id: String,
    pub session_id: i64,
    pub user_name: Option<String>,
    pub database_name: Option<String>,
    pub sql: String,
    pub kind: Option<String>,
    pub start_offset_us: i64,
    pub duration_us: i64,
    pub is_error: bool,
    pub error_message: Option<String>,
    pub timestamp_us: i64,
}

/// SQLite-backed staging storage for captured proxy queries.
#[derive(Clone)]
pub struct StagingDb {
    conn: Arc<Mutex<Connection>>,
}

impl StagingDb {
    /// Wrap an existing connection (e.g. the web module's shared DB).
    pub fn new(conn: Arc<Mutex<Connection>>) -> Self {
        Self { conn }
    }

    /// Open a standalone SQLite file and create the staging table.
    pub async fn open_standalone(path: &Path) -> Result<Self> {
        let conn = Connection::open(path)?;
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS capture_staging (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                capture_id TEXT NOT NULL,
                session_id INTEGER NOT NULL,
                user_name TEXT,
                database_name TEXT,
                sql TEXT NOT NULL,
                kind TEXT,
                start_offset_us INTEGER NOT NULL,
                duration_us INTEGER NOT NULL,
                is_error INTEGER NOT NULL DEFAULT 0,
                error_message TEXT,
                timestamp_us INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_staging_capture ON capture_staging(capture_id);
            ",
        )?;
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    /// Insert a batch of rows inside a single transaction.
    pub async fn insert_batch(&self, rows: &[StagingRow]) -> Result<()> {
        let conn = self.conn.lock().await;
        let tx = conn.unchecked_transaction()?;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO capture_staging
                 (capture_id, session_id, user_name, database_name, sql, kind,
                  start_offset_us, duration_us, is_error, error_message, timestamp_us)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for r in rows {
                stmt.execute(rusqlite::params![
                    r.capture_id,
                    r.session_id,
                    r.user_name,
                    r.database_name,
                    r.sql,
                    r.kind,
                    r.start_offset_us,
                    r.duration_us,
                    r.is_error as i32,
                    r.error_message,
                    r.timestamp_us,
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// Read all rows for a given capture, ordered by timestamp.
    pub async fn read_capture(&self, capture_id: &str) -> Result<Vec<StagingRow>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT capture_id, session_id, user_name, database_name, sql, kind,
                    start_offset_us, duration_us, is_error, error_message, timestamp_us
             FROM capture_staging
             WHERE capture_id = ?1
             ORDER BY timestamp_us",
        )?;
        let rows = stmt
            .query_map([capture_id], |row| {
                let is_error_int: i32 = row.get(8)?;
                Ok(StagingRow {
                    capture_id: row.get(0)?,
                    session_id: row.get(1)?,
                    user_name: row.get(2)?,
                    database_name: row.get(3)?,
                    sql: row.get(4)?,
                    kind: row.get(5)?,
                    start_offset_us: row.get(6)?,
                    duration_us: row.get(7)?,
                    is_error: is_error_int != 0,
                    error_message: row.get(9)?,
                    timestamp_us: row.get(10)?,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Delete all rows for a capture. Returns the number of rows deleted.
    pub async fn clear_capture(&self, capture_id: &str) -> Result<usize> {
        let conn = self.conn.lock().await;
        let count = conn.execute(
            "DELETE FROM capture_staging WHERE capture_id = ?1",
            [capture_id],
        )?;
        Ok(count)
    }

    /// List all capture IDs with their row counts (useful for finding orphaned data).
    pub async fn list_orphaned_captures(&self) -> Result<Vec<(String, i64)>> {
        let conn = self.conn.lock().await;
        let mut stmt = conn.prepare(
            "SELECT capture_id, COUNT(*) as cnt
             FROM capture_staging
             GROUP BY capture_id
             ORDER BY capture_id",
        )?;
        let rows = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                let count: i64 = row.get(1)?;
                Ok((id, count))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_db() -> StagingDb {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "
            CREATE TABLE IF NOT EXISTS capture_staging (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                capture_id TEXT NOT NULL,
                session_id INTEGER NOT NULL,
                user_name TEXT,
                database_name TEXT,
                sql TEXT NOT NULL,
                kind TEXT,
                start_offset_us INTEGER NOT NULL,
                duration_us INTEGER NOT NULL,
                is_error INTEGER NOT NULL DEFAULT 0,
                error_message TEXT,
                timestamp_us INTEGER NOT NULL
            );

            CREATE INDEX IF NOT EXISTS idx_staging_capture ON capture_staging(capture_id);
            ",
        )
        .unwrap();
        StagingDb::new(Arc::new(Mutex::new(conn)))
    }

    fn make_row(capture_id: &str, session_id: i64, sql: &str, timestamp_us: i64) -> StagingRow {
        StagingRow {
            capture_id: capture_id.to_string(),
            session_id,
            user_name: Some("app".to_string()),
            database_name: Some("testdb".to_string()),
            sql: sql.to_string(),
            kind: Some("Select".to_string()),
            start_offset_us: 0,
            duration_us: 100,
            is_error: false,
            error_message: None,
            timestamp_us,
        }
    }

    #[tokio::test]
    async fn test_insert_and_read_batch() {
        let db = test_db().await;
        let rows = vec![
            make_row("cap-1", 1, "SELECT 1", 1000),
            make_row("cap-1", 1, "SELECT 2", 2000),
            make_row("cap-1", 2, "SELECT 3", 1500),
        ];

        db.insert_batch(&rows).await.unwrap();

        let read = db.read_capture("cap-1").await.unwrap();
        assert_eq!(read.len(), 3);
        // Should be ordered by timestamp_us
        assert_eq!(read[0].sql, "SELECT 1");
        assert_eq!(read[1].sql, "SELECT 3");
        assert_eq!(read[2].sql, "SELECT 2");
        assert_eq!(read[0].session_id, 1);
        assert_eq!(read[1].session_id, 2);

        // Different capture_id returns empty
        let other = db.read_capture("cap-999").await.unwrap();
        assert!(other.is_empty());
    }

    #[tokio::test]
    async fn test_clear_capture() {
        let db = test_db().await;
        let rows = vec![
            make_row("cap-1", 1, "SELECT 1", 1000),
            make_row("cap-1", 1, "SELECT 2", 2000),
            make_row("cap-2", 1, "SELECT 3", 3000),
        ];
        db.insert_batch(&rows).await.unwrap();

        let deleted = db.clear_capture("cap-1").await.unwrap();
        assert_eq!(deleted, 2);

        let remaining = db.read_capture("cap-1").await.unwrap();
        assert!(remaining.is_empty());

        // cap-2 should be untouched
        let cap2 = db.read_capture("cap-2").await.unwrap();
        assert_eq!(cap2.len(), 1);
    }

    #[tokio::test]
    async fn test_list_orphaned_captures() {
        let db = test_db().await;
        let rows = vec![
            make_row("cap-a", 1, "SELECT 1", 1000),
            make_row("cap-a", 1, "SELECT 2", 2000),
            make_row("cap-b", 1, "SELECT 3", 3000),
        ];
        db.insert_batch(&rows).await.unwrap();

        let orphans = db.list_orphaned_captures().await.unwrap();
        assert_eq!(orphans.len(), 2);
        assert_eq!(orphans[0], ("cap-a".to_string(), 2));
        assert_eq!(orphans[1], ("cap-b".to_string(), 1));
    }

    #[tokio::test]
    async fn test_error_rows() {
        let db = test_db().await;
        let rows = vec![StagingRow {
            capture_id: "cap-err".to_string(),
            session_id: 1,
            user_name: Some("app".to_string()),
            database_name: Some("testdb".to_string()),
            sql: "INSERT INTO bad_table VALUES (1)".to_string(),
            kind: Some("Insert".to_string()),
            start_offset_us: 0,
            duration_us: 50,
            is_error: true,
            error_message: Some("relation \"bad_table\" does not exist".to_string()),
            timestamp_us: 5000,
        }];

        db.insert_batch(&rows).await.unwrap();

        let read = db.read_capture("cap-err").await.unwrap();
        assert_eq!(read.len(), 1);
        assert!(read[0].is_error);
        assert_eq!(
            read[0].error_message.as_deref(),
            Some("relation \"bad_table\" does not exist")
        );
        assert_eq!(read[0].kind.as_deref(), Some("Insert"));
    }
}
