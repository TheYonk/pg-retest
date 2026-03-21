use axum::extract::State;
use axum::http::StatusCode;
use axum::Json;
use serde::Deserialize;

use crate::tuner::types::TuningEvent;
use crate::web::db;
use crate::web::state::AppState;
use crate::web::ws::WsMessage;

#[derive(Deserialize)]
pub struct StartTuningRequest {
    pub workload_id: String,
    pub target: String,
    pub provider: Option<String>,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub max_iterations: Option<u32>,
    pub hint: Option<String>,
    pub apply: Option<bool>,
    pub speed: Option<f64>,
    pub read_only: Option<bool>,
}

pub async fn start_tuning(
    State(state): State<AppState>,
    Json(req): Json<StartTuningRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    // Check if tuning is already running
    if state.tasks.has_running("tuning").await {
        return Err((
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "Tuning is already running" })),
        ));
    }

    // Resolve workload path
    let wkl_path = state.data_dir.join("workloads").join(&req.workload_id);
    if !wkl_path.exists() {
        return Err((
            StatusCode::NOT_FOUND,
            Json(
                serde_json::json!({ "error": format!("Workload not found: {}", req.workload_id) }),
            ),
        ));
    }

    let config = crate::tuner::types::TuningConfig {
        workload_path: wkl_path,
        target: req.target.clone(),
        provider: req.provider.unwrap_or_else(|| "claude".into()),
        api_key: req.api_key,
        api_url: req.api_url,
        model: req.model,
        max_iterations: req.max_iterations.unwrap_or(3),
        hint: req.hint,
        apply: req.apply.unwrap_or(false),
        force: false, // Web UI never allows force
        speed: req.speed.unwrap_or(1.0),
        read_only: req.read_only.unwrap_or(false),
        tls: None,
    };

    let state_clone = state.clone();
    let workload_id_clone = req.workload_id.clone();
    let task_id = state
        .tasks
        .clone()
        .spawn(
            "tuning",
            &format!("Tune {}", req.workload_id),
            move |_cancel, task_id| {
                let workload_id_clone = workload_id_clone;
                tokio::spawn(async move {
                    // Create event channel for per-iteration progress
                    let (events_tx, mut events_rx) =
                        tokio::sync::mpsc::unbounded_channel::<TuningEvent>();

                    let state_for_events = state_clone.clone();
                    let task_id_for_events = task_id.clone();

                    // Spawn event consumer that broadcasts WS messages
                    let events_handle = tokio::spawn(async move {
                        while let Some(event) = events_rx.recv().await {
                            let msg = match event {
                                TuningEvent::BaselineStarted => WsMessage::TuningIterationStarted {
                                    task_id: task_id_for_events.clone(),
                                    iteration: 0,
                                },
                                TuningEvent::IterationStarted {
                                    iteration,
                                    max_iterations: _,
                                } => WsMessage::TuningIterationStarted {
                                    task_id: task_id_for_events.clone(),
                                    iteration,
                                },
                                TuningEvent::RecommendationsReceived {
                                    iteration,
                                    recommendations,
                                } => WsMessage::TuningRecommendations {
                                    task_id: task_id_for_events.clone(),
                                    iteration,
                                    count: recommendations.len(),
                                },
                                TuningEvent::ChangeApplied { iteration, change } => {
                                    let summary = match &change.recommendation {
                                        crate::tuner::types::Recommendation::ConfigChange {
                                            parameter,
                                            recommended_value,
                                            ..
                                        } => format!("{} = {}", parameter, recommended_value),
                                        crate::tuner::types::Recommendation::CreateIndex {
                                            table,
                                            columns,
                                            ..
                                        } => format!("index on {}.{}", table, columns.join(",")),
                                        crate::tuner::types::Recommendation::QueryRewrite {
                                            ..
                                        } => "query rewrite".into(),
                                        crate::tuner::types::Recommendation::SchemaChange {
                                            description,
                                            ..
                                        } => description.clone(),
                                    };
                                    WsMessage::TuningChangeApplied {
                                        task_id: task_id_for_events.clone(),
                                        iteration,
                                        success: change.success,
                                        summary,
                                    }
                                }
                                TuningEvent::ReplayCompleted {
                                    iteration,
                                    comparison,
                                } => WsMessage::TuningReplayCompleted {
                                    task_id: task_id_for_events.clone(),
                                    iteration,
                                    improvement_pct: -comparison.p95_change_pct,
                                },
                                TuningEvent::RollbackStarted { iteration } => {
                                    WsMessage::TuningRollbackStarted {
                                        task_id: task_id_for_events.clone(),
                                        iteration,
                                    }
                                }
                                TuningEvent::RollbackCompleted {
                                    iteration,
                                    rolled_back,
                                    failed,
                                } => WsMessage::TuningRollbackCompleted {
                                    task_id: task_id_for_events.clone(),
                                    iteration,
                                    rolled_back,
                                    failed,
                                },
                            };
                            state_for_events.broadcast(msg);
                        }
                    });

                    match crate::tuner::run_tuning_with_events(&config, Some(events_tx)).await {
                        Ok(report) => {
                            let total = report.total_improvement_pct;
                            let iters = report.iterations.len() as u32;

                            // Persist report to SQLite
                            let report_id = uuid::Uuid::new_v4().to_string();
                            if let Ok(report_json) = serde_json::to_string(&report) {
                                let row = db::TuningReportRow {
                                    id: report_id.clone(),
                                    run_id: Some(task_id.clone()),
                                    workload_id: Some(workload_id_clone),
                                    target: report.target.clone(),
                                    provider: report.provider.clone(),
                                    hint: report.hint.clone(),
                                    iterations: iters as i64,
                                    total_improvement_pct: total,
                                    report_json: report_json.clone(),
                                    created_at: None,
                                };
                                let conn = state_clone.db.lock().await;
                                let _ = db::insert_tuning_report(&conn, &row);

                                // Also create a run entry for history tracking
                                let run = db::RunRow {
                                    id: task_id.clone(),
                                    run_type: "tuning".into(),
                                    status: "completed".into(),
                                    workload_id: row.workload_id.clone(),
                                    config_json: None,
                                    started_at: None,
                                    finished_at: Some(chrono::Utc::now().to_rfc3339()),
                                    target_conn: Some(report.target.clone()),
                                    replay_mode: None,
                                    speed: None,
                                    scale: None,
                                    results_path: None,
                                    report_json: Some(report_json),
                                    exit_code: Some(0),
                                    error_message: None,
                                    created_at: None,
                                };
                                let _ = db::insert_run(&conn, &run);
                            }

                            state_clone.broadcast(WsMessage::TuningCompleted {
                                task_id,
                                total_improvement_pct: total,
                                iterations_completed: iters,
                            });
                        }
                        Err(e) => {
                            state_clone.broadcast(WsMessage::Error {
                                message: format!("Tuning failed: {e}"),
                            });
                        }
                    }

                    // Wait for event consumer to finish
                    let _ = events_handle.await;
                })
            },
        )
        .await;

    Ok(Json(serde_json::json!({ "task_id": task_id })))
}

