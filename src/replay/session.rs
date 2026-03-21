use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::sync::Semaphore;
use tokio::time::{sleep_until, Instant as TokioInstant};
use tokio_postgres::NoTls;
use tokio_postgres_rustls::MakeRustlsConnect;
use tracing::{debug, info, warn};

use crate::profile::{QueryKind, Session};
use crate::replay::{QueryResult, ReplayMode, ReplayResults};

pub async fn replay_session(
    session: &Session,
    connection_string: &str,
    mode: ReplayMode,
    speed: f64,
    replay_start: TokioInstant,
    tls: Option<MakeRustlsConnect>,
) -> Result<ReplayResults> {
    let client = if let Some(tls_connector) = tls {
        let (client, connection) =
            tokio_postgres::connect(connection_string, tls_connector).await?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!("Connection error for session: {e}");
            }
        });
        client
    } else {
        let (client, connection) = tokio_postgres::connect(connection_string, NoTls).await?;
        tokio::spawn(async move {
            if let Err(e) = connection.await {
                warn!("Connection error for session: {e}");
            }
        });
        client
    };

    let mut query_results = Vec::new();
    let mut failed_txn_id: Option<u64> = None;

    for query in &session.queries {
        if !mode.should_replay(query) {
            continue;
        }

        // If we're inside a failed transaction, handle specially
        if let Some(failed_id) = failed_txn_id {
            if query.transaction_id == Some(failed_id) {
                // COMMIT/ROLLBACK ends the failed transaction
                if query.kind == QueryKind::Commit || query.kind == QueryKind::Rollback {
                    failed_txn_id = None;
                    query_results.push(QueryResult {
                        sql: query.sql.clone(),
                        original_duration_us: query.duration_us,
                        replay_duration_us: 0,
                        success: false,
                        error: Some("skipped: transaction already rolled back".into()),
                    });
                } else {
                    // Skip remaining queries in failed transaction
                    query_results.push(QueryResult {
                        sql: query.sql.clone(),
                        original_duration_us: query.duration_us,
                        replay_duration_us: 0,
                        success: false,
                        error: Some("skipped: transaction failed".into()),
                    });
                }
                continue;
            } else {
                // Different transaction or no transaction — clear failed state
                failed_txn_id = None;
            }
        }

        // Wait until the scaled target time (speed=0 means max speed, no delays)
        if speed > 0.0 {
            let target_offset =
                std::time::Duration::from_micros((query.start_offset_us as f64 / speed) as u64);
            sleep_until(replay_start + target_offset).await;
        }

        let start = Instant::now();
        let result = client.simple_query(&query.sql).await;
        let elapsed_us = start.elapsed().as_micros() as u64;

        let (success, error) = match result {
            Ok(_) => (true, None),
            Err(e) => {
                debug!("Query error in session {}: {e}", session.id);

                // If this query is inside a transaction, issue ROLLBACK and mark txn as failed
                if let Some(txn_id) = query.transaction_id {
                    if !query.kind.is_transaction_control() {
                        debug!(
                            "Rolling back failed transaction {} in session {}",
                            txn_id, session.id
                        );
                        let _ = client.simple_query("ROLLBACK").await;
                        failed_txn_id = Some(txn_id);
                    }
                }

                (false, Some(e.to_string()))
            }
        };

        query_results.push(QueryResult {
            sql: query.sql.clone(),
            original_duration_us: query.duration_us,
            replay_duration_us: elapsed_us,
            success,
            error,
        });
    }

    Ok(ReplayResults {
        session_id: session.id,
        query_results,
    })
}

pub async fn run_replay(
    profile: &crate::profile::WorkloadProfile,
    connection_string: &str,
    mode: ReplayMode,
    speed: f64,
    max_connections: Option<u32>,
    tls: Option<MakeRustlsConnect>,
) -> Result<Vec<ReplayResults>> {
    let replay_start = TokioInstant::now();
    let mut handles = Vec::new();
    let session_count = profile.sessions.len();

    let semaphore = max_connections.map(|n| Arc::new(Semaphore::new(n as usize)));

    if let Some(max) = max_connections {
        if session_count > max as usize {
            info!(
                "Concurrency limited to {} (workload has {} sessions)",
                max, session_count
            );
        }
    }

    for session in &profile.sessions {
        let session = session.clone();
        let conn_str = connection_string.to_string();
        let sem = semaphore.clone();
        let tls_clone = tls.clone();

        let handle = tokio::spawn(async move {
            let _permit = match sem {
                Some(ref s) => Some(s.acquire().await.unwrap()),
                None => None,
            };
            replay_session(&session, &conn_str, mode, speed, replay_start, tls_clone).await
        });

        handles.push(handle);
    }

    let mut all_results = Vec::new();
    for handle in handles {
        match handle.await? {
            Ok(results) => all_results.push(results),
            Err(e) => warn!("Session replay failed: {e}"),
        }
    }

    Ok(all_results)
}
