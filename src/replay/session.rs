use std::time::Instant;

use anyhow::Result;
use tokio::time::{sleep_until, Instant as TokioInstant};
use tokio_postgres::NoTls;
use tracing::{debug, warn};

use crate::profile::Session;
use crate::replay::{QueryResult, ReplayMode, ReplayResults};

pub async fn replay_session(
    session: &Session,
    connection_string: &str,
    mode: ReplayMode,
    speed: f64,
    replay_start: TokioInstant,
) -> Result<ReplayResults> {
    let (client, connection) = tokio_postgres::connect(connection_string, NoTls).await?;

    // Spawn the connection handler
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            warn!("Connection error for session: {e}");
        }
    });

    let mut query_results = Vec::new();

    for query in &session.queries {
        if !mode.should_replay(query) {
            continue;
        }

        // Wait until the scaled target time
        let target_offset =
            std::time::Duration::from_micros((query.start_offset_us as f64 / speed) as u64);
        sleep_until(replay_start + target_offset).await;

        let start = Instant::now();
        let result = client.simple_query(&query.sql).await;
        let elapsed_us = start.elapsed().as_micros() as u64;

        let (success, error) = match result {
            Ok(_) => (true, None),
            Err(e) => {
                debug!("Query error in session {}: {e}", session.id);
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
) -> Result<Vec<ReplayResults>> {
    let replay_start = TokioInstant::now();
    let mut handles = Vec::new();

    for session in &profile.sessions {
        let session = session.clone();
        let conn_str = connection_string.to_string();

        let handle = tokio::spawn(async move {
            replay_session(&session, &conn_str, mode, speed, replay_start).await
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
