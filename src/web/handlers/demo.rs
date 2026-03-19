use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use serde_json::json;
use tokio::sync::RwLock;

use crate::web::state::AppState;

#[derive(Default)]
struct DemoState {
    wizard_results: HashMap<u32, serde_json::Value>,
    wizard_status: HashMap<u32, String>,
    scenario_results: HashMap<String, serde_json::Value>,
    scenario_status: HashMap<String, String>,
}

fn demo_state() -> &'static Arc<RwLock<DemoState>> {
    static INSTANCE: OnceLock<Arc<RwLock<DemoState>>> = OnceLock::new();
    INSTANCE.get_or_init(|| Arc::new(RwLock::new(DemoState::default())))
}

/// GET /api/v1/demo/config
pub async fn get_config(State(state): State<AppState>) -> Json<serde_json::Value> {
    match &state.demo_config {
        Some(dc) => {
            // Extract hostname only from connection strings — never expose credentials
            let extract_host = |conn: &str| -> String {
                conn.split_whitespace()
                    .find(|s| s.starts_with("host="))
                    .map(|s| s.trim_start_matches("host=").to_string())
                    .unwrap_or_else(|| "unknown".to_string())
            };
            Json(json!({
                "enabled": true,
                "db_a": extract_host(&dc.db_a),
                "db_b": extract_host(&dc.db_b),
            }))
        }
        None => Json(json!({
            "enabled": false,
            "db_a": "",
            "db_b": "",
        })),
    }
}

