use std::collections::HashMap;
use std::time::Instant;

use chrono::Utc;
use tokio::sync::mpsc;
use tracing::debug;

use crate::capture::masking::mask_sql_literals;
use crate::profile::{self, Metadata, Query, QueryKind, Session, WorkloadProfile};

/// Events sent from relay tasks to the capture collector.
#[derive(Debug)]
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
                },
                CapturedQuery {
                    sql: "INSERT INTO t VALUES (1)".into(),
                    start_offset_us: 100,
                    duration_us: 500,
                },
                CapturedQuery {
                    sql: "COMMIT".into(),
                    start_offset_us: 700,
                    duration_us: 20,
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
            }],
        )];

        let profile = build_profile(captured, "test", true);
        assert!(profile.sessions[0].queries[0].sql.contains("$S"));
        assert!(!profile.sessions[0].queries[0].sql.contains("alice"));
    }
}
