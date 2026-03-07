use axum::{extract::State, http::StatusCode, Json};
use serde::Deserialize;
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, RwLock};

use crate::proxy::capture::CaptureEvent;
use crate::web::db;
use crate::web::state::AppState;
use crate::web::ws::WsMessage;

/// Shared proxy state for tracking running proxy info.
#[derive(Default)]
pub struct ProxyState {
    pub running: bool,
    pub task_id: Option<String>,
    pub listen_addr: Option<String>,
    pub target_addr: Option<String>,
    pub total_queries: u64,
    pub active_sessions: u64,
}

static PROXY_STATE: std::sync::OnceLock<Arc<RwLock<ProxyState>>> = std::sync::OnceLock::new();

fn proxy_state() -> &'static Arc<RwLock<ProxyState>> {
    PROXY_STATE.get_or_init(|| Arc::new(RwLock::new(ProxyState::default())))
}

#[derive(Deserialize)]
pub struct StartProxyRequest {
    pub listen: String,
    pub target: String,
    #[serde(default = "default_pool_size")]
    pub pool_size: usize,
    #[serde(default)]
    pub mask_values: bool,
    #[serde(default)]
    pub no_capture: bool,
}

fn default_pool_size() -> usize {
    100
}

/// GET /api/v1/proxy/status
pub async fn proxy_status(State(_state): State<AppState>) -> Json<serde_json::Value> {
    let ps = proxy_state().read().await;
    Json(json!({
        "running": ps.running,
        "task_id": ps.task_id,
        "listen_addr": ps.listen_addr,
        "target_addr": ps.target_addr,
        "total_queries": ps.total_queries,
        "active_sessions": ps.active_sessions,
    }))
}

/// POST /api/v1/proxy/start
pub async fn start_proxy(
    State(state): State<AppState>,
    Json(req): Json<StartProxyRequest>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    // Check if already running
    {
        let ps = proxy_state().read().await;
        if ps.running {
            return Ok(Json(json!({ "error": "Proxy is already running" })));
        }
    }

    let output_dir = state.data_dir.join("workloads");
    std::fs::create_dir_all(&output_dir).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let output_id = uuid::Uuid::new_v4().to_string();
    let output_path = output_dir.join(format!("proxy-{output_id}.wkl"));

    let config = crate::proxy::ProxyConfig {
        listen_addr: req.listen.clone(),
        target_addr: req.target.clone(),
        output: output_path,
        pool_size: req.pool_size,
        pool_timeout_secs: 30,
        mask_values: req.mask_values,
        no_capture: req.no_capture,
        duration: None,
    };

    let listen = req.listen.clone();
    let target = req.target.clone();
    let state_clone = state.clone();

    let task_id = state
        .tasks
        .clone()
        .spawn(
            "proxy",
            &format!("Proxy {listen} -> {target}"),
            move |cancel_token, task_id| {
                tokio::spawn(async move {
                    // Reset proxy state counters
                    {
                        let mut ps = proxy_state().write().await;
                        ps.running = true;
                        ps.task_id = Some(task_id.clone());
                        ps.listen_addr = Some(listen);
                        ps.target_addr = Some(target);
                        ps.total_queries = 0;
                        ps.active_sessions = 0;
                    }

                    state_clone.broadcast(WsMessage::ProxyStarted {
                        task_id: task_id.clone(),
                    });

                    // Create metrics channel
                    let (metrics_tx, metrics_rx) = mpsc::unbounded_channel();

                    // Spawn metrics consumer
                    let metrics_state = state_clone.clone();
                    let metrics_task_id = task_id.clone();
                    let metrics_handle = tokio::spawn(async move {
                        run_metrics_consumer(metrics_rx, metrics_state, &metrics_task_id).await;
                    });

                    // Run proxy with metrics channel
                    let result =
                        crate::proxy::run_proxy_managed(config, cancel_token, metrics_tx).await;

                    // Wait for metrics consumer to drain
                    let _ = metrics_handle.await;

                    // Handle result — import workload if capture was enabled
                    let workload_id = match result {
                        Ok(Some(profile)) => {
                            // Register captured workload in DB
                            let wid = uuid::Uuid::new_v4().to_string();
                            let w = db::WorkloadRow {
                                id: wid.clone(),
                                name: format!("proxy-{}", &wid[..8]),
                                file_path: profile.metadata.total_queries.to_string(), // placeholder
                                source_type: Some("proxy".into()),
                                source_host: Some(profile.source_host.clone()),
                                captured_at: Some(profile.captured_at.to_rfc3339()),
                                total_sessions: Some(profile.metadata.total_sessions as i64),
                                total_queries: Some(profile.metadata.total_queries as i64),
                                capture_duration_us: Some(
                                    profile.metadata.capture_duration_us as i64,
                                ),
                                classification: None,
                                created_at: None,
                            };
                            let db = state_clone.db.lock().await;
                            let _ = db::insert_workload(&db, &w);
                            Some(wid)
                        }
                        Ok(None) => None,
                        Err(e) => {
                            tracing::error!("Proxy error: {e}");
                            None
                        }
                    };

                    // Update proxy state
                    {
                        let mut ps = proxy_state().write().await;
                        ps.running = false;
                        ps.task_id = None;
                    }

                    state_clone.broadcast(WsMessage::ProxyStopped { workload_id });
                })
            },
        )
        .await;

    Ok(Json(json!({ "task_id": task_id })))
}

