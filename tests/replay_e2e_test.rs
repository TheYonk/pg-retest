//! End-to-end replay tests against a real PostgreSQL instance.
//!
//! These tests require a PostgreSQL server running on localhost:5441
//! with database `pg_retest_e2e` accessible by user `sales_demo_app`.
//!
//! If the database is not available, tests are skipped gracefully.

use chrono::Utc;
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use pg_retest::replay::session::{replay_session, run_replay};
use pg_retest::replay::ReplayMode;
use std::time::Duration;
use tokio::time::Instant as TokioInstant;

const CONN_STR: &str =
    "host=localhost port=5441 dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123";

/// Check if the test database is reachable; skip test if not.
async fn require_pg() -> bool {
    match tokio_postgres::connect(CONN_STR, tokio_postgres::NoTls).await {
        Ok((client, conn)) => {
            tokio::spawn(async move {
                let _ = conn.await;
            });
            let _ = client.simple_query("SELECT 1").await;
            true
        }
        Err(_) => {
            eprintln!("SKIP: PostgreSQL not available at localhost:5441");
            false
        }
    }
}

/// Helper to build a simple workload profile from sessions.
fn make_profile(sessions: Vec<Session>) -> WorkloadProfile {
    let total_queries: u64 = sessions.iter().map(|s| s.queries.len() as u64).sum();
    WorkloadProfile {
        version: 2,
        captured_at: Utc::now(),
        source_host: "test".into(),
        pg_version: "17".into(),
        capture_method: "test".into(),
        metadata: Metadata {
            total_queries,
            total_sessions: sessions.len() as u64,
            capture_duration_us: 0,
        },
        sessions,
    }
}

fn make_query(sql: &str, offset_us: u64, kind: QueryKind, txn_id: Option<u64>) -> Query {
    Query {
        sql: sql.to_string(),
        start_offset_us: offset_us,
        duration_us: 100,
        kind,
        transaction_id: txn_id,
    }
}

// ─── Basic replay: SELECTs execute successfully ─────────────────────

#[tokio::test]
async fn test_replay_session_basic_selects() {
    if !require_pg().await {
        return;
    }

    let session = Session {
        id: 1,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            make_query("SELECT 1", 0, QueryKind::Select, None),
            make_query(
                "SELECT count(*) FROM test_orders",
                1000,
                QueryKind::Select,
                None,
            ),
            make_query(
                "SELECT product FROM test_orders WHERE id = 1",
                2000,
                QueryKind::Select,
                None,
            ),
        ],
    };

    let start = TokioInstant::now();
    let results = replay_session(&session, CONN_STR, ReplayMode::ReadWrite, 0.0, start, None)
        .await
        .expect("replay_session should succeed");

    assert_eq!(results.session_id, 1);
    assert_eq!(results.query_results.len(), 3);
    for qr in &results.query_results {
        assert!(qr.success, "Query '{}' failed: {:?}", qr.sql, qr.error);
        assert!(
            qr.replay_duration_us > 0,
            "Query '{}' should have non-zero duration",
            qr.sql
        );
    }
}

// ─── DML replay: INSERT/UPDATE/DELETE modify data ───────────────────

#[tokio::test]
async fn test_replay_session_dml_execution() {
    if !require_pg().await {
        return;
    }

    // Clean up from previous runs, then test DML
    let session = Session {
        id: 2,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            make_query(
                "DELETE FROM test_orders WHERE product = 'e2e_test'",
                0,
                QueryKind::Delete,
                None,
            ),
            make_query(
                "INSERT INTO test_orders (product, quantity) VALUES ('e2e_test', 99)",
                1000,
                QueryKind::Insert,
                None,
            ),
            make_query(
                "UPDATE test_orders SET quantity = 100 WHERE product = 'e2e_test'",
                2000,
                QueryKind::Update,
                None,
            ),
            make_query(
                "SELECT quantity FROM test_orders WHERE product = 'e2e_test'",
                3000,
                QueryKind::Select,
                None,
            ),
            // Clean up
            make_query(
                "DELETE FROM test_orders WHERE product = 'e2e_test'",
                4000,
                QueryKind::Delete,
                None,
            ),
        ],
    };

    let start = TokioInstant::now();
    let results = replay_session(&session, CONN_STR, ReplayMode::ReadWrite, 0.0, start, None)
        .await
        .expect("DML replay should succeed");

    assert_eq!(results.query_results.len(), 5);
    for qr in &results.query_results {
        assert!(qr.success, "DML '{}' failed: {:?}", qr.sql, qr.error);
    }
}

// ─── Transaction replay: BEGIN/COMMIT actually commits ──────────────

