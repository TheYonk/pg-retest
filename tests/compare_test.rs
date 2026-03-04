use chrono::Utc;
use pg_retest::compare::compute_comparison;
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use pg_retest::replay::{QueryResult, ReplayResults};

fn make_source_profile() -> WorkloadProfile {
    WorkloadProfile {
        version: 1,
        captured_at: Utc::now(),
        source_host: "source".into(),
        pg_version: "16.2".into(),
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
                },
                Query {
                    sql: "SELECT 2".into(),
                    start_offset_us: 500,
                    duration_us: 200,
                    kind: QueryKind::Select,
                },
                Query {
                    sql: "UPDATE t SET x=1".into(),
                    start_offset_us: 1000,
                    duration_us: 300,
                    kind: QueryKind::Update,
                },
                Query {
                    sql: "SELECT 3".into(),
                    start_offset_us: 1500,
                    duration_us: 5000,
                    kind: QueryKind::Select,
                },
            ],
        }],
        metadata: Metadata {
            total_queries: 4,
            total_sessions: 1,
            capture_duration_us: 6500,
        },
    }
}

fn make_replay_results() -> Vec<ReplayResults> {
    vec![ReplayResults {
        session_id: 1,
        query_results: vec![
            QueryResult {
                sql: "SELECT 1".into(),
                original_duration_us: 100,
                replay_duration_us: 80,
                success: true,
                error: None,
            },
            QueryResult {
                sql: "SELECT 2".into(),
                original_duration_us: 200,
                replay_duration_us: 250,
                success: true,
                error: None,
            },
            QueryResult {
                sql: "UPDATE t SET x=1".into(),
                original_duration_us: 300,
                replay_duration_us: 280,
                success: true,
                error: None,
            },
            QueryResult {
                sql: "SELECT 3".into(),
                original_duration_us: 5000,
                replay_duration_us: 4500,
                success: false,
                error: Some("timeout".into()),
            },
        ],
    }]
}

#[test]
fn test_comparison_totals() {
    let source = make_source_profile();
    let results = make_replay_results();
    let report = compute_comparison(&source, &results, 20.0);

    assert_eq!(report.total_queries_source, 4);
    assert_eq!(report.total_queries_replayed, 4);
    assert_eq!(report.total_errors, 1);
}

#[test]
fn test_comparison_avg_latency() {
    let source = make_source_profile();
    let results = make_replay_results();
    let report = compute_comparison(&source, &results, 20.0);

    // Source avg: (100+200+300+5000)/4 = 1400
    assert_eq!(report.source_avg_latency_us, 1400);
    // Replay avg: (80+250+280+4500)/4 = 1277.5, rounds to 1278
    assert_eq!(report.replay_avg_latency_us, 1278);
}

#[test]
fn test_comparison_regressions() {
    let source = make_source_profile();
    let results = make_replay_results();
    let report = compute_comparison(&source, &results, 20.0);

    // SELECT 2: 200 -> 250 = +25% (> 20% threshold = regression)
    assert!(!report.regressions.is_empty());
    let reg = &report.regressions[0];
    assert_eq!(reg.sql, "SELECT 2");
    assert_eq!(reg.original_us, 200);
    assert_eq!(reg.replay_us, 250);
}
