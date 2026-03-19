use std::path::PathBuf;
use std::sync::Arc;

use rusqlite::Connection;
use tokio::sync::{broadcast, Mutex};

use super::tasks::TaskManager;
use super::ws::WsMessage;

/// Configuration for demo mode, parsed from environment variables.
#[derive(Clone, Debug)]
pub struct DemoConfig {
    pub db_a: String,
    pub db_b: String,
    pub workload_path: PathBuf,
    pub init_sql_path: PathBuf,
}

impl DemoConfig {
    pub fn from_env() -> Option<Self> {
        let enabled = std::env::var("PG_RETEST_DEMO").unwrap_or_default();
        if enabled != "true" {
            return None;
        }
        let db_a = std::env::var("DEMO_DB_A").unwrap_or_default();
        let db_b = std::env::var("DEMO_DB_B").unwrap_or_default();
        let workload =
            std::env::var("DEMO_WORKLOAD").unwrap_or_else(|_| "/demo/workload.wkl".to_string());
        if db_a.is_empty() || db_b.is_empty() {
            return None;
        }
        let workload_path = PathBuf::from(&workload);
        let init_sql_path = workload_path
            .parent()
            .unwrap_or(std::path::Path::new("/demo"))
            .join("init-db-b.sql");
        Some(Self {
            db_a,
            db_b,
            workload_path,
            init_sql_path,
        })
    }
}

/// Shared application state for the web server.
#[derive(Clone)]
pub struct AppState {
    pub db: Arc<Mutex<Connection>>,
    pub data_dir: PathBuf,
    pub ws_tx: broadcast::Sender<WsMessage>,
    pub tasks: Arc<TaskManager>,
    pub demo_config: Option<DemoConfig>,
}

impl AppState {
    pub fn new(db: Connection, data_dir: PathBuf, demo_config: Option<DemoConfig>) -> Self {
        let (ws_tx, _) = broadcast::channel(1024);
        Self {
            db: Arc::new(Mutex::new(db)),
            data_dir,
            ws_tx,
            tasks: Arc::new(TaskManager::new()),
            demo_config,
        }
    }

    /// Broadcast a WebSocket message to all connected clients.
    pub fn broadcast(&self, msg: WsMessage) {
        // Ignore error if no receivers are connected
        let _ = self.ws_tx.send(msg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_demo_config_from_env_disabled() {
        std::env::remove_var("PG_RETEST_DEMO");
        let config = DemoConfig::from_env();
        assert!(config.is_none());
    }

    #[test]
    fn test_demo_config_from_env_enabled() {
        // Note: these tests modify env vars, which is not thread-safe
        // but acceptable for unit tests run with --test-threads=1
        std::env::set_var("PG_RETEST_DEMO", "true");
        std::env::set_var(
            "DEMO_DB_A",
            "host=db-a dbname=ecommerce user=demo password=demo",
        );
        std::env::set_var(
            "DEMO_DB_B",
            "host=db-b dbname=ecommerce user=demo password=demo",
        );
        std::env::set_var("DEMO_WORKLOAD", "/demo/workload.wkl");
        let config = DemoConfig::from_env();
        assert!(config.is_some());
        let c = config.unwrap();
        assert!(c.db_a.contains("db-a"));
        assert!(c.db_b.contains("db-b"));
        assert_eq!(c.workload_path, PathBuf::from("/demo/workload.wkl"));
        assert_eq!(c.init_sql_path, PathBuf::from("/demo/init-db-b.sql"));
        std::env::remove_var("PG_RETEST_DEMO");
        std::env::remove_var("DEMO_DB_A");
        std::env::remove_var("DEMO_DB_B");
        std::env::remove_var("DEMO_WORKLOAD");
    }
}
