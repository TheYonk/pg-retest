use pg_retest::profile::{Query, QueryKind};
use pg_retest::replay::{QueryResult, ReplayResults, ReplayMode};

#[test]
fn test_replay_mode_read_only_filters_dml() {
    let queries = vec![
        Query {
            sql: "SELECT 1".into(),
            start_offset_us: 0,
            duration_us: 100,
            kind: QueryKind::Select,
        },
        Query {
            sql: "INSERT INTO foo VALUES (1)".into(),
            start_offset_us: 500,
            duration_us: 200,
            kind: QueryKind::Insert,
        },
        Query {
            sql: "SELECT 2".into(),
            start_offset_us: 1000,
            duration_us: 150,
            kind: QueryKind::Select,
        },
    ];

    let filtered: Vec<&Query> = queries
        .iter()
        .filter(|q| ReplayMode::ReadOnly.should_replay(q))
        .collect();

    assert_eq!(filtered.len(), 2);
    assert_eq!(filtered[0].sql, "SELECT 1");
    assert_eq!(filtered[1].sql, "SELECT 2");
}

#[test]
fn test_replay_mode_read_write_keeps_all() {
    let queries = vec![
        Query {
            sql: "SELECT 1".into(),
            start_offset_us: 0,
            duration_us: 100,
            kind: QueryKind::Select,
        },
        Query {
            sql: "INSERT INTO foo VALUES (1)".into(),
            start_offset_us: 500,
            duration_us: 200,
            kind: QueryKind::Insert,
        },
    ];

    let filtered: Vec<&Query> = queries
        .iter()
        .filter(|q| ReplayMode::ReadWrite.should_replay(q))
        .collect();

    assert_eq!(filtered.len(), 2);
}

#[test]
fn test_replay_results_structure() {
    let results = ReplayResults {
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
        ],
    };

    assert_eq!(results.query_results.len(), 2);
    assert!(results.query_results[0].replay_duration_us < results.query_results[0].original_duration_us);
}
