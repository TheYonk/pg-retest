use axum::{
    extract::{
        ws::{Message, WebSocket},
        State, WebSocketUpgrade,
    },
    response::IntoResponse,
};
use futures_util::{SinkExt, StreamExt};
use serde::Serialize;

use super::state::AppState;

/// All WebSocket message types sent from server to client.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum WsMessage {
    // Proxy events
    ProxyStarted {
        task_id: String,
    },
    ProxyStopped {
        workload_id: Option<String>,
    },
    ProxySessionOpened {
        session_id: u64,
        user: String,
        database: String,
    },
    ProxySessionClosed {
        session_id: u64,
        query_count: u64,
    },
    ProxyQueryExecuted {
        session_id: u64,
        sql_preview: String,
        duration_us: u64,
    },
    ProxyStats {
        active_sessions: u64,
        total_queries: u64,
        qps: f64,
    },

    // Replay events
    ReplayProgress {
        task_id: String,
        completed: u64,
        total: u64,
        pct: f64,
    },
    ReplayCompleted {
        task_id: String,
        run_id: String,
    },
    ReplayFailed {
        task_id: String,
        error: String,
    },

    // Pipeline events
    PipelineStageChanged {
        task_id: String,
        stage: String,
    },
    PipelineCompleted {
        task_id: String,
        exit_code: i32,
    },

    // A/B events
    ABVariantCompleted {
        task_id: String,
        label: String,
    },
    ABCompleted {
        task_id: String,
        run_id: String,
    },

    // General
    TaskStatusChanged {
        task_id: String,
        status: String,
    },
    Error {
        message: String,
    },
}

/// Axum handler for WebSocket upgrade.
pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let mut rx = state.ws_tx.subscribe();

    // Forward broadcast messages to this WebSocket client
    let send_task = tokio::spawn(async move {
        while let Ok(msg) = rx.recv().await {
            if let Ok(json) = serde_json::to_string(&msg) {
                if sender.send(Message::Text(json.into())).await.is_err() {
                    break;
                }
            }
        }
    });

    // Read from client (we just drain messages; client doesn't send commands)
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = receiver.next().await {
            if matches!(msg, Message::Close(_)) {
                break;
            }
        }
    });

    // Wait for either task to finish (client disconnect)
    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
}
