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
pub struct StartABRequest {
    pub workload_id: String,
    pub variants: Vec<VariantDef>,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_speed")]
    pub speed: f64,
    #[serde(default = "default_threshold")]
    pub threshold: f64,
}

#[derive(Deserialize, serde::Serialize, Clone)]
pub struct VariantDef {
    pub label: String,
    pub target: String,
}

fn default_speed() -> f64 {
    1.0
}
fn default_threshold() -> f64 {
    20.0
}

/// POST /api/v1/ab/start
pub async fn start_ab(
    State(state): State<AppState>,
    Json(req): Json<StartABRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    if req.variants.len() < 2 {
        return Ok(Json(json!({ "error": "At least 2 variants required" })));
    }

    let db = state.db.lock().await;
    let workload = crate::web::db::get_workload(&db, &req.workload_id)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    drop(db);

    let run_id = uuid::Uuid::new_v4().to_string();
    let run_row = crate::web::db::RunRow {
        id: run_id.clone(),
        run_type: "ab".into(),
        status: "running".into(),
        workload_id: Some(req.workload_id.clone()),
        config_json: serde_json::to_string(&req.variants).ok(),
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        finished_at: None,
        target_conn: None,
        replay_mode: Some(
            if req.read_only {
                "ReadOnly"
            } else {
                "ReadWrite"
            }
            .into(),
        ),
        speed: Some(req.speed),
        scale: Some(1),
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

    let wkl_path = workload.file_path.clone();
    let variants = req.variants.clone();
    let read_only = req.read_only;
    let speed = req.speed;
    let threshold = req.threshold;
    let state_clone = state.clone();
    let run_id_clone = run_id.clone();

    let task_id = state
        .tasks
        .clone()
        .spawn(
            "ab",
            &format!("A/B test {}", req.workload_id),
            move |_cancel_token, task_id| {
                tokio::spawn(async move {
                    let result = run_ab_task(
                        &state_clone,
                        &wkl_path,
                        &variants,
                        read_only,
                        speed,
                        threshold,
                        &run_id_clone,
                        &task_id,
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
                        state_clone.broadcast(WsMessage::Error {
                            message: e.to_string(),
                        });
                    }
                })
            },
        )
        .await;

    Ok(Json(json!({ "task_id": task_id, "run_id": run_id })))
}

#[allow(clippy::too_many_arguments)]
async fn run_ab_task(
    state: &AppState,
    wkl_path: &str,
    variants: &[VariantDef],
    read_only: bool,
    speed: f64,
    threshold: f64,
    run_id: &str,
    task_id: &str,
) -> anyhow::Result<()> {
    use crate::compare::ab::{compute_ab_comparison, VariantResult};
    use crate::replay::{session::run_replay, ReplayMode};

    let profile = crate::profile::io::read_profile(std::path::Path::new(wkl_path))?;
    let mode = if read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    let mut variant_results = Vec::new();
    for variant in variants {
        let results = run_replay(&profile, &variant.target, mode, speed).await?;
        variant_results.push(VariantResult::from_results(variant.label.clone(), results));

        state.broadcast(WsMessage::ABVariantCompleted {
            task_id: task_id.to_string(),
            label: variant.label.clone(),
        });
    }

    let report = compute_ab_comparison(variant_results, threshold);
    let report_json = serde_json::to_string(&report)?;

    let now = chrono::Utc::now().to_rfc3339();
    let db = state.db.lock().await;
    crate::web::db::update_run_results(
        &db,
        run_id,
        "completed",
        &now,
        None,
        Some(&report_json),
        Some(0),
        None,
    )?;

    state.broadcast(WsMessage::ABCompleted {
        task_id: task_id.to_string(),
        run_id: run_id.to_string(),
    });

    Ok(())
}

/// GET /api/v1/ab/:id
pub async fn get_ab(
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
            Ok(Json(json!({ "run": run, "report": report })))
        }
        Ok(None) => Err(StatusCode::NOT_FOUND),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}
