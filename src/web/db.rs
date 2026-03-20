use anyhow::Result;
use rusqlite::Connection;

/// Initialize the SQLite database with all required tables.
pub fn init_db(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS workloads (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL,
            file_path TEXT NOT NULL,
            source_type TEXT,
            source_host TEXT,
            captured_at TEXT,
            total_sessions INTEGER,
            total_queries INTEGER,
            capture_duration_us INTEGER,
            classification TEXT,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS runs (
            id TEXT PRIMARY KEY,
            run_type TEXT NOT NULL,
            status TEXT NOT NULL DEFAULT 'pending',
            workload_id TEXT,
            config_json TEXT,
            started_at TEXT,
            finished_at TEXT,
            target_conn TEXT,
            replay_mode TEXT,
            speed REAL,
            scale INTEGER,
            results_path TEXT,
            report_json TEXT,
            exit_code INTEGER,
            error_message TEXT,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS proxy_sessions (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            proxy_run_id TEXT,
            session_id INTEGER,
            user_name TEXT,
            database_name TEXT,
            query_count INTEGER DEFAULT 0,
            started_at TEXT,
            ended_at TEXT
        );

        CREATE TABLE IF NOT EXISTS threshold_results (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            run_id TEXT NOT NULL,
            name TEXT NOT NULL,
            passed INTEGER NOT NULL,
            actual REAL NOT NULL,
            threshold_limit REAL NOT NULL
        );

        CREATE TABLE IF NOT EXISTS tuning_reports (
            id TEXT PRIMARY KEY,
            run_id TEXT,
            workload_id TEXT,
            target TEXT NOT NULL,
            provider TEXT NOT NULL,
            hint TEXT,
            iterations INTEGER NOT NULL DEFAULT 0,
            total_improvement_pct REAL NOT NULL DEFAULT 0.0,
            report_json TEXT NOT NULL,
            created_at TEXT DEFAULT (datetime('now'))
        );

        CREATE TABLE IF NOT EXISTS capture_staging (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            capture_id TEXT NOT NULL,
            session_id INTEGER NOT NULL,
            user_name TEXT,
            database_name TEXT,
            sql TEXT NOT NULL,
            kind TEXT,
            start_offset_us INTEGER NOT NULL,
            duration_us INTEGER NOT NULL,
            is_error INTEGER NOT NULL DEFAULT 0,
            error_message TEXT,
            timestamp_us INTEGER NOT NULL
        );

        CREATE INDEX IF NOT EXISTS idx_staging_capture ON capture_staging(capture_id);
        ",
    )?;
    Ok(())
}

// ── Workload CRUD ──────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WorkloadRow {
    pub id: String,
    pub name: String,
    pub file_path: String,
    pub source_type: Option<String>,
    pub source_host: Option<String>,
    pub captured_at: Option<String>,
    pub total_sessions: Option<i64>,
    pub total_queries: Option<i64>,
    pub capture_duration_us: Option<i64>,
    pub classification: Option<String>,
    pub created_at: Option<String>,
}

pub fn insert_workload(conn: &Connection, w: &WorkloadRow) -> Result<()> {
    conn.execute(
        "INSERT INTO workloads (id, name, file_path, source_type, source_host, captured_at,
         total_sessions, total_queries, capture_duration_us, classification)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            w.id,
            w.name,
            w.file_path,
            w.source_type,
            w.source_host,
            w.captured_at,
            w.total_sessions,
            w.total_queries,
            w.capture_duration_us,
            w.classification,
        ],
    )?;
    Ok(())
}

