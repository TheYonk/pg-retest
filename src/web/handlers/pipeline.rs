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
pub struct StartPipelineRequest {
    pub config_toml: String,
}

#[derive(Deserialize)]
pub struct ValidateRequest {
    pub config_toml: String,
}

/// POST /api/v1/pipeline/validate
pub async fn validate_pipeline(Json(req): Json<ValidateRequest>) -> Json<serde_json::Value> {
    match toml::from_str::<crate::config::PipelineConfig>(&req.config_toml) {
        Ok(config) => Json(json!({
            "valid": true,
            "config": {
                "has_capture": config.capture.is_some(),
                "has_provision": config.provision.is_some(),
                "has_thresholds": config.thresholds.is_some(),
                "variants": config.variants.as_ref().map(|v| v.len()).unwrap_or(0),
            }
        })),
        Err(e) => Json(json!({
            "valid": false,
            "error": e.to_string(),
        })),
    }
}

/// POST /api/v1/pipeline/start
pub async fn start_pipeline(
    State(state): State<AppState>,
    Json(req): Json<StartPipelineRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    let config: crate::config::PipelineConfig =
        toml::from_str(&req.config_toml).map_err(|_| StatusCode::BAD_REQUEST)?;

    let run_id = uuid::Uuid::new_v4().to_string();
    let run_row = crate::web::db::RunRow {
        id: run_id.clone(),
        run_type: "pipeline".into(),
        status: "running".into(),
        workload_id: None,
        config_json: Some(req.config_toml.clone()),
        started_at: Some(chrono::Utc::now().to_rfc3339()),
        finished_at: None,
        target_conn: config.replay.target.clone(),
        replay_mode: Some(
            if config.replay.read_only {
                "ReadOnly"
            } else {
                "ReadWrite"
            }
            .into(),
        ),
        speed: Some(config.replay.speed),
        scale: Some(config.replay.scale as i64),
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

    let state_clone = state.clone();
    let run_id_clone = run_id.clone();

    let task_id = state
        .tasks
        .clone()
        .spawn("pipeline", "Pipeline run", move |_cancel_token, task_id| {
            tokio::spawn(async move {
                state_clone.broadcast(WsMessage::PipelineStageChanged {
                    task_id: task_id.clone(),
                    stage: "starting".into(),
                });

                // Run pipeline in a blocking task since it uses its own runtime
                let config_clone = config.clone();
                let result = tokio::task::spawn_blocking(move || {
                    crate::pipeline::run_pipeline(&config_clone)
                })
                .await;

                let now = chrono::Utc::now().to_rfc3339();
                let db = state_clone.db.lock().await;

                match result {
                    Ok(pipeline_result) => {
                        let report_json = pipeline_result
                            .report
                            .as_ref()
                            .and_then(|r| serde_json::to_string(r).ok());
                        let _ = crate::web::db::update_run_results(
                            &db,
                            &run_id_clone,
                            "completed",
                            &now,
                            None,
                            report_json.as_deref(),
                            Some(pipeline_result.exit_code),
                            None,
                        );

                        // Persist threshold results if report + thresholds available
                        if let Some(ref report) = pipeline_result.report {
                            if let Some(ref thresholds) = config.thresholds {
                                let threshold_results =
                                    crate::compare::threshold::evaluate_thresholds(
                                        report, thresholds,
                                    );
                                let rows: Vec<crate::web::db::ThresholdResultRow> =
                                    threshold_results
                                        .iter()
                                        .map(|r| crate::web::db::ThresholdResultRow {
                                            name: r.name.clone(),
                                            passed: r.passed,
                                            actual: r.actual,
                                            threshold_limit: r.limit,
                                        })
                                        .collect();
                                let _ = crate::web::db::insert_threshold_results(
                                    &db,
                                    &run_id_clone,
                                    &rows,
                                );
                            }
                        }

                        state_clone.broadcast(WsMessage::PipelineCompleted {
                            task_id: task_id.clone(),
                            exit_code: pipeline_result.exit_code,
                        });
                    }
                    Err(e) => {
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
                }
            })
        })
        .await;

    Ok(Json(json!({ "task_id": task_id, "run_id": run_id })))
}

/// GET /api/v1/pipeline/:id  (alias for get_run)
pub async fn get_pipeline(
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