pub async fn get_tuning_status(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    match state.tasks.get(&id).await {
        Some(info) => Ok(Json(serde_json::json!({
            "task_id": info.id,
            "status": if info.running { "running" } else { "completed" },
            "label": info.label,
        }))),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Tuning task not found" })),
        )),
    }
}

pub async fn cancel_tuning(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    if state.tasks.cancel(&id).await {
        Ok(Json(serde_json::json!({ "cancelled": true })))
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Tuning task not found" })),
        ))
    }
}

pub async fn list_tuning_reports(
    State(state): State<AppState>,
) -> Result<Json<Vec<db::TuningReportRow>>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().await;
    match db::list_tuning_reports(&conn, None) {
        Ok(reports) => Ok(Json(reports)),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to list reports: {e}") })),
        )),
    }
}

pub async fn get_tuning_report(
    State(state): State<AppState>,
    axum::extract::Path(id): axum::extract::Path<String>,
) -> Result<Json<db::TuningReportRow>, (StatusCode, Json<serde_json::Value>)> {
    let conn = state.db.lock().await;
    match db::get_tuning_report(&conn, &id) {
        Ok(Some(report)) => Ok(Json(report)),
        Ok(None) => Err((
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({ "error": "Tuning report not found" })),
        )),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": format!("Failed to get report: {e}") })),
        )),
    }
}
