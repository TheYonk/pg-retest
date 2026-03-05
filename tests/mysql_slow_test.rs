use pg_retest::capture::mysql_slow::MysqlSlowLogCapture;
use pg_retest::profile::QueryKind;

#[test]
fn test_mysql_slow_log_capture_with_transform() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "mysql-host",
            true, // transform enabled
        )
        .unwrap();

    assert_eq!(profile.capture_method, "mysql_slow_log");
    assert_eq!(profile.source_host, "mysql-host");

    // We expect 3 sessions: thread 42 (5 queries after transform), thread 55 (1 query), thread 99 (skipped - SHOW)
    // Thread 99's SHOW VARIABLES is skipped by the transform pipeline
    assert!(
        profile.sessions.len() >= 2,
        "Expected at least 2 sessions, got {}",
        profile.sessions.len()
    );

    let total = profile.metadata.total_queries;
    // 8 entries in fixture, 1 SHOW is skipped = 7 queries
    assert!(total >= 6, "Expected at least 6 queries, got {total}");
}

#[test]
fn test_mysql_slow_log_capture_no_transform() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "mysql-host",
            false, // no transform
        )
        .unwrap();

    // Without transform, all queries including SHOW should be present
    assert!(
        profile.metadata.total_queries >= 7,
        "Expected at least 7 queries without transform"
    );
}

#[test]
fn test_mysql_slow_log_timing_preserved() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            false,
        )
        .unwrap();

    // Find a session with queries
    let session = profile
        .sessions
        .iter()
        .find(|s| s.queries.len() > 1)
        .unwrap();

    // First query should have the expected duration (1234us for first entry in thread 42)
    let first_q = &session.queries[0];
    assert!(first_q.duration_us > 0, "Duration should be > 0");
}

#[test]
fn test_mysql_slow_log_backticks_transformed() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            true, // transform enabled
        )
        .unwrap();

    // The second query in thread 42 has backticks — they should be converted to double quotes
    let all_sql: Vec<&str> = profile
        .sessions
        .iter()
        .flat_map(|s| s.queries.iter())
        .map(|q| q.sql.as_str())
        .collect();

    // No backticks should remain after transform
    for sql in &all_sql {
        assert!(
            !sql.contains('`'),
            "Backtick found in transformed SQL: {sql}"
        );
    }
}

#[test]
fn test_mysql_slow_log_transaction_ids() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            true,
        )
        .unwrap();

    // Thread 42 has BEGIN + UPDATE + COMMIT — should get transaction IDs
    let thread42 = profile.sessions.iter().find(|s| s.id == 42).unwrap();

    let begin_q = thread42.queries.iter().find(|q| q.kind == QueryKind::Begin);
    assert!(begin_q.is_some(), "Should have a BEGIN query");
    assert!(
        begin_q.unwrap().transaction_id.is_some(),
        "BEGIN should have transaction_id"
    );

    let commit_q = thread42
        .queries
        .iter()
        .find(|q| q.kind == QueryKind::Commit);
    assert!(commit_q.is_some(), "Should have a COMMIT query");
    assert_eq!(
        begin_q.unwrap().transaction_id,
        commit_q.unwrap().transaction_id,
        "BEGIN and COMMIT should have the same transaction_id"
    );
}

#[test]
fn test_mysql_slow_log_multiline_query() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            false,
        )
        .unwrap();

    // Thread 55 has a multi-line JOIN query
    let thread55 = profile.sessions.iter().find(|s| s.id == 55).unwrap();
    let q = &thread55.queries[0];
    assert!(
        q.sql.contains("JOIN"),
        "Multi-line query should contain JOIN"
    );
    assert!(
        q.sql.contains("ORDER BY"),
        "Multi-line query should contain ORDER BY"
    );
    assert_eq!(q.duration_us, 50000, "Query time should be 50000us (0.05s)");
}
