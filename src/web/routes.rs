use axum::{
    routing::{delete, get, post},
    Router,
};

use super::handlers;
use super::state::AppState;
use super::ws;

/// Build the complete API router.
pub fn build_router(state: AppState) -> Router {
    let api = Router::new()
        // Health
        .route("/health", get(handlers::health))
        .route("/tasks", get(handlers::list_tasks))
        // WebSocket
        .route("/ws", get(ws::ws_handler))
        // Workloads
        .route("/workloads", get(handlers::workloads::list_workloads))
        .route(
            "/workloads/upload",
            post(handlers::workloads::upload_workload),
        )
        .route(
            "/workloads/import",
            post(handlers::workloads::import_workload),
        )
        .route("/workloads/{id}", get(handlers::workloads::get_workload))
        .route(
            "/workloads/{id}",
            delete(handlers::workloads::delete_workload),
        )
        .route(
            "/workloads/{id}/inspect",
            get(handlers::workloads::inspect_workload),
        )
        // Proxy
        .route("/proxy/status", get(handlers::proxy::proxy_status))
        .route("/proxy/start", post(handlers::proxy::start_proxy))
        .route("/proxy/stop", post(handlers::proxy::stop_proxy))
        .route(
            "/proxy/toggle-capture",
            post(handlers::proxy::toggle_capture),
        )
        .route("/proxy/sessions", get(handlers::proxy::proxy_sessions))
        // Replay
        .route("/replay/start", post(handlers::replay::start_replay))
        .route("/replay/{id}", get(handlers::replay::get_replay))
        .route("/replay/{id}/cancel", post(handlers::replay::cancel_replay))
        // Compare
        .route("/compare", post(handlers::compare::compute_compare))
        .route("/compare/{run_id}", get(handlers::compare::get_compare))
        // A/B
        .route("/ab/start", post(handlers::ab::start_ab))
        .route("/ab/{id}", get(handlers::ab::get_ab))
        // Pipeline
        .route("/pipeline/start", post(handlers::pipeline::start_pipeline))
        .route(
            "/pipeline/validate",
            post(handlers::pipeline::validate_pipeline),
        )
        .route("/pipeline/{id}", get(handlers::pipeline::get_pipeline))
        // Transform
        .route(
            "/transform/analyze",
            post(handlers::transform::analyze_transform),
        )
        .route("/transform/plan", post(handlers::transform::generate_plan))
        .route(
            "/transform/apply",
            post(handlers::transform::apply_transform_handler),
        )
        // Tuning
        .route("/tuning/start", post(handlers::tuning::start_tuning))
        .route(
            "/tuning/reports",
            get(handlers::tuning::list_tuning_reports),
        )
        .route(
            "/tuning/reports/{id}",
            get(handlers::tuning::get_tuning_report),
        )
        .route("/tuning/{id}", get(handlers::tuning::get_tuning_status))
        .route("/tuning/{id}/cancel", post(handlers::tuning::cancel_tuning))
        // Demo
        .route("/demo/config", get(handlers::demo::get_config))
        .route("/demo/reset-db", post(handlers::demo::reset_db))
        .route("/demo/wizard/{step}", post(handlers::demo::run_wizard_step))
        .route("/demo/wizard/{step}", get(handlers::demo::get_wizard_step))
        .route("/demo/scenario/{name}", post(handlers::demo::run_scenario))
        .route("/demo/scenario/{name}", get(handlers::demo::get_scenario))
        // Runs
        .route("/runs", get(handlers::runs::list_runs))
        .route("/runs/stats", get(handlers::runs::run_stats))
        .route("/runs/trends", get(handlers::runs::run_trends))
        .route("/runs/{id}", get(handlers::runs::get_run));

    Router::new().nest("/api/v1", api).with_state(state)
}