pub fn list_workloads(conn: &Connection) -> Result<Vec<WorkloadRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, file_path, source_type, source_host, captured_at,
         total_sessions, total_queries, capture_duration_us, classification, created_at
         FROM workloads ORDER BY created_at DESC",
    )?;
    let rows = stmt
        .query_map([], |row| {
            Ok(WorkloadRow {
                id: row.get(0)?,
                name: row.get(1)?,
                file_path: row.get(2)?,
                source_type: row.get(3)?,
                source_host: row.get(4)?,
                captured_at: row.get(5)?,
                total_sessions: row.get(6)?,
                total_queries: row.get(7)?,
                capture_duration_us: row.get(8)?,
                classification: row.get(9)?,
                created_at: row.get(10)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_workload(conn: &Connection, id: &str) -> Result<Option<WorkloadRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, name, file_path, source_type, source_host, captured_at,
         total_sessions, total_queries, capture_duration_us, classification, created_at
         FROM workloads WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], |row| {
        Ok(WorkloadRow {
            id: row.get(0)?,
            name: row.get(1)?,
            file_path: row.get(2)?,
            source_type: row.get(3)?,
            source_host: row.get(4)?,
            captured_at: row.get(5)?,
            total_sessions: row.get(6)?,
            total_queries: row.get(7)?,
            capture_duration_us: row.get(8)?,
            classification: row.get(9)?,
            created_at: row.get(10)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn delete_workload(conn: &Connection, id: &str) -> Result<bool> {
    let count = conn.execute("DELETE FROM workloads WHERE id = ?1", [id])?;
    Ok(count > 0)
}

// ── Run CRUD ───────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RunRow {
    pub id: String,
    pub run_type: String,
    pub status: String,
    pub workload_id: Option<String>,
    pub config_json: Option<String>,
    pub started_at: Option<String>,
    pub finished_at: Option<String>,
    pub target_conn: Option<String>,
    pub replay_mode: Option<String>,
    pub speed: Option<f64>,
    pub scale: Option<i64>,
    pub results_path: Option<String>,
    pub report_json: Option<String>,
    pub exit_code: Option<i64>,
    pub error_message: Option<String>,
    pub created_at: Option<String>,
}

pub fn insert_run(conn: &Connection, r: &RunRow) -> Result<()> {
    conn.execute(
        "INSERT INTO runs (id, run_type, status, workload_id, config_json, started_at,
         target_conn, replay_mode, speed, scale)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        rusqlite::params![
            r.id,
            r.run_type,
            r.status,
            r.workload_id,
            r.config_json,
            r.started_at,
            r.target_conn,
            r.replay_mode,
            r.speed,
            r.scale,
        ],
    )?;
    Ok(())
}

pub fn update_run_status(conn: &Connection, id: &str, status: &str) -> Result<()> {
    conn.execute(
        "UPDATE runs SET status = ?2 WHERE id = ?1",
        rusqlite::params![id, status],
    )?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn update_run_results(
    conn: &Connection,
    id: &str,
    status: &str,
    finished_at: &str,
    results_path: Option<&str>,
    report_json: Option<&str>,
    exit_code: Option<i32>,
    error_message: Option<&str>,
) -> Result<()> {
    conn.execute(
        "UPDATE runs SET status = ?2, finished_at = ?3, results_path = ?4,
         report_json = ?5, exit_code = ?6, error_message = ?7 WHERE id = ?1",
        rusqlite::params![
            id,
            status,
            finished_at,
            results_path,
            report_json,
            exit_code,
            error_message
        ],
    )?;
    Ok(())
}

pub fn list_runs(
    conn: &Connection,
    run_type: Option<&str>,
    limit: Option<u32>,
) -> Result<Vec<RunRow>> {
    let limit = limit.unwrap_or(100);
    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(rt) = run_type {
        (
            format!(
                "SELECT id, run_type, status, workload_id, config_json, started_at, finished_at,
                 target_conn, replay_mode, speed, scale, results_path, report_json, exit_code,
                 error_message, created_at
                 FROM runs WHERE run_type = ?1 ORDER BY created_at DESC LIMIT {limit}"
            ),
            vec![Box::new(rt.to_string())],
        )
    } else {
        (
            format!(
                "SELECT id, run_type, status, workload_id, config_json, started_at, finished_at,
                 target_conn, replay_mode, speed, scale, results_path, report_json, exit_code,
                 error_message, created_at
                 FROM runs ORDER BY created_at DESC LIMIT {limit}"
            ),
            vec![],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_refs.as_slice(), |row| {
            Ok(RunRow {
                id: row.get(0)?,
                run_type: row.get(1)?,
                status: row.get(2)?,
                workload_id: row.get(3)?,
                config_json: row.get(4)?,
                started_at: row.get(5)?,
                finished_at: row.get(6)?,
                target_conn: row.get(7)?,
                replay_mode: row.get(8)?,
                speed: row.get(9)?,
                scale: row.get(10)?,
                results_path: row.get(11)?,
                report_json: row.get(12)?,
                exit_code: row.get(13)?,
                error_message: row.get(14)?,
                created_at: row.get(15)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

pub fn get_run(conn: &Connection, id: &str) -> Result<Option<RunRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, run_type, status, workload_id, config_json, started_at, finished_at,
         target_conn, replay_mode, speed, scale, results_path, report_json, exit_code,
         error_message, created_at
         FROM runs WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], |row| {
        Ok(RunRow {
            id: row.get(0)?,
            run_type: row.get(1)?,
            status: row.get(2)?,
            workload_id: row.get(3)?,
            config_json: row.get(4)?,
            started_at: row.get(5)?,
            finished_at: row.get(6)?,
            target_conn: row.get(7)?,
            replay_mode: row.get(8)?,
            speed: row.get(9)?,
            scale: row.get(10)?,
            results_path: row.get(11)?,
            report_json: row.get(12)?,
            exit_code: row.get(13)?,
            error_message: row.get(14)?,
            created_at: row.get(15)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn get_run_stats(conn: &Connection) -> Result<serde_json::Value> {
    let total: i64 = conn.query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))?;
    let passed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE exit_code = 0 AND status = 'completed'",
        [],
        |r| r.get(0),
    )?;
    let failed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE exit_code > 0 AND status = 'completed'",
        [],
        |r| r.get(0),
    )?;
    let running: i64 = conn.query_row(
        "SELECT COUNT(*) FROM runs WHERE status = 'running'",
        [],
        |r| r.get(0),
    )?;

    Ok(serde_json::json!({
        "total": total,
        "passed": passed,
        "failed": failed,
        "running": running,
    }))
}

pub fn get_run_trend(
    conn: &Connection,
    workload_id: Option<&str>,
    limit: Option<u32>,
) -> Result<Vec<serde_json::Value>> {
    let limit = limit.unwrap_or(20);
    let (sql, params): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(wid) =
        workload_id
    {
        (
            format!(
                "SELECT id, run_type, started_at, report_json, exit_code
                 FROM runs WHERE workload_id = ?1 AND status = 'completed' AND report_json IS NOT NULL
                 ORDER BY started_at DESC LIMIT {limit}"
            ),
            vec![Box::new(wid.to_string())],
        )
    } else {
        (
            format!(
                "SELECT id, run_type, started_at, report_json, exit_code
                 FROM runs WHERE status = 'completed' AND report_json IS NOT NULL
                 ORDER BY started_at DESC LIMIT {limit}"
            ),
            vec![],
        )
    };

    let mut stmt = conn.prepare(&sql)?;
    let params_refs: Vec<&dyn rusqlite::types::ToSql> = params.iter().map(|p| p.as_ref()).collect();
    let rows = stmt
        .query_map(params_refs.as_slice(), |row| {
            let id: String = row.get(0)?;
            let run_type: String = row.get(1)?;
            let started_at: Option<String> = row.get(2)?;
            let report_json: Option<String> = row.get(3)?;
            let exit_code: Option<i64> = row.get(4)?;
            Ok(serde_json::json!({
                "id": id,
                "run_type": run_type,
                "started_at": started_at,
                "report": report_json.and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok()),
                "exit_code": exit_code,
            }))
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── Proxy Session CRUD ──────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProxySessionRow {
    pub id: i64,
    pub proxy_run_id: String,
    pub session_id: i64,
    pub user_name: String,
    pub database_name: String,
    pub query_count: i64,
    pub started_at: String,
    pub ended_at: Option<String>,
}

pub fn insert_proxy_session(
    conn: &Connection,
    proxy_run_id: &str,
    session_id: u64,
    user_name: &str,
    database_name: &str,
    started_at: &str,
) -> Result<()> {
    conn.execute(
        "INSERT INTO proxy_sessions (proxy_run_id, session_id, user_name, database_name, started_at)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        rusqlite::params![proxy_run_id, session_id as i64, user_name, database_name, started_at],
    )?;
    Ok(())
}

pub fn update_proxy_session_end(
    conn: &Connection,
    proxy_run_id: &str,
    session_id: u64,
    query_count: u64,
    ended_at: &str,
) -> Result<()> {
    conn.execute(
        "UPDATE proxy_sessions SET query_count = ?3, ended_at = ?4
         WHERE proxy_run_id = ?1 AND session_id = ?2",
        rusqlite::params![
            proxy_run_id,
            session_id as i64,
            query_count as i64,
            ended_at
        ],
    )?;
    Ok(())
}

pub fn list_proxy_sessions(conn: &Connection, proxy_run_id: &str) -> Result<Vec<ProxySessionRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, proxy_run_id, session_id, user_name, database_name, query_count, started_at, ended_at
         FROM proxy_sessions WHERE proxy_run_id = ?1 ORDER BY session_id",
    )?;
    let rows = stmt
        .query_map([proxy_run_id], |row| {
            Ok(ProxySessionRow {
                id: row.get(0)?,
                proxy_run_id: row.get(1)?,
                session_id: row.get(2)?,
                user_name: row.get(3)?,
                database_name: row.get(4)?,
                query_count: row.get(5)?,
                started_at: row.get(6)?,
                ended_at: row.get(7)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── Threshold Result CRUD ───────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ThresholdResultRow {
    pub name: String,
    pub passed: bool,
    pub actual: f64,
    pub threshold_limit: f64,
}

pub fn insert_threshold_results(
    conn: &Connection,
    run_id: &str,
    results: &[ThresholdResultRow],
) -> Result<()> {
    let mut stmt = conn.prepare(
        "INSERT INTO threshold_results (run_id, name, passed, actual, threshold_limit)
         VALUES (?1, ?2, ?3, ?4, ?5)",
    )?;
    for r in results {
        stmt.execute(rusqlite::params![
            run_id,
            r.name,
            r.passed as i32,
            r.actual,
            r.threshold_limit,
        ])?;
    }
    Ok(())
}

pub fn list_threshold_results(conn: &Connection, run_id: &str) -> Result<Vec<ThresholdResultRow>> {
    let mut stmt = conn.prepare(
        "SELECT name, passed, actual, threshold_limit
         FROM threshold_results WHERE run_id = ?1",
    )?;
    let rows = stmt
        .query_map([run_id], |row| {
            let passed_int: i32 = row.get(1)?;
            Ok(ThresholdResultRow {
                name: row.get(0)?,
                passed: passed_int != 0,
                actual: row.get(2)?,
                threshold_limit: row.get(3)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

// ── Tuning Report CRUD ──────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TuningReportRow {
    pub id: String,
    pub run_id: Option<String>,
    pub workload_id: Option<String>,
    pub target: String,
    pub provider: String,
    pub hint: Option<String>,
    pub iterations: i64,
    pub total_improvement_pct: f64,
    pub report_json: String,
    pub created_at: Option<String>,
}

pub fn insert_tuning_report(conn: &Connection, r: &TuningReportRow) -> Result<()> {
    conn.execute(
        "INSERT INTO tuning_reports (id, run_id, workload_id, target, provider, hint,
         iterations, total_improvement_pct, report_json)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        rusqlite::params![
            r.id,
            r.run_id,
            r.workload_id,
            r.target,
            r.provider,
            r.hint,
            r.iterations,
            r.total_improvement_pct,
            r.report_json,
        ],
    )?;
    Ok(())
}

pub fn get_tuning_report(conn: &Connection, id: &str) -> Result<Option<TuningReportRow>> {
    let mut stmt = conn.prepare(
        "SELECT id, run_id, workload_id, target, provider, hint,
         iterations, total_improvement_pct, report_json, created_at
         FROM tuning_reports WHERE id = ?1",
    )?;
    let mut rows = stmt.query_map([id], |row| {
        Ok(TuningReportRow {
            id: row.get(0)?,
            run_id: row.get(1)?,
            workload_id: row.get(2)?,
            target: row.get(3)?,
            provider: row.get(4)?,
            hint: row.get(5)?,
            iterations: row.get(6)?,
            total_improvement_pct: row.get(7)?,
            report_json: row.get(8)?,
            created_at: row.get(9)?,
        })
    })?;
    Ok(rows.next().transpose()?)
}

pub fn list_tuning_reports(conn: &Connection, limit: Option<u32>) -> Result<Vec<TuningReportRow>> {
    let limit = limit.unwrap_or(50);
    let sql = format!(
        "SELECT id, run_id, workload_id, target, provider, hint,
         iterations, total_improvement_pct, report_json, created_at
         FROM tuning_reports ORDER BY created_at DESC LIMIT {limit}"
    );
    let mut stmt = conn.prepare(&sql)?;
    let rows = stmt
        .query_map([], |row| {
            Ok(TuningReportRow {
                id: row.get(0)?,
                run_id: row.get(1)?,
                workload_id: row.get(2)?,
                target: row.get(3)?,
                provider: row.get(4)?,
                hint: row.get(5)?,
                iterations: row.get(6)?,
                total_improvement_pct: row.get(7)?,
                report_json: row.get(8)?,
                created_at: row.get(9)?,
            })
        })?
        .collect::<std::result::Result<Vec<_>, _>>()?;
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_db(&conn).unwrap();
        conn
    }

    #[test]
    fn test_init_db_creates_tables() {
        let conn = test_db();
        // Verify tables exist by querying them
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM workloads", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM runs", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_workload_crud() {
        let conn = test_db();
        let w = WorkloadRow {
            id: "w1".into(),
            name: "test workload".into(),
            file_path: "/tmp/test.wkl".into(),
            source_type: Some("pg-csv".into()),
            source_host: Some("localhost".into()),
            captured_at: Some("2026-03-06T00:00:00Z".into()),
            total_sessions: Some(5),
            total_queries: Some(100),
            capture_duration_us: Some(1_000_000),
            classification: None,
            created_at: None,
        };

        insert_workload(&conn, &w).unwrap();
        let all = list_workloads(&conn).unwrap();
        assert_eq!(all.len(), 1);
        assert_eq!(all[0].name, "test workload");

        let got = get_workload(&conn, "w1").unwrap().unwrap();
        assert_eq!(got.total_sessions, Some(5));

        assert!(delete_workload(&conn, "w1").unwrap());
        assert!(get_workload(&conn, "w1").unwrap().is_none());
    }

    #[test]
    fn test_run_crud() {
        let conn = test_db();
        let r = RunRow {
            id: "r1".into(),
            run_type: "replay".into(),
            status: "running".into(),
            workload_id: Some("w1".into()),
            config_json: None,
            started_at: Some("2026-03-06T00:00:00Z".into()),
            finished_at: None,
            target_conn: Some("postgres://localhost/test".into()),
            replay_mode: Some("ReadWrite".into()),
            speed: Some(1.0),
            scale: Some(1),
            results_path: None,
            report_json: None,
            exit_code: None,
            error_message: None,
            created_at: None,
        };

        insert_run(&conn, &r).unwrap();
        let got = get_run(&conn, "r1").unwrap().unwrap();
        assert_eq!(got.status, "running");

        update_run_status(&conn, "r1", "completed").unwrap();
        let got = get_run(&conn, "r1").unwrap().unwrap();
        assert_eq!(got.status, "completed");

        update_run_results(
            &conn,
            "r1",
            "completed",
            "2026-03-06T00:01:00Z",
            Some("/tmp/results.wkl"),
            Some(r#"{"p95": 100}"#),
            Some(0),
            None,
        )
        .unwrap();
        let got = get_run(&conn, "r1").unwrap().unwrap();
        assert_eq!(got.exit_code, Some(0));
    }

    #[test]
    fn test_list_runs_filtered() {
        let conn = test_db();
        for (id, rt) in [("r1", "replay"), ("r2", "pipeline"), ("r3", "replay")] {
            let r = RunRow {
                id: id.into(),
                run_type: rt.into(),
                status: "completed".into(),
                workload_id: None,
                config_json: None,
                started_at: None,
                finished_at: None,
                target_conn: None,
                replay_mode: None,
                speed: None,
                scale: None,
                results_path: None,
                report_json: None,
                exit_code: None,
                error_message: None,
                created_at: None,
            };
            insert_run(&conn, &r).unwrap();
        }

        let all = list_runs(&conn, None, None).unwrap();
        assert_eq!(all.len(), 3);

        let replays = list_runs(&conn, Some("replay"), None).unwrap();
        assert_eq!(replays.len(), 2);
    }

    #[test]
    fn test_run_stats() {
        let conn = test_db();
        let stats = get_run_stats(&conn).unwrap();
        assert_eq!(stats["total"], 0);
    }

    #[test]
    fn test_proxy_session_crud() {
        let conn = test_db();

        insert_proxy_session(&conn, "proxy-1", 1, "app", "mydb", "2026-03-06T00:00:00Z").unwrap();
        insert_proxy_session(&conn, "proxy-1", 2, "admin", "mydb", "2026-03-06T00:00:01Z").unwrap();

        let sessions = list_proxy_sessions(&conn, "proxy-1").unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].user_name, "app");
        assert_eq!(sessions[1].user_name, "admin");
        assert!(sessions[0].ended_at.is_none());

        update_proxy_session_end(&conn, "proxy-1", 1, 42, "2026-03-06T00:01:00Z").unwrap();
        let sessions = list_proxy_sessions(&conn, "proxy-1").unwrap();
        assert_eq!(sessions[0].query_count, 42);
        assert_eq!(
            sessions[0].ended_at.as_deref(),
            Some("2026-03-06T00:01:00Z")
        );

        // Different proxy_run_id returns empty
        let other = list_proxy_sessions(&conn, "proxy-2").unwrap();
        assert!(other.is_empty());
    }

    #[test]
    fn test_threshold_results_crud() {
        let conn = test_db();

        let results = vec![
            ThresholdResultRow {
                name: "p95_latency".into(),
                passed: true,
                actual: 12.3,
                threshold_limit: 50.0,
            },
            ThresholdResultRow {
                name: "error_rate".into(),
                passed: false,
                actual: 5.5,
                threshold_limit: 1.0,
            },
        ];

        insert_threshold_results(&conn, "run-1", &results).unwrap();
        let got = list_threshold_results(&conn, "run-1").unwrap();
        assert_eq!(got.len(), 2);
        assert_eq!(got[0].name, "p95_latency");
        assert!(got[0].passed);
        assert!(!got[1].passed);
        assert!((got[1].actual - 5.5).abs() < f64::EPSILON);

        // Different run_id returns empty
        let other = list_threshold_results(&conn, "run-2").unwrap();
        assert!(other.is_empty());
    }

    #[test]
    fn test_tuning_report_crud() {
        let conn = test_db();

        let report = TuningReportRow {
            id: "tune-1".into(),
            run_id: Some("run-1".into()),
            workload_id: Some("wkl-1".into()),
            target: "postgres://localhost/test".into(),
            provider: "openai".into(),
            hint: Some("focus on reads".into()),
            iterations: 2,
            total_improvement_pct: 15.5,
            report_json: r#"{"iterations":[]}"#.into(),
            created_at: None,
        };

        insert_tuning_report(&conn, &report).unwrap();

        let got = get_tuning_report(&conn, "tune-1").unwrap().unwrap();
        assert_eq!(got.provider, "openai");
        assert_eq!(got.iterations, 2);
        assert!((got.total_improvement_pct - 15.5).abs() < f64::EPSILON);

        let all = list_tuning_reports(&conn, None).unwrap();
        assert_eq!(all.len(), 1);

        assert!(get_tuning_report(&conn, "tune-999").unwrap().is_none());
    }
}
