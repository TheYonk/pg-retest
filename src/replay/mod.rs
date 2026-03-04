pub mod session;

use serde::{Deserialize, Serialize};

use crate::profile::Query;

#[derive(Debug, Clone, Copy)]
pub enum ReplayMode {
    ReadWrite,
    ReadOnly,
}

impl ReplayMode {
    pub fn should_replay(&self, query: &Query) -> bool {
        match self {
            ReplayMode::ReadWrite => true,
            ReplayMode::ReadOnly => query.kind.is_read_only(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResults {
    pub session_id: u64,
    pub query_results: Vec<QueryResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub sql: String,
    pub original_duration_us: u64,
    pub replay_duration_us: u64,
    pub success: bool,
    pub error: Option<String>,
}
