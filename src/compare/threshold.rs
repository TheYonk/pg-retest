use crate::config::ThresholdConfig;

use super::ComparisonReport;

/// Result of evaluating one threshold check.
#[derive(Debug, Clone)]
pub struct ThresholdResult {
    pub name: String,
    pub passed: bool,
    pub actual: f64,
    pub limit: f64,
    pub message: Option<String>,
}

/// Evaluate all configured thresholds against a comparison report.
pub fn evaluate_thresholds(
    report: &ComparisonReport,
    config: &ThresholdConfig,
) -> Vec<ThresholdResult> {
    let mut results = Vec::new();

    if let Some(limit) = config.p95_max_ms {
        let actual = report.replay_p95_latency_us as f64 / 1000.0;
        results.push(ThresholdResult {
            name: "p95_latency".into(),
            passed: actual <= limit,
            actual,
            limit,
            message: if actual > limit {
                Some(format!(
                    "P95 latency {actual:.1}ms exceeds limit {limit:.1}ms"
                ))
            } else {
                None
            },
        });
    }

    if let Some(limit) = config.p99_max_ms {
        let actual = report.replay_p99_latency_us as f64 / 1000.0;
        results.push(ThresholdResult {
            name: "p99_latency".into(),
            passed: actual <= limit,
            actual,
            limit,
            message: if actual > limit {
                Some(format!(
                    "P99 latency {actual:.1}ms exceeds limit {limit:.1}ms"
                ))
            } else {
                None
            },
        });
    }

    if let Some(limit) = config.error_rate_max_pct {
        let actual = if report.total_queries_replayed > 0 {
            report.total_errors as f64 / report.total_queries_replayed as f64 * 100.0
        } else {
            0.0
        };
        results.push(ThresholdResult {
            name: "error_rate".into(),
            passed: actual <= limit,
            actual,
            limit,
            message: if actual > limit {
                Some(format!("Error rate {actual:.2}% exceeds limit {limit:.1}%"))
            } else {
                None
            },
        });
    }

    if let Some(limit) = config.regression_max_count {
        let actual = report.regressions.len();
        results.push(ThresholdResult {
            name: "regression_count".into(),
            passed: actual <= limit,
            actual: actual as f64,
            limit: limit as f64,
            message: if actual > limit {
                Some(format!("{actual} regressions found, max allowed: {limit}"))
            } else {
                None
            },
        });
    }

    results
}

/// Returns true if all threshold checks passed.
pub fn all_passed(results: &[ThresholdResult]) -> bool {
    results.iter().all(|r| r.passed)
}
