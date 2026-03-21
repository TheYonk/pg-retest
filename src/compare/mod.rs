pub mod ab;
pub mod capacity;
pub mod junit;
pub mod report;
pub mod threshold;

use serde::{Deserialize, Serialize};

use crate::profile::WorkloadProfile;
use crate::replay::{ReplayMode, ReplayResults};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub total_queries_source: u64,
    pub total_queries_replayed: u64,
    pub total_queries_filtered: u64,
    pub total_errors: u64,
    pub source_avg_latency_us: u64,
    pub replay_avg_latency_us: u64,
    pub source_p50_latency_us: u64,
    pub replay_p50_latency_us: u64,
    pub source_p95_latency_us: u64,
    pub replay_p95_latency_us: u64,
    pub source_p99_latency_us: u64,
    pub replay_p99_latency_us: u64,
    pub regressions: Vec<Regression>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Regression {
    pub sql: String,
    pub original_us: u64,
    pub replay_us: u64,
    pub change_pct: f64,
}

pub fn compute_comparison(
    source: &WorkloadProfile,
    results: &[ReplayResults],
    threshold_pct: f64,
    mode: Option<ReplayMode>,
) -> ComparisonReport {
    let mut source_durations: Vec<u64> = Vec::new();
    let mut replay_durations: Vec<u64> = Vec::new();
    let mut regressions = Vec::new();
    let mut total_errors: u64 = 0;
    let mut total_queries_filtered: u64 = 0;

    // Collect original durations from source, filtering by replay mode if set
    for session in &source.sessions {
        for query in &session.queries {
            if let Some(ref m) = mode {
                if !m.should_replay(query) {
                    total_queries_filtered += 1;
                    continue;
                }
            }
            source_durations.push(query.duration_us);
        }
    }

    // Collect replay durations and detect regressions
    for result in results {
        for qr in &result.query_results {
            replay_durations.push(qr.replay_duration_us);

            if !qr.success {
                total_errors += 1;
            }

            if qr.original_duration_us > 0 {
                let change_pct = ((qr.replay_duration_us as f64 - qr.original_duration_us as f64)
                    / qr.original_duration_us as f64)
                    * 100.0;

                if change_pct > threshold_pct {
                    regressions.push(Regression {
                        sql: qr.sql.clone(),
                        original_us: qr.original_duration_us,
                        replay_us: qr.replay_duration_us,
                        change_pct,
                    });
                }
            }
        }
    }

    // Sort regressions by severity (worst first)
    regressions.sort_by(|a, b| b.change_pct.partial_cmp(&a.change_pct).unwrap());

    source_durations.sort();
    replay_durations.sort();

    ComparisonReport {
        total_queries_source: source_durations.len() as u64,
        total_queries_replayed: replay_durations.len() as u64,
        total_queries_filtered,
        total_errors,
        source_avg_latency_us: avg(&source_durations),
        replay_avg_latency_us: avg(&replay_durations),
        source_p50_latency_us: percentile(&source_durations, 50),
        replay_p50_latency_us: percentile(&replay_durations, 50),
        source_p95_latency_us: percentile(&source_durations, 95),
        replay_p95_latency_us: percentile(&replay_durations, 95),
        source_p99_latency_us: percentile(&source_durations, 99),
        replay_p99_latency_us: percentile(&replay_durations, 99),
        regressions,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompareOutcome {
    Pass,
    Regressions,
    Errors,
}

impl CompareOutcome {
    pub fn exit_code(&self) -> i32 {
        match self {
            CompareOutcome::Pass => 0,
            CompareOutcome::Regressions => 1,
            CompareOutcome::Errors => 2,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            CompareOutcome::Pass => "PASS",
            CompareOutcome::Regressions => "FAIL (regressions detected)",
            CompareOutcome::Errors => "FAIL (query errors detected)",
        }
    }
}

pub fn evaluate_outcome(
    report: &ComparisonReport,
    fail_on_regression: bool,
    fail_on_error: bool,
) -> CompareOutcome {
    // Errors take priority over regressions
    if fail_on_error && report.total_errors > 0 {
        return CompareOutcome::Errors;
    }
    if fail_on_regression && !report.regressions.is_empty() {
        return CompareOutcome::Regressions;
    }
    CompareOutcome::Pass
}

fn avg(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    (values.iter().sum::<u64>() as f64 / values.len() as f64).round() as u64
}

fn percentile(sorted: &[u64], pct: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((pct as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
    use crate::replay::QueryResult;
    use chrono::Utc;

    #[test]
    fn test_filtered_percentiles_read_only_mode() {
        // Source has 3 SELECTs and 2 DML queries
        let source = WorkloadProfile {
            version: 2,
            captured_at: Utc::now(),
            source_host: "src".into(),
            pg_version: "16".into(),
            capture_method: "csv_log".into(),
            sessions: vec![Session {
                id: 1,
                user: "app".into(),
                database: "db".into(),
                queries: vec![
                    Query {
                        sql: "SELECT 1".into(),
                        start_offset_us: 0,
                        duration_us: 100,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "INSERT INTO t VALUES (1)".into(),
                        start_offset_us: 200,
                        duration_us: 9000,
                        kind: QueryKind::Insert,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT 2".into(),
                        start_offset_us: 400,
                        duration_us: 200,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "UPDATE t SET x=1".into(),
                        start_offset_us: 600,
                        duration_us: 8000,
                        kind: QueryKind::Update,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT 3".into(),
                        start_offset_us: 800,
                        duration_us: 300,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                ],
            }],
            metadata: Metadata {
                total_queries: 5,
                total_sessions: 1,
                capture_duration_us: 1000,
            },
        };

        // Replay results only contain SELECTs (ReadOnly mode skipped DML)
        let results = vec![ReplayResults {
            session_id: 1,
            query_results: vec![
                QueryResult {
                    sql: "SELECT 1".into(),
                    original_duration_us: 100,
                    replay_duration_us: 90,
                    success: true,
                    error: None,
                },
                QueryResult {
                    sql: "SELECT 2".into(),
                    original_duration_us: 200,
                    replay_duration_us: 180,
                    success: true,
                    error: None,
                },
                QueryResult {
                    sql: "SELECT 3".into(),
                    original_duration_us: 300,
                    replay_duration_us: 310,
                    success: true,
                    error: None,
                },
            ],
        }];

        // Without mode filter: source includes all 5 queries (with high DML durations)
        let report_unfiltered = compute_comparison(&source, &results, 20.0, None);
        assert_eq!(report_unfiltered.total_queries_source, 5);
        assert_eq!(report_unfiltered.total_queries_filtered, 0);
        // Source avg includes DML: (100+9000+200+8000+300)/5 = 3520
        assert_eq!(report_unfiltered.source_avg_latency_us, 3520);

        // With ReadOnly mode: source only includes 3 SELECTs
        let report_filtered =
            compute_comparison(&source, &results, 20.0, Some(ReplayMode::ReadOnly));
        assert_eq!(report_filtered.total_queries_source, 3);
        assert_eq!(report_filtered.total_queries_replayed, 3);
        assert_eq!(report_filtered.total_queries_filtered, 2);
        // Source avg is only SELECTs: (100+200+300)/3 = 200
        assert_eq!(report_filtered.source_avg_latency_us, 200);

        // With ReadWrite mode: no filtering, same as None
        let report_rw = compute_comparison(&source, &results, 20.0, Some(ReplayMode::ReadWrite));
        assert_eq!(report_rw.total_queries_source, 5);
        assert_eq!(report_rw.total_queries_filtered, 0);
    }
}