/// Consume metrics events from the proxy and update state/DB/WS.
async fn run_metrics_consumer(
    mut metrics_rx: mpsc::UnboundedReceiver<CaptureEvent>,
    state: AppState,
    task_id: &str,
) {
    let mut query_count: u64 = 0;
    let mut active_sessions: u64 = 0;
    let mut last_stats = Instant::now();
    let mut last_qps_count: u64 = 0;
    let mut queries_broadcast_this_second: u64 = 0;
    // Track per-session query counts for SessionEnd
    let mut session_query_counts: HashMap<u64, u64> = HashMap::new();
    // Track per-session user/db for WS messages
    let mut session_info: HashMap<u64, (String, String)> = HashMap::new();

    while let Some(event) = metrics_rx.recv().await {
        match event {
            CaptureEvent::SessionStart {
                session_id,
                ref user,
                ref database,
                ..
            } => {
                active_sessions += 1;
                session_query_counts.insert(session_id, 0);
                session_info.insert(session_id, (user.clone(), database.clone()));

                // Update ProxyState
                {
                    let mut ps = proxy_state().write().await;
                    ps.active_sessions = active_sessions;
                }

                // Broadcast WS
                state.broadcast(WsMessage::ProxySessionOpened {
                    session_id,
                    user: user.clone(),
                    database: database.clone(),
                });

                // Insert into DB
                let now = chrono::Utc::now().to_rfc3339();
                let db = state.db.lock().await;
                let _ = db::insert_proxy_session(&db, task_id, session_id, user, database, &now);
            }
            CaptureEvent::QueryStart {
                session_id,
                ref sql,
                ..
            } => {
                // Rate-limited WS broadcast (max 50/s)
                if queries_broadcast_this_second < 50 {
                    let preview = if sql.len() > 80 {
                        format!("{}...", &sql[..77])
                    } else {
                        sql.clone()
                    };
                    state.broadcast(WsMessage::ProxyQueryExecuted {
                        session_id,
                        sql_preview: preview,
                        duration_us: 0, // Not known yet at QueryStart
                    });
                    queries_broadcast_this_second += 1;
                }
            }
            CaptureEvent::QueryComplete { session_id, .. } => {
                query_count += 1;
                if let Some(count) = session_query_counts.get_mut(&session_id) {
                    *count += 1;
                }

                // Update ProxyState
                {
                    let mut ps = proxy_state().write().await;
                    ps.total_queries = query_count;
                }
            }
            CaptureEvent::QueryError { session_id, .. } => {
                query_count += 1;
                if let Some(count) = session_query_counts.get_mut(&session_id) {
                    *count += 1;
                }
                {
                    let mut ps = proxy_state().write().await;
                    ps.total_queries = query_count;
                }
            }
            CaptureEvent::SessionEnd { session_id } => {
                active_sessions = active_sessions.saturating_sub(1);
                let qcount = session_query_counts.remove(&session_id).unwrap_or(0);
                session_info.remove(&session_id);

                // Update ProxyState
                {
                    let mut ps = proxy_state().write().await;
                    ps.active_sessions = active_sessions;
                }

                // Broadcast WS
                state.broadcast(WsMessage::ProxySessionClosed {
                    session_id,
                    query_count: qcount,
                });

                // Update DB
                let now = chrono::Utc::now().to_rfc3339();
                let db = state.db.lock().await;
                let _ = db::update_proxy_session_end(&db, task_id, session_id, qcount, &now);
            }
        }

        // Every 1 second: broadcast ProxyStats and reset rate limiter
        if last_stats.elapsed() >= Duration::from_secs(1) {
            let elapsed_secs = last_stats.elapsed().as_secs_f64();
            let qps = (query_count - last_qps_count) as f64 / elapsed_secs;
            state.broadcast(WsMessage::ProxyStats {
                active_sessions,
                total_queries: query_count,
                qps,
            });
            last_qps_count = query_count;
            last_stats = Instant::now();
            queries_broadcast_this_second = 0;
        }
    }

    // Final stats broadcast when channel closes
    let elapsed_secs = last_stats.elapsed().as_secs_f64();
    let qps = if elapsed_secs > 0.0 {
        (query_count - last_qps_count) as f64 / elapsed_secs
    } else {
        0.0
    };
    state.broadcast(WsMessage::ProxyStats {
        active_sessions: 0,
        total_queries: query_count,
        qps,
    });
}

/// POST /api/v1/proxy/stop
pub async fn stop_proxy(State(state): State<AppState>) -> Json<serde_json::Value> {
    let task_id = {
        let ps = proxy_state().read().await;
        ps.task_id.clone()
    };

    if let Some(tid) = task_id {
        let cancelled = state.tasks.cancel(&tid).await;
        Json(json!({ "stopped": cancelled }))
    } else {
        Json(json!({ "stopped": false, "error": "No proxy running" }))
    }
}

/// GET /api/v1/proxy/sessions
pub async fn proxy_sessions(State(state): State<AppState>) -> Json<serde_json::Value> {
    let ps = proxy_state().read().await;
    let task_id = ps.task_id.clone().unwrap_or_default();
    let active = ps.active_sessions;
    let total_queries = ps.total_queries;
    drop(ps);

    let db = state.db.lock().await;
    let sessions = db::list_proxy_sessions(&db, &task_id).unwrap_or_default();
    Json(json!({
        "active_sessions": active,
        "total_queries": total_queries,
        "sessions": sessions,
    }))
}

/// POST /api/v1/proxy/toggle-capture
pub async fn toggle_capture(State(_state): State<AppState>) -> Json<serde_json::Value> {
    // Toggle capture mode — placeholder for future integration
    Json(json!({ "toggled": true }))
}
