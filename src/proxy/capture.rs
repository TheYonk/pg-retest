use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use chrono::Utc;
use tokio::sync::mpsc;
use tracing::debug;

use crate::capture::masking::mask_sql_literals;
use crate::profile::{self, Metadata, Query, QueryKind, Session, WorkloadProfile};

use super::staging::{StagingDb, StagingRow};

/// Events sent from relay tasks to the capture collector.
#[derive(Debug, Clone)]
pub enum CaptureEvent {
    SessionStart {
        session_id: u64,
        user: String,
        database: String,
        timestamp: Instant,
    },
    QueryStart {
        session_id: u64,
        sql: String,
        timestamp: Instant,
    },
    QueryComplete {
        session_id: u64,
        timestamp: Instant,
    },
    QueryError {
        session_id: u64,
        message: String,
        timestamp: Instant,
    },
    SessionEnd {
        session_id: u64,
    },
}

/// Per-session state tracked by the collector.
struct SessionState {
    user: String,
    database: String,
    session_start: Instant,
    queries: Vec<CapturedQuery>,
    pending_sql: Option<(String, Instant)>,
}

pub(crate) struct CapturedQuery {
    sql: String,
    start_offset_us: u64,
    duration_us: u64,
    is_error: bool,
    error_message: Option<String>,
}

/// Runs the capture collector loop, consuming events until the channel closes.
/// Returns the captured sessions (to be built into a WorkloadProfile).
pub(crate) async fn run_collector(
    mut rx: mpsc::UnboundedReceiver<CaptureEvent>,
) -> Vec<(u64, String, String, Vec<CapturedQuery>)> {
    let mut sessions: HashMap<u64, SessionState> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            CaptureEvent::SessionStart {
                session_id,
                user,
                database,
                timestamp,
            } => {
                sessions.insert(
                    session_id,
                    SessionState {
                        user,
                        database,
                        session_start: timestamp,
                        queries: Vec::new(),
                        pending_sql: None,
                    },
                );
                debug!("Capture: session {session_id} started");
            }
            CaptureEvent::QueryStart {
                session_id,
                sql,
                timestamp,
            } => {
                if let Some(state) = sessions.get_mut(&session_id) {
                    state.pending_sql = Some((sql, timestamp));
                }
            }
            CaptureEvent::QueryComplete {
                session_id,
                timestamp,
            } => {
                if let Some(state) = sessions.get_mut(&session_id) {
                    if let Some((sql, start)) = state.pending_sql.take() {
                        let offset = start.duration_since(state.session_start);
                        let duration = timestamp.duration_since(start);
                        state.queries.push(CapturedQuery {
                            sql,
                            start_offset_us: offset.as_micros() as u64,
                            duration_us: duration.as_micros() as u64,
                            is_error: false,
                            error_message: None,
                        });
                    }
                }
            }
            CaptureEvent::QueryError {
                session_id,
                message,
                timestamp,
            } => {
                if let Some(state) = sessions.get_mut(&session_id) {
                    if let Some((sql, start)) = state.pending_sql.take() {
                        let offset = start.duration_since(state.session_start);
                        let duration = timestamp.duration_since(start);
                        debug!("Capture: query error in session {session_id}: {message}");
                        state.queries.push(CapturedQuery {
                            sql,
                            start_offset_us: offset.as_micros() as u64,
                            duration_us: duration.as_micros() as u64,
                            is_error: true,
                            error_message: Some(message.clone()),
                        });
                    }
                }
            }
            CaptureEvent::SessionEnd { session_id } => {
                debug!("Capture: session {session_id} ended");
            }
        }
    }

    sessions
        .into_iter()
        .map(|(id, state)| (id, state.user, state.database, state.queries))
        .collect()
}