#[tokio::test]
async fn test_replay_session_transaction_commit() {
    if !require_pg().await {
        return;
    }

    let session = Session {
        id: 3,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            // Clean up first
            make_query(
                "DELETE FROM test_orders WHERE product = 'txn_test'",
                0,
                QueryKind::Delete,
                None,
            ),
            // Transaction
            make_query("BEGIN", 1000, QueryKind::Begin, Some(1)),
            make_query(
                "INSERT INTO test_orders (product, quantity) VALUES ('txn_test', 42)",
                2000,
                QueryKind::Insert,
                Some(1),
            ),
            make_query("COMMIT", 3000, QueryKind::Commit, Some(1)),
            // Verify data persisted after commit
            make_query(
                "SELECT quantity FROM test_orders WHERE product = 'txn_test'",
                4000,
                QueryKind::Select,
                None,
            ),
            // Clean up
            make_query(
                "DELETE FROM test_orders WHERE product = 'txn_test'",
                5000,
                QueryKind::Delete,
                None,
            ),
        ],
    };

    let start = TokioInstant::now();
    let results = replay_session(&session, CONN_STR, ReplayMode::ReadWrite, 0.0, start, None)
        .await
        .expect("Transaction replay should succeed");

    assert_eq!(results.query_results.len(), 6);
    for qr in &results.query_results {
        assert!(qr.success, "Query '{}' failed: {:?}", qr.sql, qr.error);
    }
}

// ─── Failed transaction auto-rollback ───────────────────────────────

#[tokio::test]
async fn test_replay_session_failed_transaction_auto_rollback() {
    if !require_pg().await {
        return;
    }

    let session = Session {
        id: 4,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            make_query("BEGIN", 0, QueryKind::Begin, Some(1)),
            make_query(
                "INSERT INTO test_orders (product, quantity) VALUES ('rollback_test', 1)",
                1000,
                QueryKind::Insert,
                Some(1),
            ),
            // This will fail: nonexistent_table doesn't exist
            make_query(
                "INSERT INTO nonexistent_table VALUES (1)",
                2000,
                QueryKind::Insert,
                Some(1),
            ),
            // These should be SKIPPED because the transaction failed
            make_query(
                "INSERT INTO test_orders (product, quantity) VALUES ('rollback_test', 2)",
                3000,
                QueryKind::Insert,
                Some(1),
            ),
            make_query("COMMIT", 4000, QueryKind::Commit, Some(1)),
        ],
    };

    let start = TokioInstant::now();
    let results = replay_session(&session, CONN_STR, ReplayMode::ReadWrite, 0.0, start, None)
        .await
        .expect("Replay should complete (with errors)");

    assert_eq!(results.query_results.len(), 5);

    // BEGIN succeeds
    assert!(results.query_results[0].success, "BEGIN should succeed");
    // First INSERT succeeds
    assert!(
        results.query_results[1].success,
        "First INSERT should succeed"
    );
    // Bad INSERT fails
    assert!(!results.query_results[2].success, "Bad INSERT should fail");
    // Remaining queries in the transaction should be skipped
    assert!(
        !results.query_results[3].success,
        "Post-failure INSERT should be skipped"
    );
    assert!(
        !results.query_results[4].success,
        "COMMIT should be skipped after failure"
    );

    // Verify the skipped queries have appropriate error messages
    assert!(
        results.query_results[3]
            .error
            .as_ref()
            .unwrap()
            .contains("skipped"),
        "Skipped query should have 'skipped' in error: {:?}",
        results.query_results[3].error
    );
    assert!(
        results.query_results[4]
            .error
            .as_ref()
            .unwrap()
            .contains("skipped"),
        "Skipped COMMIT should have 'skipped' in error: {:?}",
        results.query_results[4].error
    );

    // Verify rollback actually happened — 'rollback_test' should NOT be in the table
    let (client, conn) = tokio_postgres::connect(CONN_STR, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let rows = client
        .query(
            "SELECT count(*) FROM test_orders WHERE product = 'rollback_test'",
            &[],
        )
        .await
        .unwrap();
    let count: i64 = rows[0].get(0);
    assert_eq!(count, 0, "Rolled-back INSERT should not persist");
}

// ─── Parallel session replay via run_replay() ───────────────────────