/// POST /api/v1/demo/reset-db
pub async fn reset_db(
    State(state): State<AppState>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let dc = state.demo_config.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    // Read init SQL file
    let init_sql = std::fs::read_to_string(&dc.init_sql_path).map_err(|e| {
        tracing::error!("Failed to read init SQL file {:?}: {}", dc.init_sql_path, e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Connect to DB-B
    let client = crate::tuner::context::connect(&dc.db_b)
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect to DB-B: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    // Drop all tables in the public schema
    let rows = client
        .query(
            "SELECT tablename FROM pg_tables WHERE schemaname = 'public'",
            &[],
        )
        .await
        .map_err(|e| {
            tracing::error!("Failed to list tables: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    for row in &rows {
        let table: String = row.get(0);
        client
            .execute(&format!("DROP TABLE IF EXISTS \"{}\" CASCADE", table), &[])
            .await
            .map_err(|e| {
                tracing::error!("Failed to drop table {}: {}", table, e);
                StatusCode::INTERNAL_SERVER_ERROR
            })?;
    }

    // Execute init SQL (split on semicolons for multi-statement)
    for stmt in init_sql.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        if let Err(e) = client.execute(stmt, &[]).await {
            tracing::warn!("Init SQL statement failed (continuing): {}", e);
        }
    }

    // Clear wizard/scenario state on reset
    {
        let mut ds = demo_state().write().await;
        ds.wizard_results.clear();
        ds.wizard_status.clear();
        ds.scenario_results.clear();
        ds.scenario_status.clear();
    }

    Ok(Json(json!({
        "status": "ok",
        "tables_dropped": rows.len(),
        "init_sql_executed": true,
    })))
}

/// POST /api/v1/demo/wizard/:step
pub async fn run_wizard_step(
    State(state): State<AppState>,
    Path(step): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let dc = state.demo_config.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    // Mark as running
    {
        let mut ds = demo_state().write().await;
        ds.wizard_status.insert(step, "running".into());
    }

    let result = match step {
        1 => run_step_explore(dc).await,
        2 => run_step_replay(dc, &state).await,
        3 => run_step_compare(dc, &state).await,
        4 => run_step_scale(dc, &state).await,
        5 => run_step_tune(dc).await,
        _ => Err(StatusCode::BAD_REQUEST),
    };

    match result {
        Ok(value) => {
            let mut ds = demo_state().write().await;
            ds.wizard_status.insert(step, "completed".into());
            ds.wizard_results.insert(step, value.clone());
            Ok(Json(json!({ "status": "completed", "result": value })))
        }
        Err(code) => {
            let mut ds = demo_state().write().await;
            ds.wizard_status.insert(step, "failed".into());
            Err(code)
        }
    }
}

/// GET /api/v1/demo/wizard/:step
pub async fn get_wizard_step(
    State(state): State<AppState>,
    Path(step): Path<u32>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    state.demo_config.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    let ds = demo_state().read().await;
    let status = ds
        .wizard_status
        .get(&step)
        .cloned()
        .unwrap_or_else(|| "pending".into());
    let result = ds.wizard_results.get(&step).cloned();

    Ok(Json(json!({
        "step": step,
        "status": status,
        "result": result,
    })))
}

/// POST /api/v1/demo/scenario/:name
pub async fn run_scenario(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let dc = state.demo_config.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    // Mark as running
    {
        let mut ds = demo_state().write().await;
        ds.scenario_status.insert(name.clone(), "running".into());
    }

    let result = match name.as_str() {
        "migration" => run_scenario_migration(dc, &state).await,
        "capacity" => run_scenario_capacity(dc, &state).await,
        "ab" => run_scenario_ab(dc).await,
        "tuning" => run_scenario_tuning(dc).await,
        _ => Err(StatusCode::BAD_REQUEST),
    };

    match result {
        Ok(value) => {
            let mut ds = demo_state().write().await;
            ds.scenario_status.insert(name.clone(), "completed".into());
            ds.scenario_results.insert(name, value.clone());
            Ok(Json(json!({ "status": "completed", "result": value })))
        }
        Err(code) => {
            let mut ds = demo_state().write().await;
            ds.scenario_status.insert(name, "failed".into());
            Err(code)
        }
    }
}

/// GET /api/v1/demo/scenario/:name
pub async fn get_scenario(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    state.demo_config.as_ref().ok_or(StatusCode::NOT_FOUND)?;

    let ds = demo_state().read().await;
    let status = ds
        .scenario_status
        .get(&name)
        .cloned()
        .unwrap_or_else(|| "pending".into());
    let result = ds.scenario_results.get(&name).cloned();

    Ok(Json(json!({
        "name": name,
        "status": status,
        "result": result,
    })))
}

// ── Wizard step implementations ─────────────────────────────────────

/// Step 1: Explore — load profile, classify, return stats
async fn run_step_explore(
    dc: &crate::web::state::DemoConfig,
) -> Result<serde_json::Value, StatusCode> {
    let profile = crate::profile::io::read_profile(&dc.workload_path).map_err(|e| {
        tracing::error!("Failed to read workload profile: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let classification = crate::classify::classify_workload(&profile);

    Ok(json!({
        "sessions": profile.sessions.len(),
        "total_queries": profile.metadata.total_queries,
        "capture_duration_us": profile.metadata.capture_duration_us,
        "source_host": profile.source_host,
        "pg_version": profile.pg_version,
        "capture_method": profile.capture_method,
        "overall_class": format!("{:?}", classification.overall_class),
        "class_counts": {
            "analytical": classification.class_counts.analytical,
            "transactional": classification.class_counts.transactional,
            "mixed": classification.class_counts.mixed,
            "bulk": classification.class_counts.bulk,
        },
    }))
}

/// Step 2: Replay — run replay against DB-B, save results
async fn run_step_replay(
    dc: &crate::web::state::DemoConfig,
    state: &AppState,
) -> Result<serde_json::Value, StatusCode> {
    let profile = crate::profile::io::read_profile(&dc.workload_path).map_err(|e| {
        tracing::error!("Failed to read workload profile: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let results = crate::replay::session::run_replay(
        &profile,
        &dc.db_b,
        crate::replay::ReplayMode::ReadWrite,
        1.0,
    )
    .await
    .map_err(|e| {
        tracing::error!("Replay failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Save results to disk for step 3
    let results_dir = state.data_dir.join("results");
    std::fs::create_dir_all(&results_dir).map_err(|e| {
        tracing::error!("Failed to create results dir: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let results_path = results_dir.join("demo-replay.msgpack");
    let bytes = rmp_serde::to_vec(&results).map_err(|e| {
        tracing::error!("Failed to serialize results: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    std::fs::write(&results_path, bytes).map_err(|e| {
        tracing::error!("Failed to write results: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Summarize
    let total_queries: usize = results.iter().map(|r| r.query_results.len()).sum();
    let total_errors: usize = results
        .iter()
        .map(|r| r.query_results.iter().filter(|q| !q.success).count())
        .sum();

    Ok(json!({
        "sessions_replayed": results.len(),
        "total_queries": total_queries,
        "total_errors": total_errors,
        "results_path": results_path.to_string_lossy(),
    }))
}

/// Step 3: Compare — load saved results + source, compute comparison
async fn run_step_compare(
    dc: &crate::web::state::DemoConfig,
    state: &AppState,
) -> Result<serde_json::Value, StatusCode> {
    let profile = crate::profile::io::read_profile(&dc.workload_path).map_err(|e| {
        tracing::error!("Failed to read workload profile: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Load saved replay results
    let results_path = state.data_dir.join("results").join("demo-replay.msgpack");
    let bytes = std::fs::read(&results_path).map_err(|e| {
        tracing::error!("Failed to read saved results (run step 2 first): {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let results: Vec<crate::replay::ReplayResults> =
        rmp_serde::from_slice(&bytes).map_err(|e| {
            tracing::error!("Failed to deserialize results: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let comparison = crate::compare::compute_comparison(&profile, &results, 20.0);
    let value = serde_json::to_value(&comparison).map_err(|e| {
        tracing::error!("Failed to serialize comparison: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(value)
}

/// Step 4: Scale — scale sessions, replay, compute scale report
async fn run_step_scale(
    dc: &crate::web::state::DemoConfig,
    _state: &AppState,
) -> Result<serde_json::Value, StatusCode> {
    use crate::classify::WorkloadClass;
    use std::collections::HashMap as StdHashMap;

    let profile = crate::profile::io::read_profile(&dc.workload_path).map_err(|e| {
        tracing::error!("Failed to read workload profile: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Per-category scaling: analytical 2x, transactional 4x, mixed 1x, bulk 1x
    let mut class_scales = StdHashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 2u32);
    class_scales.insert(WorkloadClass::Transactional, 4u32);
    class_scales.insert(WorkloadClass::Mixed, 1u32);
    class_scales.insert(WorkloadClass::Bulk, 1u32);

    let scaled_sessions =
        crate::replay::scaling::scale_sessions_by_class(&profile, &class_scales, 100);

    // Wrap scaled sessions into a cloned profile
    let mut scaled_profile = profile.clone();
    scaled_profile.sessions = scaled_sessions;
    scaled_profile.metadata.total_sessions = scaled_profile.sessions.len() as u64;
    scaled_profile.metadata.total_queries = scaled_profile
        .sessions
        .iter()
        .map(|s| s.queries.len() as u64)
        .sum();

    let start = tokio::time::Instant::now();
    let results = crate::replay::session::run_replay(
        &scaled_profile,
        &dc.db_b,
        crate::replay::ReplayMode::ReadOnly,
        1.0,
    )
    .await
    .map_err(|e| {
        tracing::error!("Scaled replay failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    let elapsed_us = start.elapsed().as_micros() as u64;

    let scale_report =
        crate::compare::capacity::compute_scale_report(&results, 3, elapsed_us);
    let value = serde_json::to_value(&scale_report).map_err(|e| {
        tracing::error!("Failed to serialize scale report: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(value)
}

/// Step 5: Tune — connect to DB-B, collect context, return dry-run info
async fn run_step_tune(
    dc: &crate::web::state::DemoConfig,
) -> Result<serde_json::Value, StatusCode> {
    let profile = crate::profile::io::read_profile(&dc.workload_path).map_err(|e| {
        tracing::error!("Failed to read workload profile: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let client = crate::tuner::context::connect(&dc.db_b)
        .await
        .map_err(|e| {
            tracing::error!("Failed to connect to DB-B for tuning context: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    let context = crate::tuner::context::collect_context(&client, &profile, 10)
        .await
        .map_err(|e| {
            tracing::error!("Failed to collect tuning context: {}", e);
            StatusCode::INTERNAL_SERVER_ERROR
        })?;

    Ok(json!({
        "mode": "dry-run",
        "pg_version": context.pg_version,
        "non_default_settings": context.non_default_settings.len(),
        "top_slow_queries": context.top_slow_queries.len(),
        "stat_statements_available": context.stat_statements.is_some(),
        "schema_tables": context.schema.len(),
        "index_count": context.index_usage.len(),
        "explain_plans": context.explain_plans.len(),
    }))
}

// ── Scenario implementations ────────────────────────────────────────

/// Scenario: migration — replay + compare (like steps 2+3 combined)
async fn run_scenario_migration(
    dc: &crate::web::state::DemoConfig,
    _state: &AppState,
) -> Result<serde_json::Value, StatusCode> {
    let profile = crate::profile::io::read_profile(&dc.workload_path).map_err(|e| {
        tracing::error!("Failed to read workload profile: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let results = crate::replay::session::run_replay(
        &profile,
        &dc.db_b,
        crate::replay::ReplayMode::ReadWrite,
        1.0,
    )
    .await
    .map_err(|e| {
        tracing::error!("Migration replay failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let comparison = crate::compare::compute_comparison(&profile, &results, 20.0);
    let value = serde_json::to_value(&comparison).map_err(|e| {
        tracing::error!("Failed to serialize comparison: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(value)
}

/// Scenario: capacity — same as step 4
async fn run_scenario_capacity(
    dc: &crate::web::state::DemoConfig,
    state: &AppState,
) -> Result<serde_json::Value, StatusCode> {
    run_step_scale(dc, state).await
}

/// Scenario: ab — replay against DB-A and DB-B with ReadOnly, compare
async fn run_scenario_ab(
    dc: &crate::web::state::DemoConfig,
) -> Result<serde_json::Value, StatusCode> {
    let profile = crate::profile::io::read_profile(&dc.workload_path).map_err(|e| {
        tracing::error!("Failed to read workload profile: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Replay against DB-A (baseline)
    let results_a = crate::replay::session::run_replay(
        &profile,
        &dc.db_a,
        crate::replay::ReplayMode::ReadOnly,
        1.0,
    )
    .await
    .map_err(|e| {
        tracing::error!("A/B replay against DB-A failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Replay against DB-B (variant)
    let results_b = crate::replay::session::run_replay(
        &profile,
        &dc.db_b,
        crate::replay::ReplayMode::ReadOnly,
        1.0,
    )
    .await
    .map_err(|e| {
        tracing::error!("A/B replay against DB-B failed: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    let variant_a =
        crate::compare::ab::VariantResult::from_results("DB-A (baseline)".into(), results_a);
    let variant_b =
        crate::compare::ab::VariantResult::from_results("DB-B (variant)".into(), results_b);

    let ab_report = crate::compare::ab::compute_ab_comparison(vec![variant_a, variant_b], 10.0);
    let value = serde_json::to_value(&ab_report).map_err(|e| {
        tracing::error!("Failed to serialize A/B report: {}", e);
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    Ok(value)
}

/// Scenario: tuning — same as step 5
async fn run_scenario_tuning(
    dc: &crate::web::state::DemoConfig,
) -> Result<serde_json::Value, StatusCode> {
    run_step_tune(dc).await
}
