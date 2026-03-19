pub mod ab;
pub mod compare;
pub mod demo;
pub mod pipeline;
pub mod proxy;
pub mod replay;
pub mod runs;
pub mod transform;
pub mod tuning;
pub mod workloads;

use axum::{extract::State, Json};
use serde_json::json;

use super::state::AppState;

/// GET /api/v1/health
pub async fn health() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "name": "pg-retest",
    }))
}

/// GET /api/v1/tasks
pub async fn list_tasks(State(state): State<AppState>) -> Json<serde_json::Value> {
    state.tasks.cleanup().await;
    let tasks = state.tasks.list().await;
    Json(json!({ "tasks": tasks }))
}
