use serde::{Deserialize, Serialize};

/// A single tuning recommendation from the LLM.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Recommendation {
    ConfigChange {
        parameter: String,
        current_value: String,
        recommended_value: String,
        rationale: String,
    },
    CreateIndex {
        table: String,
        columns: Vec<String>,
        index_type: Option<String>,
        sql: String,
        rationale: String,
    },
    QueryRewrite {
        original_sql: String,
        rewritten_sql: String,
        rationale: String,
    },
    SchemaChange {
        sql: String,
        description: String,
        rationale: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedChange {
    pub recommendation: Recommendation,
    pub success: bool,
    pub error: Option<String>,
    pub rollback_sql: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonSummary {
    pub p50_change_pct: f64,
    pub p95_change_pct: f64,
    pub p99_change_pct: f64,
    pub regressions: usize,
    pub improvements: usize,
    pub errors_delta: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningIteration {
    pub iteration: u32,
    pub recommendations: Vec<Recommendation>,
    pub applied: Vec<AppliedChange>,
    pub comparison: Option<ComparisonSummary>,
    pub llm_feedback: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningReport {
    pub workload: String,
    pub target: String,
    pub provider: String,
    pub hint: Option<String>,
    pub iterations: Vec<TuningIteration>,
    pub total_improvement_pct: f64,
    pub all_changes: Vec<AppliedChange>,
}

#[derive(Debug, Clone)]
pub struct TuningConfig {
    pub workload_path: std::path::PathBuf,
    pub target: String,
    pub provider: String,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub max_iterations: u32,
    pub hint: Option<String>,
    pub apply: bool,
    pub force: bool,
    pub speed: f64,
    pub read_only: bool,
}

/// Events emitted during tuning for real-time progress reporting.
#[derive(Debug, Clone)]
pub enum TuningEvent {
    /// Baseline replay started.
    BaselineStarted,
    /// A new iteration has started.
    IterationStarted { iteration: u32, max_iterations: u32 },
    /// LLM returned recommendations.
    RecommendationsReceived { iteration: u32, recommendations: Vec<Recommendation> },
    /// A single change was applied (success or failure).
    ChangeApplied { iteration: u32, change: AppliedChange },
    /// Replay after changes completed with comparison.
    ReplayCompleted { iteration: u32, comparison: ComparisonSummary },
    /// Rollback started due to regression.
    RollbackStarted { iteration: u32 },
    /// Rollback completed with results.
    RollbackCompleted { iteration: u32, rolled_back: u32, failed: u32 },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recommendation_json_roundtrip() {
        let recs = vec![
            Recommendation::ConfigChange {
                parameter: "shared_buffers".into(),
                current_value: "128MB".into(),
                recommended_value: "1GB".into(),
                rationale: "More memory".into(),
            },
            Recommendation::CreateIndex {
                table: "orders".into(),
                columns: vec!["status".into(), "created_at".into()],
                index_type: Some("btree".into()),
                sql: "CREATE INDEX idx_orders_status ON orders (status, created_at)".into(),
                rationale: "Frequent filter".into(),
            },
            Recommendation::QueryRewrite {
                original_sql: "SELECT * FROM orders".into(),
                rewritten_sql: "SELECT id, status FROM orders".into(),
                rationale: "Select only needed columns".into(),
            },
            Recommendation::SchemaChange {
                sql: "ALTER TABLE orders ADD COLUMN archived boolean DEFAULT false".into(),
                description: "Add archive flag".into(),
                rationale: "Partition active/archived".into(),
            },
        ];
        let json = serde_json::to_string(&recs).unwrap();
        let parsed: Vec<Recommendation> = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.len(), 4);
    }

    #[test]
    fn test_tuning_report_serialization() {
        let report = TuningReport {
            workload: "test.wkl".into(),
            target: "postgresql://localhost/test".into(),
            provider: "claude".into(),
            hint: Some("focus on reads".into()),
            iterations: vec![],
            total_improvement_pct: 0.0,
            all_changes: vec![],
        };
        let json = serde_json::to_string_pretty(&report).unwrap();
        let parsed: TuningReport = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.workload, "test.wkl");
        assert_eq!(parsed.hint, Some("focus on reads".into()));
    }
}
