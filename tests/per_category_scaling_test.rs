use chrono::Utc;
use pg_retest::classify::WorkloadClass;
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use pg_retest::replay::scaling::scale_sessions_by_class;
use std::collections::HashMap;

fn make_profile(sessions: Vec<Session>) -> WorkloadProfile {
    let total_queries = sessions.iter().map(|s| s.queries.len() as u64).sum();
    let total_sessions = sessions.len() as u64;
    WorkloadProfile {
        version: 2,
        captured_at: Utc::now(),
        source_host: "test".into(),
        pg_version: "16.2".into(),
        capture_method: "csv_log".into(),
        sessions,
        metadata: Metadata {
            total_queries,
            total_sessions,
            capture_duration_us: 10000,
        },
    }
}

/// Build an analytical session: >80% reads, avg latency >10ms
fn analytical_session(id: u64) -> Session {
    Session {
        id,
        user: "analyst".into(),
        database: "analytics".into(),
        queries: vec![
            Query {
                sql: "SELECT * FROM large_table".into(),
                start_offset_us: 0,
                duration_us: 50_000,
                kind: QueryKind::Select,
                transaction_id: None,
            },
            Query {
                sql: "SELECT * FROM another_table".into(),
                start_offset_us: 100_000,
                duration_us: 30_000,
                kind: QueryKind::Select,
                transaction_id: None,
            },
        ],
    }
}

/// Build a transactional session: >20% writes, avg latency <5ms, >2 transactions
fn transactional_session(id: u64) -> Session {
    Session {
        id,
        user: "app".into(),
        database: "oltp".into(),
        queries: vec![
            Query {
                sql: "BEGIN".into(),
                start_offset_us: 0,
                duration_us: 50,
                kind: QueryKind::Begin,
                transaction_id: Some(1),
            },
            Query {
                sql: "INSERT INTO orders VALUES (1)".into(),
                start_offset_us: 100,
                duration_us: 500,
                kind: QueryKind::Insert,
                transaction_id: Some(1),
            },
            Query {
                sql: "COMMIT".into(),
                start_offset_us: 700,
                duration_us: 50,
                kind: QueryKind::Commit,
                transaction_id: Some(1),
            },
            Query {
                sql: "BEGIN".into(),
                start_offset_us: 1000,
                duration_us: 50,
                kind: QueryKind::Begin,
                transaction_id: Some(2),
            },
            Query {
                sql: "UPDATE orders SET status = 'shipped'".into(),
                start_offset_us: 1100,
                duration_us: 800,
                kind: QueryKind::Update,
                transaction_id: Some(2),
            },
            Query {
                sql: "SELECT id FROM orders".into(),
                start_offset_us: 2000,
                duration_us: 300,
                kind: QueryKind::Select,
                transaction_id: Some(2),
            },
            Query {
                sql: "COMMIT".into(),
                start_offset_us: 2500,
                duration_us: 50,
                kind: QueryKind::Commit,
                transaction_id: Some(2),
            },
            Query {
                sql: "BEGIN".into(),
                start_offset_us: 3000,
                duration_us: 50,
                kind: QueryKind::Begin,
                transaction_id: Some(3),
            },
            Query {
                sql: "DELETE FROM old_orders WHERE created < now()".into(),
                start_offset_us: 3100,
                duration_us: 400,
                kind: QueryKind::Delete,
                transaction_id: Some(3),
            },
            Query {
                sql: "COMMIT".into(),
                start_offset_us: 3600,
                duration_us: 50,
                kind: QueryKind::Commit,
                transaction_id: Some(3),
            },
        ],
    }
}

#[test]
fn test_scale_by_class_analytical_2x_transactional_4x() {
    let profile = make_profile(vec![
        analytical_session(1),
        analytical_session(2),
        transactional_session(3),
    ]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 2u32);
    class_scales.insert(WorkloadClass::Transactional, 4);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 0);
    // 2 analytical * 2x = 4, 1 transactional * 4x = 4, total = 8
    assert_eq!(scaled.len(), 8);
}

#[test]
fn test_scale_by_class_zero_excludes() {
    let profile = make_profile(vec![analytical_session(1), transactional_session(2)]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 0u32);
    class_scales.insert(WorkloadClass::Transactional, 1);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 0);
    // Analytical excluded (0x), transactional kept (1x) = 1
    assert_eq!(scaled.len(), 1);
    assert_eq!(scaled[0].user, "app");
}

#[test]
fn test_scale_by_class_stagger() {
    let profile = make_profile(vec![analytical_session(1)]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 3u32);
    class_scales.insert(WorkloadClass::Transactional, 1);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 500);
    assert_eq!(scaled.len(), 3);
    // First copy: original offsets
    assert_eq!(scaled[0].queries[0].start_offset_us, 0);
    // Second copy: +500ms stagger
    assert_eq!(scaled[1].queries[0].start_offset_us, 500_000);
    // Third copy: +1000ms stagger
    assert_eq!(scaled[2].queries[0].start_offset_us, 1_000_000);
}

#[test]
fn test_scale_by_class_all_same_class() {
    let profile = make_profile(vec![analytical_session(1), analytical_session(2)]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 2u32);
    class_scales.insert(WorkloadClass::Transactional, 1);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 0);
    // 2 sessions * 2x = 4
    assert_eq!(scaled.len(), 4);
}
