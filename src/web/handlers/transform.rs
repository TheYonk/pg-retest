use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::json;

use crate::profile::io;
use crate::transform::analyze::analyze_workload;
use crate::transform::engine::apply_transform;
use crate::transform::plan::TransformPlan;
use crate::web::state::AppState;

#[derive(Deserialize)]
pub struct AnalyzeRequest {
    pub workload_id: String,
}

/// POST /api/v1/transform/analyze
pub async fn analyze_transform(
    State(state): State<AppState>,
    Json(req): Json<AnalyzeRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let file_path = {
        let db = state.db.lock().await;
        let workload = crate::web::db::get_workload(&db, &req.workload_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND)?;
        workload.file_path.clone()
    };

    let profile = io::read_profile(std::path::Path::new(&file_path))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let analysis = analyze_workload(&profile);

    Ok(Json(json!({ "analysis": analysis })))
}

#[derive(Deserialize)]
pub struct PlanRequest {
    pub workload_id: String,
    pub prompt: String,
    pub provider: Option<String>,
    pub api_key: String,
    pub model: Option<String>,
    pub api_url: Option<String>,
}

/// POST /api/v1/transform/plan
pub async fn generate_plan(
    State(state): State<AppState>,
    Json(req): Json<PlanRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let file_path = {
        let db = state.db.lock().await;
        let workload = crate::web::db::get_workload(&db, &req.workload_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND)?;
        workload.file_path.clone()
    };

    let profile = io::read_profile(std::path::Path::new(&file_path))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let analysis = analyze_workload(&profile);

    let provider_str = req.provider.as_deref().unwrap_or("claude");
    let provider: crate::transform::planner::LlmProvider =
        provider_str.parse().map_err(|_| StatusCode::BAD_REQUEST)?;

    let planner =
        crate::transform::planner::create_planner(crate::transform::planner::PlannerConfig {
            provider,
            api_key: req.api_key,
            api_url: req.api_url,
            model: req.model,
        });

    let plan = planner
        .generate_plan(&analysis, &req.prompt)
        .await
        .map_err(|e| {
            tracing::error!("LLM planner error: {e}");
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(Json(json!({ "plan": plan })))
}

#[derive(Deserialize)]
pub struct ApplyRequest {
    pub workload_id: String,
    pub plan: TransformPlan,
    pub seed: Option<u64>,
}

/// POST /api/v1/transform/apply
pub async fn apply_transform_handler(
    State(state): State<AppState>,
    Json(req): Json<ApplyRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let file_path = {
        let db = state.db.lock().await;
        let workload = crate::web::db::get_workload(&db, &req.workload_id)
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
            .ok_or(StatusCode::NOT_FOUND)?;
        workload.file_path.clone()
    };

    let profile = io::read_profile(std::path::Path::new(&file_path))
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let result = apply_transform(&profile, &req.plan, req.seed)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Save transformed profile
    let new_id = uuid::Uuid::new_v4().to_string();
    let filename = format!("transformed-{}.wkl", &new_id[..8]);
    let output_dir = state.data_dir.join("workloads");
    std::fs::create_dir_all(&output_dir).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let output_path = output_dir.join(&filename);

    io::write_profile(&output_path, &result).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;

    // Register in DB
    let db = state.db.lock().await;
    let w = crate::web::db::WorkloadRow {
        id: new_id.clone(),
        name: format!("transformed-{}", &new_id[..8]),
        file_path: output_path.to_string_lossy().to_string(),
        source_type: Some("transformed".into()),
        source_host: Some(result.source_host.clone()),
        captured_at: Some(result.captured_at.to_rfc3339()),
        total_sessions: Some(result.metadata.total_sessions as i64),
        total_queries: Some(result.metadata.total_queries as i64),
        capture_duration_us: Some(result.metadata.capture_duration_us as i64),
        classification: None,
        created_at: None,
    };
    let _ = crate::web::db::insert_workload(&db, &w);

    Ok(Json(json!({
        "workload_id": new_id,
        "total_sessions": result.metadata.total_sessions,
        "total_queries": result.metadata.total_queries,
    })))
}