#[tokio::test]
async fn test_run_replay_parallel_sessions() {
    if !require_pg().await {
        return;
    }

    let profile = make_profile(vec![
        Session {
            id: 1,
            user: "sales_demo_app".into(),
            database: "pg_retest_e2e".into(),
            queries: vec![
                make_query("SELECT 1", 0, QueryKind::Select, None),
                make_query("SELECT pg_sleep(0.05)", 1000, QueryKind::Select, None),
                make_query("SELECT 2", 2000, QueryKind::Select, None),
            ],
        },
        Session {
            id: 2,
            user: "sales_demo_app".into(),
            database: "pg_retest_e2e".into(),
            queries: vec![
                make_query("SELECT 3", 0, QueryKind::Select, None),
                make_query("SELECT pg_sleep(0.05)", 1000, QueryKind::Select, None),
                make_query("SELECT 4", 2000, QueryKind::Select, None),
            ],
        },
        Session {
            id: 3,
            user: "sales_demo_app".into(),
            database: "pg_retest_e2e".into(),
            queries: vec![
                make_query("SELECT 5", 0, QueryKind::Select, None),
                make_query(
                    "SELECT count(*) FROM test_orders",
                    1000,
                    QueryKind::Select,
                    None,
                ),
            ],
        },
    ]);

    let results = run_replay(&profile, CONN_STR, ReplayMode::ReadWrite, 0.0, None, None)
        .await
        .expect("Parallel replay should succeed");

    // All 3 sessions should produce results
    assert_eq!(results.len(), 3, "Should have results for all 3 sessions");

    // All queries should succeed
    let total_queries: usize = results.iter().map(|r| r.query_results.len()).sum();
    assert_eq!(total_queries, 8, "Should have 8 total query results");

    for session_result in &results {
        for qr in &session_result.query_results {
            assert!(
                qr.success,
                "Session {} query '{}' failed: {:?}",
                session_result.session_id, qr.sql, qr.error
            );
        }
    }
}

// ─── Read-only mode skips DML against real PG ───────────────────────

#[tokio::test]
async fn test_replay_read_only_mode_skips_dml() {
    if !require_pg().await {
        return;
    }

    let profile = make_profile(vec![Session {
        id: 1,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            make_query(
                "SELECT count(*) FROM test_orders",
                0,
                QueryKind::Select,
                None,
            ),
            make_query(
                "INSERT INTO test_orders (product, quantity) VALUES ('readonly_test', 1)",
                1000,
                QueryKind::Insert,
                None,
            ),
            make_query(
                "UPDATE test_orders SET quantity = 999 WHERE product = 'widget'",
                2000,
                QueryKind::Update,
                None,
            ),
            make_query(
                "SELECT product FROM test_orders LIMIT 1",
                3000,
                QueryKind::Select,
                None,
            ),
        ],
    }]);

    let results = run_replay(&profile, CONN_STR, ReplayMode::ReadOnly, 0.0, None, None)
        .await
        .expect("Read-only replay should succeed");

    assert_eq!(results.len(), 1);
    // Only SELECTs should have been replayed
    assert_eq!(
        results[0].query_results.len(),
        2,
        "Only 2 SELECTs should execute in read-only mode"
    );
    for qr in &results[0].query_results {
        assert!(
            qr.sql.starts_with("SELECT"),
            "Only SELECTs should execute: {}",
            qr.sql
        );
        assert!(qr.success, "SELECT should succeed: {:?}", qr.error);
    }

    // Verify INSERT did NOT execute
    let (client, conn) = tokio_postgres::connect(CONN_STR, tokio_postgres::NoTls)
        .await
        .unwrap();
    tokio::spawn(async move {
        let _ = conn.await;
    });
    let rows = client
        .query(
            "SELECT count(*) FROM test_orders WHERE product = 'readonly_test'",
            &[],
        )
        .await
        .unwrap();
    let count: i64 = rows[0].get(0);
    assert_eq!(
        count, 0,
        "INSERT should not have executed in read-only mode"
    );
}

// ─── Speed multiplier affects timing ────────────────────────────────

#[tokio::test]
async fn test_replay_speed_multiplier() {
    if !require_pg().await {
        return;
    }

    // Queries spaced 100ms apart (100_000 us)
    let session = Session {
        id: 1,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            make_query("SELECT 1", 0, QueryKind::Select, None),
            make_query("SELECT 2", 100_000, QueryKind::Select, None),
            make_query("SELECT 3", 200_000, QueryKind::Select, None),
        ],
    };

    // Replay at max speed (speed=0) — should be nearly instant
    let start_fast = std::time::Instant::now();
    let start_tok = TokioInstant::now();
    let _ = replay_session(
        &session,
        CONN_STR,
        ReplayMode::ReadWrite,
        0.0,
        start_tok,
        None,
    )
    .await
    .expect("Fast replay should succeed");
    let fast_elapsed = start_fast.elapsed();

    // Replay at 1x speed — should take ~200ms (the offset of the last query)
    let start_normal = std::time::Instant::now();
    let start_tok2 = TokioInstant::now();
    let _ = replay_session(
        &session,
        CONN_STR,
        ReplayMode::ReadWrite,
        1.0,
        start_tok2,
        None,
    )
    .await
    .expect("Normal speed replay should succeed");
    let normal_elapsed = start_normal.elapsed();

    // 1x speed should take noticeably longer than max speed
    // (connection setup adds overhead, so we compare relative, not absolute)
    assert!(
        normal_elapsed >= Duration::from_millis(150),
        "1x speed replay should be >=150ms (queries spaced 100ms apart), was {:?}",
        normal_elapsed
    );
    assert!(
        normal_elapsed > fast_elapsed,
        "1x speed ({:?}) should be slower than max speed ({:?})",
        normal_elapsed,
        fast_elapsed
    );
}

