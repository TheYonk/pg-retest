use pg_retest::compare::threshold::{all_passed, evaluate_thresholds};
use pg_retest::compare::{ComparisonReport, Regression};
use pg_retest::config::ThresholdConfig;

fn make_report(
    p95_us: u64,
    p99_us: u64,
    errors: u64,
    replayed: u64,
    regressions: usize,
) -> ComparisonReport {
    ComparisonReport {
        total_queries_source: 100,
        total_queries_replayed: replayed,
        total_errors: errors,
        source_avg_latency_us: 1000,
        replay_avg_latency_us: 2000,
        source_p50_latency_us: 800,
        replay_p50_latency_us: 1600,
        source_p95_latency_us: 5000,
        replay_p95_latency_us: p95_us,
        source_p99_latency_us: 10000,
        replay_p99_latency_us: p99_us,
        total_queries_filtered: 0,
        regressions: (0..regressions)
            .map(|i| Regression {
                sql: format!("SELECT {i}"),
                original_us: 100,
                replay_us: 500,
                change_pct: 400.0,
            })
            .collect(),
    }
}

#[test]
fn test_all_thresholds_pass() {
    let report = make_report(40_000, 150_000, 0, 100, 2);
    let config = ThresholdConfig {
        p95_max_ms: Some(50.0),
        p99_max_ms: Some(200.0),
        error_rate_max_pct: Some(1.0),
        regression_max_count: Some(5),
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert_eq!(results.len(), 4);
    assert!(all_passed(&results));
}

#[test]
fn test_p95_threshold_violation() {
    let report = make_report(60_000, 150_000, 0, 100, 0);
    let config = ThresholdConfig {
        p95_max_ms: Some(50.0),
        p99_max_ms: None,
        error_rate_max_pct: None,
        regression_max_count: None,
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    assert!(results[0].message.as_ref().unwrap().contains("P95"));
}

#[test]
fn test_error_rate_violation() {
    let report = make_report(40_000, 150_000, 5, 100, 0);
    let config = ThresholdConfig {
        p95_max_ms: None,
        p99_max_ms: None,
        error_rate_max_pct: Some(1.0),
        regression_max_count: None,
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert!(!all_passed(&results));
    assert_eq!(results[0].actual, 5.0); // 5/100 = 5%
}

#[test]
fn test_regression_count_violation() {
    let report = make_report(40_000, 150_000, 0, 100, 7);
    let config = ThresholdConfig {
        p95_max_ms: None,
        p99_max_ms: None,
        error_rate_max_pct: None,
        regression_max_count: Some(5),
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert!(!all_passed(&results));
    assert!(results[0]
        .message
        .as_ref()
        .unwrap()
        .contains("7 regressions"));
}

#[test]
fn test_no_thresholds_configured() {
    let report = make_report(999_000, 999_000, 99, 100, 50);
    let config = ThresholdConfig {
        p95_max_ms: None,
        p99_max_ms: None,
        error_rate_max_pct: None,
        regression_max_count: None,
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert!(results.is_empty());
    assert!(all_passed(&results)); // vacuously true
}
