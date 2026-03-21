use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde::Deserialize;
use serde_json::json;

use crate::web::state::AppState;

#[derive(Deserialize)]
pub struct CompareRequest {
    pub workload_id: String,
    pub run_id: String,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
}

fn default_threshold() -> f64 {
    20.0
}

/// POST /api/v1/compare
pub async fn compute_compare(
    State(state): State<AppState>,
    Json(req): Json<CompareRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;
    let workload = crate::web::db::get_workload(&db, &req.workload_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    let run = crate::web::db::get_run(&db, &req.run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    drop(db);

    // Read source profile
    let profile = crate::profile::io::read_profile(std::path::Path::new(&workload.file_path))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Read replay results
    let results_path = run.results_path.ok_or(StatusCode::BAD_REQUEST)?;
    let replay_bytes = std::fs::read(&results_path).map_err(|_| StatusCode::NOT_FOUND)?;
    let results: Vec<crate::replay::ReplayResults> =
        rmp_serde::from_slice(&replay_bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    let report = crate::compare::compute_comparison(&profile, &results, req.threshold, None);
    let report_json =
        serde_json::to_string(&report).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Store report on the run
    let now = chrono::Utc::now().to_rfc3339();
    let db = state.db.lock().await;
    let _ = crate::web::db::update_run_results(
        &db,
        &req.run_id,
        "completed",
        &now,
        Some(&results_path),
        Some(&report_json),
        run.exit_code.map(|c| c as i32),
        None,
    );

    Ok(Json(json!({ "report": report })))
}

/// GET /api/v1/compare/:run_id
pub async fn get_compare(
    State(state): State<AppState>,
    Path(run_id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let db = state.db.lock().await;
    let run = crate::web::db::get_run(&db, &run_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;

    if let Some(report_json) = &run.report_json {
        let report: serde_json::Value =
            serde_json::from_str(report_json).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        Ok(Json(json!({ "report": report, "run": run })))
    } else {
        Ok(Json(json!({ "run": run, "report": null })))
    }
}
