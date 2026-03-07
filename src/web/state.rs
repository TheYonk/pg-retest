use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::{broadcast, Mutex};

use super::tasks::TaskManager;
use super::ws::WsMessage;

/// Shared application state for the web server.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub data_dir: PathBuf,
    pub ws_tx: broadcast::Sender<WsMessage>,
    pub tasks: Arc<TaskManager>,
}

impl AppState {
    pub fn new(db: Connection, data_dir: PathBuf) -> Self {
        let (ws_tx, _) = broadcast::channel(1024);
        Self {
            db: Arc::new(Mutex::new(db)),
            data_dir,
            ws_tx,
            tasks: Arc::new(TaskManager::new()),
        }
    }

    /// Broadcast a WebSocket message to all connected clients.
    pub fn broadcast(&self, msg: WsMessage) {
        // Ignore error if no receivers are connected
        let _ = self.ws_tx.send(msg);
    }
}
