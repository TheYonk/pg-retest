use pg_retest::capture::csv_log::CsvLogCapture;
use pg_retest::profile::QueryKind;
use std::path::Path;

#[test]
fn test_csv_log_capture_parses_sessions() {
    let capture = CsvLogCapture;
    let path = Path::new("tests/fixtures/sample_pg.csv");
    let profile = capture
        .capture_from_file(path, "localhost", "16.2")
        .unwrap();

    assert_eq!(profile.version, 1);
    assert_eq!(profile.capture_method, "csv_log");
    assert_eq!(profile.sessions.len(), 2);
    assert_eq!(profile.metadata.total_queries, 5);
    assert_eq!(profile.metadata.total_sessions, 2);
}

#[test]
fn test_csv_log_capture_session_ordering() {
    let capture = CsvLogCapture;
    let path = Path::new("tests/fixtures/sample_pg.csv");
    let profile = capture
        .capture_from_file(path, "localhost", "16.2")
        .unwrap();

    // Find session for process_id 1234 (session_id 6600a000.4d2)
    // It should have 3 queries, ordered by timestamp
    let session = profile
        .sessions
        .iter()
        .find(|s| s.user == "app_user" && s.queries.len() == 3)
        .expect("Should find app_user session with 3 queries");

    assert_eq!(session.queries[0].kind, QueryKind::Select);
    assert_eq!(session.queries[1].kind, QueryKind::Update);
    assert_eq!(session.queries[2].kind, QueryKind::Select);

    // Verify relative timing: queries should have increasing start offsets
    assert_eq!(session.queries[0].start_offset_us, 0);
    assert!(session.queries[1].start_offset_us > 0);
    assert!(session.queries[2].start_offset_us > session.queries[1].start_offset_us);
}

#[test]
fn test_csv_log_capture_duration_parsing() {
    let capture = CsvLogCapture;
    let path = Path::new("tests/fixtures/sample_pg.csv");
    let profile = capture
        .capture_from_file(path, "localhost", "16.2")
        .unwrap();

    let session = profile
        .sessions
        .iter()
        .find(|s| s.user == "app_user" && s.queries.len() == 3)
        .expect("Should find app_user session");

    // First query: duration 0.450 ms = 450 us
    assert_eq!(session.queries[0].duration_us, 450);
    // Second query: duration 1.200 ms = 1200 us
    assert_eq!(session.queries[1].duration_us, 1200);
}

#[test]
fn test_csv_log_capture_admin_session() {
    let capture = CsvLogCapture;
    let path = Path::new("tests/fixtures/sample_pg.csv");
    let profile = capture
        .capture_from_file(path, "localhost", "16.2")
        .unwrap();

    let session = profile
        .sessions
        .iter()
        .find(|s| s.user == "admin")
        .expect("Should find admin session");

    assert_eq!(session.queries.len(), 2);
    assert_eq!(session.database, "mydb");
    assert_eq!(session.queries[0].kind, QueryKind::Select);
    assert_eq!(session.queries[1].kind, QueryKind::Insert);
}
