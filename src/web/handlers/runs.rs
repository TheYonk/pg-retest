use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::web::state::AppState;

#[derive(Deserialize, Default)]
pub struct RunsQuery {
    pub run_type: Option<String>,
    pub limit: Option<u32>,
}

/// GET /api/v1/runs
pub async fn list_runs(
    State(state): State<AppState>,
    Query(q): Query<RunsQuery>,
) -> Json<serde_json::Value> {
    let db = state.db.lock().await;
    match crate::web::db::list_runs(&db, q.run_type.as_deref(), q.limit) {
        Ok(runs) => Json(json!({ "runs": runs })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/v1/runs/stats
pub async fn run_stats(State(state): State<AppState>) -> Json<serde_json::Value> {
    let db = state.db.lock().await;
    match crate::web::db::get_run_stats(&db) {
        Ok(stats) => Json(json!({ "stats": stats })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}

/// GET /api/v1/runs/:id
pub async fn get_run(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;
    match crate::web::db::get_run(&db, &id) {
        Ok(Some(run)) => {
            let report = run
                .report_json
                .as_ref()
                .and_then(|j| serde_json::from_str::<serde_json::Value>(j).ok());
            let threshold_results =
                crate::web::db::list_threshold_results(&db, &id).unwrap_or_default();
            Ok(Json(
                json!({ "run": run, "report": report, "threshold_results": threshold_results }),
            ))
        }
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Deserialize, Default)]
pub struct TrendQuery {
    pub workload_id: Option<String>,
    pub limit: Option<u32>,
}

/// GET /api/v1/runs/trends
pub async fn run_trends(
    State(state): State<AppState>,
    Query(q): Query<TrendQuery>,
) -> Json<serde_json::Value> {
    let db = state.db.lock().await;
    match crate::web::db::get_run_trend(&db, q.workload_id.as_deref(), q.limit) {
        Ok(trends) => Json(json!({ "trends": trends })),
        Err(e) => Json(json!({ "error": e.to_string() })),
    }
}
