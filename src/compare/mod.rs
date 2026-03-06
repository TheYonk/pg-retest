pub mod ab;
pub mod capacity;
pub mod junit;
pub mod report;
pub mod threshold;

use serde::{Deserialize, Serialize};

use crate::profile::WorkloadProfile;
use crate::replay::ReplayResults;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub total_queries_source: u64,
    pub total_queries_replayed: u64,
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
) -> ComparisonReport {
    let mut source_durations: Vec<u64> = Vec::new();
    let mut replay_durations: Vec<u64> = Vec::new();
    let mut regressions = Vec::new();
    let mut total_errors: u64 = 0;

    // Collect all original durations from source
    for session in &source.sessions {
        for query in &session.queries {
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
