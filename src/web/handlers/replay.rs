use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::web::state::AppState;
use crate::web::ws::WsMessage;

#[derive(Deserialize)]
pub struct StartReplayRequest {
    pub workload_id: String,
    pub target: String,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_speed")]
    pub speed: f64,
    #[serde(default = "default_scale")]
    pub scale: u32,
    #[serde(default)]
    pub stagger_ms: u64,
    pub scale_analytical: Option<u32>,
    pub scale_transactional: Option<u32>,
    pub scale_mixed: Option<u32>,
    pub scale_bulk: Option<u32>,
}

fn default_speed() -> f64 {
    1.0
}
fn default_scale() -> u32 {
    1
}

/// POST /api/v1/replay/start
pub async fn start_replay(
    State(state): State<AppState>,
    Json(req): Json<StartReplayRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Look up workload
    let db = state.db.lock().await;
    let workload = crate::web::db::get_workload(&db, &req.workload_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let wkl_path = workload.file_path.clone();
    drop(db);

    let run_id = uuid::Uuid::new_v4().to_string();
    let mode = if req.read_only {
        crate::replay::ReplayMode::ReadOnly
    } else {
        crate::replay::ReplayMode::ReadWrite
    };

    // Insert run record
    let run_row = crate::web::db::RunRow {
        id: run_id.clone(),
        run_type: "replay".into(),
        status: "running".into(),
        workload_id: Some(req.workload_id.clone()),
        config_json: None,
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        finished_at: None,
        target_conn: Some(req.target.clone()),
        replay_mode: Some(
            if req.read_only {
                "ReadOnly"
            } else {
                "ReadWrite"
            }
            .into(),
        ),
        speed: Some(req.speed),
        scale: Some(req.scale as i64),
        results_path: None,
        report_json: None,
        exit_code: None,
        error_message: None,
        created_at: None,
    };
    {
        let db = state.db.lock().await;
        crate::web::db::insert_run(&db, &run_row).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    }

    let target = req.target.clone();
    let speed = req.speed;
    let scale = req.scale;
    let stagger_ms = req.stagger_ms;
    let scale_analytical = req.scale_analytical;
    let scale_transactional = req.scale_transactional;
    let scale_mixed = req.scale_mixed;
    let scale_bulk = req.scale_bulk;
    let state_clone = state.clone();
    let run_id_clone = run_id.clone();

    // Spawn background task
    let task_id = state
        .tasks
        .clone()
        .spawn(
            "replay",
            &format!("Replay {}", req.workload_id),
            move |cancel_token, task_id| {
                tokio::spawn(async move {
                    let result = run_replay_task(
                        &state_clone,
                        &wkl_path,
                        &target,
                        mode,
                        speed,
                        scale,
                        stagger_ms,
                        scale_analytical,
                        scale_transactional,
                        scale_mixed,
                        scale_bulk,
                        &run_id_clone,
                        &task_id,
                        cancel_token,
                    )
                    .await;

                    if let Err(e) = result {
                        let now = chrono::Utc::now().to_rfc3339();
                        let db = state_clone.db.lock().await;
                        let _ = crate::web::db::update_run_results(
                            &db,
                            &run_id_clone,
                            "failed",
                            &now,
                            None,
                            None,
                            Some(5),
                            Some(&e.to_string()),
                        );
                        state_clone.broadcast(WsMessage::ReplayFailed {
                            task_id: task_id.clone(),
                            error: e.to_string(),
                        });
                    }
                })
            },
        )
        .await;

    Ok(Json(json!({ "task_id": task_id, "run_id": run_id })))
}

#[allow(clippy::too_many_arguments)]
async fn run_replay_task(
    state: &AppState,
    wkl_path: &str,
    target: &str,
    mode: crate::replay::ReplayMode,
    speed: f64,
    scale: u32,
    stagger_ms: u64,
    scale_analytical: Option<u32>,
    scale_transactional: Option<u32>,
    scale_mixed: Option<u32>,
    scale_bulk: Option<u32>,
    run_id: &str,
    task_id: &str,
    _cancel_token: tokio_util::sync::CancellationToken,
) -> anyhow::Result<()> {
    use crate::profile::io;
    use crate::replay::session::replay_session;
    use tokio::time::Instant as TokioInstant;
    use tracing::warn;

    let profile = io::read_profile(std::path::Path::new(wkl_path))?;

    // Apply scaling
    let has_class_scaling = scale_analytical.is_some()
        || scale_transactional.is_some()
        || scale_mixed.is_some()
        || scale_bulk.is_some();

    let replay_profile = if has_class_scaling {
        use crate::classify::WorkloadClass;
        use crate::replay::scaling::scale_sessions_by_class;
        use std::collections::HashMap;

        let mut class_scales = HashMap::new();
        class_scales.insert(WorkloadClass::Analytical, scale_analytical.unwrap_or(1));
        class_scales.insert(
            WorkloadClass::Transactional,
            scale_transactional.unwrap_or(1),
        );
        class_scales.insert(WorkloadClass::Mixed, scale_mixed.unwrap_or(1));
        class_scales.insert(WorkloadClass::Bulk, scale_bulk.unwrap_or(1));

        let scaled_sessions = scale_sessions_by_class(&profile, &class_scales, stagger_ms);
        let mut scaled = profile.clone();
        scaled.sessions = scaled_sessions;
        scaled.metadata.total_sessions = scaled.sessions.len() as u64;
        scaled.metadata.total_queries =
            scaled.sessions.iter().map(|s| s.queries.len() as u64).sum();
        scaled
    } else if scale > 1 {
        use crate::replay::scaling::scale_sessions;
        let scaled_sessions = scale_sessions(&profile, scale, stagger_ms);
        let mut scaled = profile.clone();
        scaled.sessions = scaled_sessions;
        scaled.metadata.total_sessions = scaled.sessions.len() as u64;
        scaled.metadata.total_queries =
            scaled.sessions.iter().map(|s| s.queries.len() as u64).sum();
        scaled
    } else {
        profile.clone()
    };

    let total = replay_profile.sessions.len() as u64;
    state.broadcast(WsMessage::ReplayProgress {
        task_id: task_id.to_string(),
        completed: 0,
        total,
        pct: 0.0,
    });

    // Spawn all sessions concurrently
    let replay_start = TokioInstant::now();
    let mut handles = Vec::new();
    for session in &replay_profile.sessions {
        let session = session.clone();
        let conn_str = target.to_string();
        let handle = tokio::spawn(async move {
            replay_session(&session, &conn_str, mode, speed, replay_start, None).await
        });
        handles.push(handle);
    }

    // Collect results with per-session progress broadcast
    let mut all_results = Vec::new();
    let mut completed = 0u64;
    for handle in handles {
        match handle.await? {
            Ok(results) => all_results.push(results),
            Err(e) => warn!("Session replay failed: {e}"),
        }
        completed += 1;
        let pct = (completed as f64 / total as f64) * 100.0;
        state.broadcast(WsMessage::ReplayProgress {
            task_id: task_id.to_string(),
            completed,
            total,
            pct,
        });
    }

    // Save results
    let results_dir = state.data_dir.join("results");
    std::fs::create_dir_all(&results_dir)?;
    let results_path = results_dir.join(format!("{run_id}.wkl"));
    let bytes = rmp_serde::to_vec(&all_results)?;
    std::fs::write(&results_path, bytes)?;

    // Compute comparison
    let comparison = crate::compare::compute_comparison(&profile, &all_results, 20.0, None);
    let report_json = serde_json::to_string(&comparison)?;

    let now = chrono::Utc::now().to_rfc3339();
    let db = state.db.lock().await;
    crate::web::db::update_run_results(
        &db,
        run_id,
        "completed",
        &now,
        Some(&results_path.to_string_lossy()),
        Some(&report_json),
        Some(0),
        None,
    )?;

    state.broadcast(WsMessage::ReplayCompleted {
        task_id: task_id.to_string(),
        run_id: run_id.to_string(),
    });

    Ok(())
}

/// GET /api/v1/replay/:id
pub async fn get_replay(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;
    match crate::web::db::get_run(&db, &id) {
        Ok(Some(run)) => Ok(Json(json!(run))),
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

/// POST /api/v1/replay/:id/cancel
pub async fn cancel_replay(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Json<serde_json::Value> {
    let cancelled = state.tasks.cancel(&id).await;
    Json(json!({ "cancelled": cancelled }))
}