/// Build a WorkloadProfile from captured session data.
pub(crate) fn build_profile(
    captured: Vec<(u64, String, String, Vec<CapturedQuery>)>,
    source_host: &str,
    mask_values: bool,
) -> WorkloadProfile {
    let mut sessions = Vec::new();
    let mut total_queries: u64 = 0;
    let mut next_txn_id: u64 = 1;
    let mut capture_duration_us: u64 = 0;

    for (session_id, user, database, raw_queries) in captured {
        let mut queries: Vec<Query> = raw_queries
            .into_iter()
            .map(|cq| {
                let sql = if mask_values {
                    mask_sql_literals(&cq.sql)
                } else {
                    cq.sql
                };
                Query {
                    kind: QueryKind::from_sql(&sql),
                    sql,
                    start_offset_us: cq.start_offset_us,
                    duration_us: cq.duration_us,
                    transaction_id: None,
                }
            })
            .collect();

        profile::assign_transaction_ids(&mut queries, &mut next_txn_id);

        if let Some(last) = queries.last() {
            let end = last.start_offset_us + last.duration_us;
            if end > capture_duration_us {
                capture_duration_us = end;
            }
        }

        total_queries += queries.len() as u64;

        sessions.push(Session {
            id: session_id,
            user,
            database,
            queries,
        });
    }

    sessions.sort_by_key(|s| s.id);

    let total_sessions = sessions.len() as u64;

    WorkloadProfile {
        version: 2,
        captured_at: Utc::now(),
        source_host: source_host.to_string(),
        pg_version: "unknown".to_string(),
        capture_method: "proxy".to_string(),
        sessions,
        metadata: Metadata {
            total_queries,
            total_sessions,
            capture_duration_us,
        },
    }
}

/// Runs the staging collector loop, consuming capture events and batching
/// inserts to the SQLite staging database.
///
/// Events are buffered and flushed every 100 rows or every 500ms, whichever
/// comes first. On channel close, any remaining batch is flushed.
pub(crate) async fn run_staging_collector(
    mut rx: mpsc::UnboundedReceiver<CaptureEvent>,
    db: StagingDb,
    capture_id: String,
) {
    // session_id → (user, database, start_instant)
    let mut session_meta: HashMap<u64, (String, String, Instant)> = HashMap::new();
    // session_id → (sql, query_start_instant)
    let mut pending: HashMap<u64, (String, Instant)> = HashMap::new();
    let mut batch: Vec<StagingRow> = Vec::new();

    let flush_interval = std::time::Duration::from_millis(500);
    const BATCH_SIZE: usize = 100;

    loop {
        let event = match tokio::time::timeout(flush_interval, rx.recv()).await {
            Ok(Some(ev)) => Some(ev),
            Ok(None) => {
                // Channel closed — flush remaining and exit
                if !batch.is_empty() {
                    if let Err(e) = db.insert_batch(&batch).await {
                        debug!("Staging flush error on shutdown: {e}");
                    }
                }
                break;
            }
            Err(_) => {
                // Timeout — periodic flush
                if !batch.is_empty() {
                    if let Err(e) = db.insert_batch(&batch).await {
                        debug!("Staging periodic flush error: {e}");
                    }
                    batch.clear();
                }
                continue;
            }
        };

        if let Some(event) = event {
            match event {
                CaptureEvent::SessionStart {
                    session_id,
                    user,
                    database,
                    timestamp,
                } => {
                    session_meta.insert(session_id, (user, database, timestamp));
                    debug!("Staging: session {session_id} started");
                }
                CaptureEvent::QueryStart {
                    session_id,
                    sql,
                    timestamp,
                } => {
                    pending.insert(session_id, (sql, timestamp));
                }
                CaptureEvent::QueryComplete {
                    session_id,
                    timestamp,
                } => {
                    if let Some((sql, start)) = pending.remove(&session_id) {
                        if let Some((user, database, session_start)) = session_meta.get(&session_id)
                        {
                            let offset = start
                                .checked_duration_since(*session_start)
                                .unwrap_or_default();
                            let duration =
                                timestamp.checked_duration_since(start).unwrap_or_default();
                            batch.push(StagingRow {
                                capture_id: capture_id.clone(),
                                session_id: session_id as i64,
                                user_name: Some(user.clone()),
                                database_name: Some(database.clone()),
                                sql: sql.clone(),
                                kind: Some(format!("{:?}", QueryKind::from_sql(&sql))),
                                start_offset_us: offset.as_micros() as i64,
                                duration_us: duration.as_micros() as i64,
                                is_error: false,
                                error_message: None,
                                timestamp_us: offset.as_micros() as i64,
                            });
                        }
                    }
                }
                CaptureEvent::QueryError {
                    session_id,
                    message,
                    timestamp,
                } => {
                    if let Some((sql, start)) = pending.remove(&session_id) {
                        if let Some((user, database, session_start)) = session_meta.get(&session_id)
                        {
                            let offset = start
                                .checked_duration_since(*session_start)
                                .unwrap_or_default();
                            let duration =
                                timestamp.checked_duration_since(start).unwrap_or_default();
                            debug!("Staging: query error in session {session_id}: {message}");
                            batch.push(StagingRow {
                                capture_id: capture_id.clone(),
                                session_id: session_id as i64,
                                user_name: Some(user.clone()),
                                database_name: Some(database.clone()),
                                sql: sql.clone(),
                                kind: Some(format!("{:?}", QueryKind::from_sql(&sql))),
                                start_offset_us: offset.as_micros() as i64,
                                duration_us: duration.as_micros() as i64,
                                is_error: true,
                                error_message: Some(message),
                                timestamp_us: offset.as_micros() as i64,
                            });
                        }
                    }
                }
                CaptureEvent::SessionEnd { session_id } => {
                    debug!("Staging: session {session_id} ended");
                }
            }

            if batch.len() >= BATCH_SIZE {
                if let Err(e) = db.insert_batch(&batch).await {
                    debug!("Staging batch flush error: {e}");
                }
                batch.clear();
            }
        }
    }
}

