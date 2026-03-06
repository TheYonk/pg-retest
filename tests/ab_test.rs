use pg_retest::compare::ab::{compute_ab_comparison, VariantResult};
use pg_retest::replay::{QueryResult, ReplayResults};

fn mock_variant_results(label: &str, latencies: &[u64]) -> VariantResult {
    let results = vec![ReplayResults {
        session_id: 1,
        query_results: latencies
            .iter()
            .enumerate()
            .map(|(i, &lat)| QueryResult {
                sql: format!("SELECT {}", i + 1),
                original_duration_us: 100,
                replay_duration_us: lat,
                success: true,
                error: None,
            })
            .collect(),
    }];
    VariantResult::from_results(label.to_string(), results)
}

#[test]
fn test_variant_result_stats() {
    let v = mock_variant_results("pg16", &[100, 200, 300, 400, 500]);
    assert_eq!(v.label, "pg16");
    assert_eq!(v.total_queries, 5);
    assert_eq!(v.total_errors, 0);
    assert_eq!(v.avg_latency_us, 300);
}

#[test]
fn test_ab_comparison_two_variants() {
    let baseline = mock_variant_results("pg16-default", &[100, 200, 300]);
    let tuned = mock_variant_results("pg16-tuned", &[80, 150, 250]);

    let report = compute_ab_comparison(vec![baseline, tuned], 20.0);

    assert_eq!(report.baseline_label, "pg16-default");
    assert_eq!(report.variants.len(), 2);
    assert!(report.variants[1].avg_latency_us < report.variants[0].avg_latency_us);
}

#[test]
fn test_ab_comparison_detects_regressions() {
    let baseline = mock_variant_results("fast", &[100, 100, 100]);
    let slow = mock_variant_results("slow", &[100, 100, 500]);

    let report = compute_ab_comparison(vec![baseline, slow], 20.0);
    assert!(!report.regressions.is_empty());
    assert!(report.regressions[0].change_pct > 100.0);
}

#[test]
fn test_ab_comparison_detects_improvements() {
    let baseline = mock_variant_results("slow", &[500, 500, 500]);
    let fast = mock_variant_results("fast", &[100, 100, 100]);

    let report = compute_ab_comparison(vec![baseline, fast], 20.0);
    assert!(!report.improvements.is_empty());
}

#[test]
fn test_ab_winner_determination() {
    let slow = mock_variant_results("slow", &[500, 500, 500]);
    let fast = mock_variant_results("fast", &[100, 100, 100]);

    let report = compute_ab_comparison(vec![slow, fast], 20.0);
    assert_eq!(report.winner().unwrap(), "fast");
}