// ─── Connection error handling ──────────────────────────────────────

#[tokio::test]
async fn test_replay_session_bad_connection_string() {
    // This doesn't need PG — it tests error handling for unreachable target
    let session = Session {
        id: 1,
        user: "test".into(),
        database: "test".into(),
        queries: vec![make_query("SELECT 1", 0, QueryKind::Select, None)],
    };

    let start = TokioInstant::now();
    let result = replay_session(
        &session,
        "host=localhost port=59999 dbname=nonexistent user=nobody",
        ReplayMode::ReadWrite,
        0.0,
        start,
        None,
    )
    .await;

    assert!(result.is_err(), "Should fail with bad connection string");
}

// ─── Query error inside session doesn't crash replay ────────────────

#[tokio::test]
async fn test_replay_session_query_error_continues() {
    if !require_pg().await {
        return;
    }

    // Mix of valid and invalid queries (no transaction — errors are independent)
    let session = Session {
        id: 1,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            make_query("SELECT 1", 0, QueryKind::Select, None),
            make_query(
                "SELECT * FROM this_table_does_not_exist_12345",
                1000,
                QueryKind::Select,
                None,
            ),
            make_query("SELECT 2", 2000, QueryKind::Select, None),
        ],
    };

    let start = TokioInstant::now();
    let results = replay_session(&session, CONN_STR, ReplayMode::ReadWrite, 0.0, start, None)
        .await
        .expect("Replay should complete despite query errors");

    assert_eq!(results.query_results.len(), 3);
    assert!(results.query_results[0].success, "SELECT 1 should succeed");
    assert!(
        !results.query_results[1].success,
        "Bad table query should fail"
    );
    assert!(
        results.query_results[1].error.is_some(),
        "Failed query should have error message"
    );
    assert!(
        results.query_results[2].success,
        "SELECT 2 should still succeed after prior error"
    );
}

// ─── Multiple failed transactions in one session ────────────────────

#[tokio::test]
async fn test_replay_session_multiple_transactions() {
    if !require_pg().await {
        return;
    }

    let session = Session {
        id: 5,
        user: "sales_demo_app".into(),
        database: "pg_retest_e2e".into(),
        queries: vec![
            // Transaction 1: succeeds
            make_query("BEGIN", 0, QueryKind::Begin, Some(1)),
            make_query("SELECT 1", 1000, QueryKind::Select, Some(1)),
            make_query("COMMIT", 2000, QueryKind::Commit, Some(1)),
            // Transaction 2: fails
            make_query("BEGIN", 3000, QueryKind::Begin, Some(2)),
            make_query(
                "INSERT INTO nonexistent_xyz VALUES (1)",
                4000,
                QueryKind::Insert,
                Some(2),
            ),
            make_query("SELECT 2", 5000, QueryKind::Select, Some(2)),
            make_query("COMMIT", 6000, QueryKind::Commit, Some(2)),
            // Transaction 3: succeeds (after failed txn 2)
            make_query("BEGIN", 7000, QueryKind::Begin, Some(3)),
            make_query("SELECT 3", 8000, QueryKind::Select, Some(3)),
            make_query("COMMIT", 9000, QueryKind::Commit, Some(3)),
        ],
    };

    let start = TokioInstant::now();
    let results = replay_session(&session, CONN_STR, ReplayMode::ReadWrite, 0.0, start, None)
        .await
        .expect("Multi-transaction replay should complete");

    assert_eq!(results.query_results.len(), 10);

    // Transaction 1: all succeed
    assert!(results.query_results[0].success, "Txn1 BEGIN");
    assert!(results.query_results[1].success, "Txn1 SELECT");
    assert!(results.query_results[2].success, "Txn1 COMMIT");

    // Transaction 2: BEGIN succeeds, INSERT fails, rest skipped
    assert!(results.query_results[3].success, "Txn2 BEGIN");
    assert!(
        !results.query_results[4].success,
        "Txn2 bad INSERT should fail"
    );
    assert!(
        !results.query_results[5].success,
        "Txn2 SELECT should be skipped"
    );
    assert!(
        !results.query_results[6].success,
        "Txn2 COMMIT should be skipped"
    );

    // Transaction 3: should succeed (clean state after txn 2 failure)
    assert!(
        results.query_results[7].success,
        "Txn3 BEGIN should succeed"
    );
    assert!(
        results.query_results[8].success,
        "Txn3 SELECT should succeed"
    );
    assert!(
        results.query_results[9].success,
        "Txn3 COMMIT should succeed"
    );
}
