use chrono::Utc;
use pg_retest::profile::io;
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use tempfile::NamedTempFile;

#[test]
fn test_profile_roundtrip_messagepack() {
    let profile = WorkloadProfile {
        version: 1,
        captured_at: Utc::now(),
        source_host: "localhost".into(),
        pg_version: "16.2".into(),
        capture_method: "csv_log".into(),
        sessions: vec![
            Session {
                id: 1,
                user: "app_user".into(),
                database: "mydb".into(),
                queries: vec![
                    Query {
                        sql: "SELECT 1".into(),
                        start_offset_us: 0,
                        duration_us: 500,
                        kind: QueryKind::Select,
                    },
                    Query {
                        sql: "UPDATE users SET name = 'test' WHERE id = 1".into(),
                        start_offset_us: 1000,
                        duration_us: 1200,
                        kind: QueryKind::Update,
                    },
                ],
            },
            Session {
                id: 2,
                user: "admin".into(),
                database: "mydb".into(),
                queries: vec![Query {
                    sql: "SELECT count(*) FROM orders".into(),
                    start_offset_us: 200,
                    duration_us: 3000,
                    kind: QueryKind::Select,
                }],
            },
        ],
        metadata: Metadata {
            total_queries: 3,
            total_sessions: 2,
            capture_duration_us: 5000,
        },
    };

    let file = NamedTempFile::new().unwrap();
    let path = file.path();

    io::write_profile(path, &profile).unwrap();
    let loaded = io::read_profile(path).unwrap();

    assert_eq!(loaded.version, 1);
    assert_eq!(loaded.source_host, "localhost");
    assert_eq!(loaded.pg_version, "16.2");
    assert_eq!(loaded.capture_method, "csv_log");
    assert_eq!(loaded.sessions.len(), 2);
    assert_eq!(loaded.sessions[0].queries.len(), 2);
    assert_eq!(loaded.sessions[0].queries[0].sql, "SELECT 1");
    assert_eq!(loaded.sessions[0].queries[0].kind, QueryKind::Select);
    assert_eq!(loaded.sessions[0].queries[1].kind, QueryKind::Update);
    assert_eq!(loaded.sessions[1].queries[0].duration_us, 3000);
    assert_eq!(loaded.metadata.total_queries, 3);
}

#[test]
fn test_query_kind_classification() {
    assert_eq!(
        QueryKind::from_sql("SELECT * FROM users"),
        QueryKind::Select
    );
    assert_eq!(
        QueryKind::from_sql("select count(*) from orders"),
        QueryKind::Select
    );
    assert_eq!(
        QueryKind::from_sql("INSERT INTO users VALUES (1)"),
        QueryKind::Insert
    );
    assert_eq!(
        QueryKind::from_sql("UPDATE users SET x=1"),
        QueryKind::Update
    );
    assert_eq!(
        QueryKind::from_sql("DELETE FROM users WHERE id=1"),
        QueryKind::Delete
    );
    assert_eq!(
        QueryKind::from_sql("CREATE TABLE foo (id int)"),
        QueryKind::Ddl
    );
    assert_eq!(
        QueryKind::from_sql("ALTER TABLE foo ADD COLUMN bar text"),
        QueryKind::Ddl
    );
    assert_eq!(QueryKind::from_sql("DROP TABLE foo"), QueryKind::Ddl);
    assert_eq!(QueryKind::from_sql("VACUUM users"), QueryKind::Other);
    assert_eq!(QueryKind::from_sql("BEGIN"), QueryKind::Other);
    assert_eq!(QueryKind::from_sql("COMMIT"), QueryKind::Other);
}