/// Per-session staging data: (user, database, rows).
type SessionStagingData = (Option<String>, Option<String>, Vec<StagingRow>);

/// Build a WorkloadProfile from staging rows (read from SQLite).
///
/// Follows the same logic as `build_profile()` but reads from `StagingRow`
/// instead of in-memory `CapturedQuery`.
pub(crate) fn build_profile_from_staging(
    rows: Vec<StagingRow>,
    source_host: &str,
    mask_values: bool,
) -> WorkloadProfile {
    // Group rows by session_id, preserving order within each session
    let mut sessions_map: BTreeMap<i64, SessionStagingData> = BTreeMap::new();
    for row in rows {
        let entry = sessions_map
            .entry(row.session_id)
            .or_insert_with(|| (row.user_name.clone(), row.database_name.clone(), Vec::new()));
        entry.2.push(row);
    }

    let mut sessions = Vec::new();
    let mut total_queries: u64 = 0;
    let mut next_txn_id: u64 = 1;
    let mut capture_duration_us: u64 = 0;

    for (session_id, (user, database, staging_rows)) in sessions_map {
        let mut queries: Vec<Query> = staging_rows
            .into_iter()
            .map(|sr| {
                let sql = if mask_values {
                    mask_sql_literals(&sr.sql)
                } else {
                    sr.sql
                };
                Query {
                    kind: QueryKind::from_sql(&sql),
                    sql,
                    start_offset_us: sr.start_offset_us as u64,
                    duration_us: sr.duration_us as u64,
                    transaction_id: None,
                }
            })
            .collect();

        profile::assign_transaction_ids(&mut queries, &mut next_txn_id);

        if let Some(last) = queries.last() {
            let end = last.start_offset_us + last.duration_us;
            if end > capture_duration_us {
                capture_duration_us = end;
            }
        }

        total_queries += queries.len() as u64;

        sessions.push(Session {
            id: session_id as u64,
            user: user.unwrap_or_default(),
            database: database.unwrap_or_default(),
            queries,
        });
    }

    let total_sessions = sessions.len() as u64;

    WorkloadProfile {
        version: 2,
        captured_at: Utc::now(),
        source_host: source_host.to_string(),
        pg_version: "unknown".to_string(),
        capture_method: "proxy".to_string(),
        sessions,
        metadata: Metadata {
            total_queries,
            total_sessions,
            capture_duration_us,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collector_basic_session() {
        let (tx, rx) = mpsc::unbounded_channel();
        let now = Instant::now();

        tx.send(CaptureEvent::SessionStart {
            session_id: 1,
            user: "app".into(),
            database: "mydb".into(),
            timestamp: now,
        })
        .unwrap();

        tx.send(CaptureEvent::QueryStart {
            session_id: 1,
            sql: "SELECT 1".into(),
            timestamp: now + std::time::Duration::from_micros(100),
        })
        .unwrap();

        tx.send(CaptureEvent::QueryComplete {
            session_id: 1,
            timestamp: now + std::time::Duration::from_micros(600),
        })
        .unwrap();

        tx.send(CaptureEvent::SessionEnd { session_id: 1 }).unwrap();

        drop(tx); // Close channel

        let captured = run_collector(rx).await;
        assert_eq!(captured.len(), 1);

        let (id, user, _db, queries) = &captured[0];
        assert_eq!(*id, 1);
        assert_eq!(user, "app");
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].sql, "SELECT 1");
        assert!(queries[0].duration_us >= 400); // 600 - 100 = 500, allow slack
    }

    #[test]
    fn test_build_profile_with_transactions() {
        let captured = vec![(
            1u64,
            "app".to_string(),
            "db".to_string(),
            vec![
                CapturedQuery {
                    sql: "BEGIN".into(),
                    start_offset_us: 0,
                    duration_us: 10,
                    is_error: false,
                    error_message: None,
                },
                CapturedQuery {
                    sql: "INSERT INTO t VALUES (1)".into(),
                    start_offset_us: 100,
                    duration_us: 500,
                    is_error: false,
                    error_message: None,
                },
                CapturedQuery {
                    sql: "COMMIT".into(),
                    start_offset_us: 700,
                    duration_us: 20,
                    is_error: false,
                    error_message: None,
                },
            ],
        )];

        let profile = build_profile(captured, "test-host", false);
        assert_eq!(profile.capture_method, "proxy");
        assert_eq!(profile.sessions.len(), 1);
        assert_eq!(profile.sessions[0].queries.len(), 3);
        assert_eq!(profile.sessions[0].queries[0].kind, QueryKind::Begin);
        assert_eq!(profile.sessions[0].queries[0].transaction_id, Some(1));
        assert_eq!(profile.sessions[0].queries[1].transaction_id, Some(1));
        assert_eq!(profile.sessions[0].queries[2].transaction_id, Some(1));
    }

    #[test]
    fn test_build_profile_with_masking() {
        let captured = vec![(
            1u64,
            "app".to_string(),
            "db".to_string(),
            vec![CapturedQuery {
                sql: "SELECT * FROM users WHERE email = 'alice@corp.com'".into(),
                start_offset_us: 0,
                duration_us: 100,
                is_error: false,
                error_message: None,
            }],
        )];

        let profile = build_profile(captured, "test", true);
        assert!(profile.sessions[0].queries[0].sql.contains("$S"));
        assert!(!profile.sessions[0].queries[0].sql.contains("alice"));
    }

    #[tokio::test]
    async fn test_staging_collector_basic() {
        use rusqlite::Connection;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        // Set up in-memory staging DB
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS capture_staging (
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
            );",
        )
        .unwrap();
        let db = StagingDb::new(Arc::new(Mutex::new(conn)));

        let (tx, rx) = mpsc::unbounded_channel();
        let now = Instant::now();

        tx.send(CaptureEvent::SessionStart {
            session_id: 1,
            user: "app".into(),
            database: "mydb".into(),
            timestamp: now,
        })
        .unwrap();

        tx.send(CaptureEvent::QueryStart {
            session_id: 1,
            sql: "SELECT 1".into(),
            timestamp: now + std::time::Duration::from_micros(100),
        })
        .unwrap();

        tx.send(CaptureEvent::QueryComplete {
            session_id: 1,
            timestamp: now + std::time::Duration::from_micros(600),
        })
        .unwrap();

        tx.send(CaptureEvent::SessionEnd { session_id: 1 }).unwrap();

        drop(tx);

        run_staging_collector(rx, db.clone(), "test-cap".to_string()).await;

        let rows = db.read_capture("test-cap").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].sql, "SELECT 1");
        assert_eq!(rows[0].user_name.as_deref(), Some("app"));
        assert_eq!(rows[0].database_name.as_deref(), Some("mydb"));
        assert!(!rows[0].is_error);
        assert!(rows[0].error_message.is_none());
        assert_eq!(rows[0].kind.as_deref(), Some("Select"));
    }

    #[tokio::test]
    async fn test_staging_collector_error_query() {
        use rusqlite::Connection;
        use std::sync::Arc;
        use tokio::sync::Mutex;

        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS capture_staging (
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
            );",
        )
        .unwrap();
        let db = StagingDb::new(Arc::new(Mutex::new(conn)));

        let (tx, rx) = mpsc::unbounded_channel();
        let now = Instant::now();

        tx.send(CaptureEvent::SessionStart {
            session_id: 1,
            user: "app".into(),
            database: "mydb".into(),
            timestamp: now,
        })
        .unwrap();

        tx.send(CaptureEvent::QueryStart {
            session_id: 1,
            sql: "INSERT INTO bad VALUES (1)".into(),
            timestamp: now + std::time::Duration::from_micros(100),
        })
        .unwrap();

        tx.send(CaptureEvent::QueryError {
            session_id: 1,
            message: "relation does not exist".into(),
            timestamp: now + std::time::Duration::from_micros(300),
        })
        .unwrap();

        drop(tx);

        run_staging_collector(rx, db.clone(), "err-cap".to_string()).await;

        let rows = db.read_capture("err-cap").await.unwrap();
        assert_eq!(rows.len(), 1);
        assert!(rows[0].is_error);
        assert_eq!(
            rows[0].error_message.as_deref(),
            Some("relation does not exist")
        );
    }

    #[test]
    fn test_build_profile_from_staging_basic() {
        let rows = vec![
            StagingRow {
                capture_id: "cap-1".to_string(),
                session_id: 1,
                user_name: Some("app".to_string()),
                database_name: Some("db".to_string()),
                sql: "SELECT 1".to_string(),
                kind: Some("Select".to_string()),
                start_offset_us: 0,
                duration_us: 100,
                is_error: false,
                error_message: None,
                timestamp_us: 0,
            },
            StagingRow {
                capture_id: "cap-1".to_string(),
                session_id: 1,
                user_name: Some("app".to_string()),
                database_name: Some("db".to_string()),
                sql: "SELECT 2".to_string(),
                kind: Some("Select".to_string()),
                start_offset_us: 200,
                duration_us: 50,
                is_error: false,
                error_message: None,
                timestamp_us: 200,
            },
        ];

        let profile = build_profile_from_staging(rows, "test-host", false);
        assert_eq!(profile.version, 2);
        assert_eq!(profile.capture_method, "proxy");
        assert_eq!(profile.metadata.total_sessions, 1);
        assert_eq!(profile.metadata.total_queries, 2);
        assert_eq!(profile.sessions[0].user, "app");
        assert_eq!(profile.sessions[0].queries[0].sql, "SELECT 1");
        assert_eq!(profile.sessions[0].queries[1].sql, "SELECT 2");
    }

    #[test]
    fn test_build_profile_from_staging_with_transactions() {
        let rows = vec![
            StagingRow {
                capture_id: "cap-1".to_string(),
                session_id: 1,
                user_name: Some("app".to_string()),
                database_name: Some("db".to_string()),
                sql: "BEGIN".to_string(),
                kind: Some("Begin".to_string()),
                start_offset_us: 0,
                duration_us: 10,
                is_error: false,
                error_message: None,
                timestamp_us: 0,
            },
            StagingRow {
                capture_id: "cap-1".to_string(),
                session_id: 1,
                user_name: Some("app".to_string()),
                database_name: Some("db".to_string()),
                sql: "INSERT INTO t VALUES (1)".to_string(),
                kind: Some("Insert".to_string()),
                start_offset_us: 100,
                duration_us: 500,
                is_error: false,
                error_message: None,
                timestamp_us: 100,
            },
            StagingRow {
                capture_id: "cap-1".to_string(),
                session_id: 1,
                user_name: Some("app".to_string()),
                database_name: Some("db".to_string()),
                sql: "COMMIT".to_string(),
                kind: Some("Commit".to_string()),
                start_offset_us: 700,
                duration_us: 20,
                is_error: false,
                error_message: None,
                timestamp_us: 700,
            },
        ];

        let profile = build_profile_from_staging(rows, "test-host", false);
        assert_eq!(profile.sessions[0].queries[0].kind, QueryKind::Begin);
        assert_eq!(profile.sessions[0].queries[0].transaction_id, Some(1));
        assert_eq!(profile.sessions[0].queries[1].transaction_id, Some(1));
        assert_eq!(profile.sessions[0].queries[2].transaction_id, Some(1));
    }

    #[test]
    fn test_build_profile_from_staging_with_masking() {
        let rows = vec![StagingRow {
            capture_id: "cap-1".to_string(),
            session_id: 1,
            user_name: Some("app".to_string()),
            database_name: Some("db".to_string()),
            sql: "SELECT * FROM users WHERE email = 'alice@corp.com'".to_string(),
            kind: Some("Select".to_string()),
            start_offset_us: 0,
            duration_us: 100,
            is_error: false,
            error_message: None,
            timestamp_us: 0,
        }];

        let profile = build_profile_from_staging(rows, "test", true);
        assert!(profile.sessions[0].queries[0].sql.contains("$S"));
        assert!(!profile.sessions[0].queries[0].sql.contains("alice"));
    }
}
