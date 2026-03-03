# pg-retest Milestone 1 Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Rust CLI tool that captures PostgreSQL workload from CSV logs, replays it against a target PG instance, and produces a side-by-side performance comparison report.

**Architecture:** Single binary with clap subcommands (`capture`, `replay`, `compare`, `inspect`). Capture uses a pluggable backend trait (CSV log parser for M1). Replay uses Tokio async with one connection per captured session. Profile format is MessagePack (`.wkl`).

**Tech Stack:** Rust, Tokio, tokio-postgres, clap (derive), rmp-serde, serde, csv, tabled, tracing, anyhow/thiserror

---

### Task 1: Project Scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `src/main.rs`
- Create: `.gitignore`

**Step 1: Initialize git repo**

Run: `cd /Users/matt.yonkovit/yonk-tools/pg-retest && git init`

**Step 2: Create .gitignore**

```gitignore
/target
*.wkl
*.swp
.DS_Store
```

**Step 3: Create Cargo.toml**

```toml
[package]
name = "pg-retest"
version = "0.1.0"
edition = "2021"
description = "Capture, replay, and compare PostgreSQL workloads"

[dependencies]
anyhow = "1"
chrono = { version = "0.4", features = ["serde"] }
clap = { version = "4", features = ["derive"] }
csv = "1"
rmp-serde = "1"
serde = { version = "1", features = ["derive"] }
serde_json = "1"
tabled = "0.17"
thiserror = "2"
tokio = { version = "1", features = ["full"] }
tokio-postgres = { version = "0.7", features = ["with-chrono-0_4"] }
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }

[dev-dependencies]
tempfile = "3"
```

**Step 4: Create minimal main.rs**

```rust
fn main() {
    println!("pg-retest");
}
```

**Step 5: Build to verify**

Run: `cargo build`
Expected: Compiles successfully

**Step 6: Commit**

```bash
git add .gitignore Cargo.toml src/main.rs CLAUDE.md docs/
git commit -m "feat: initialize pg-retest project scaffolding"
```

---

### Task 2: Profile Types (Core Data Model)

**Files:**
- Create: `src/profile/mod.rs`
- Create: `src/profile/io.rs`
- Modify: `src/main.rs` (add module declaration)
- Create: `tests/profile_io_test.rs`

**Step 1: Write the failing test for profile serialization round-trip**

Create `tests/profile_io_test.rs`:

```rust
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use pg_retest::profile::io;
use chrono::Utc;
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
                queries: vec![
                    Query {
                        sql: "SELECT count(*) FROM orders".into(),
                        start_offset_us: 200,
                        duration_us: 3000,
                        kind: QueryKind::Select,
                    },
                ],
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
    assert_eq!(QueryKind::from_sql("SELECT * FROM users"), QueryKind::Select);
    assert_eq!(QueryKind::from_sql("select count(*) from orders"), QueryKind::Select);
    assert_eq!(QueryKind::from_sql("INSERT INTO users VALUES (1)"), QueryKind::Insert);
    assert_eq!(QueryKind::from_sql("UPDATE users SET x=1"), QueryKind::Update);
    assert_eq!(QueryKind::from_sql("DELETE FROM users WHERE id=1"), QueryKind::Delete);
    assert_eq!(QueryKind::from_sql("CREATE TABLE foo (id int)"), QueryKind::Ddl);
    assert_eq!(QueryKind::from_sql("ALTER TABLE foo ADD COLUMN bar text"), QueryKind::Ddl);
    assert_eq!(QueryKind::from_sql("DROP TABLE foo"), QueryKind::Ddl);
    assert_eq!(QueryKind::from_sql("VACUUM users"), QueryKind::Other);
    assert_eq!(QueryKind::from_sql("BEGIN"), QueryKind::Other);
    assert_eq!(QueryKind::from_sql("COMMIT"), QueryKind::Other);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test profile_io_test`
Expected: FAIL — module `profile` not found

**Step 3: Create profile types**

Create `src/profile/mod.rs`:

```rust
pub mod io;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadProfile {
    pub version: u8,
    pub captured_at: DateTime<Utc>,
    pub source_host: String,
    pub pg_version: String,
    pub capture_method: String,
    pub sessions: Vec<Session>,
    pub metadata: Metadata,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: u64,
    pub user: String,
    pub database: String,
    pub queries: Vec<Query>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Query {
    pub sql: String,
    pub start_offset_us: u64,
    pub duration_us: u64,
    pub kind: QueryKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum QueryKind {
    Select,
    Insert,
    Update,
    Delete,
    Ddl,
    Other,
}

impl QueryKind {
    pub fn from_sql(sql: &str) -> Self {
        let trimmed = sql.trim_start().to_uppercase();
        if trimmed.starts_with("SELECT") || trimmed.starts_with("WITH") {
            QueryKind::Select
        } else if trimmed.starts_with("INSERT") {
            QueryKind::Insert
        } else if trimmed.starts_with("UPDATE") {
            QueryKind::Update
        } else if trimmed.starts_with("DELETE") {
            QueryKind::Delete
        } else if trimmed.starts_with("CREATE")
            || trimmed.starts_with("ALTER")
            || trimmed.starts_with("DROP")
            || trimmed.starts_with("TRUNCATE")
        {
            QueryKind::Ddl
        } else {
            QueryKind::Other
        }
    }

    pub fn is_read_only(&self) -> bool {
        matches!(self, QueryKind::Select)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Metadata {
    pub total_queries: u64,
    pub total_sessions: u64,
    pub capture_duration_us: u64,
}
```

**Step 4: Create profile I/O**

Create `src/profile/io.rs`:

```rust
use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

use super::WorkloadProfile;

pub fn write_profile(path: &Path, profile: &WorkloadProfile) -> Result<()> {
    let bytes = rmp_serde::to_vec(profile)
        .context("Failed to serialize workload profile to MessagePack")?;
    fs::write(path, bytes)
        .with_context(|| format!("Failed to write profile to {}", path.display()))?;
    Ok(())
}

pub fn read_profile(path: &Path) -> Result<WorkloadProfile> {
    let bytes = fs::read(path)
        .with_context(|| format!("Failed to read profile from {}", path.display()))?;
    let profile: WorkloadProfile = rmp_serde::from_slice(&bytes)
        .context("Failed to deserialize workload profile from MessagePack")?;
    Ok(profile)
}
```

**Step 5: Update main.rs to expose modules as library**

Replace `src/main.rs`:

```rust
pub mod profile;

fn main() {
    println!("pg-retest");
}
```

**Step 6: Run tests to verify they pass**

Run: `cargo test --test profile_io_test`
Expected: 2 tests PASS

**Step 7: Commit**

```bash
git add src/profile/ tests/profile_io_test.rs src/main.rs
git commit -m "feat: add workload profile types and MessagePack I/O"
```

---

### Task 3: CSV Log Capture Backend

**Files:**
- Create: `src/capture/mod.rs`
- Create: `src/capture/csv_log.rs`
- Create: `tests/capture_csv_test.rs`
- Create: `tests/fixtures/sample_pg.csv`
- Modify: `src/main.rs` (add module)

**Step 1: Create a sample PG CSV log fixture**

Create `tests/fixtures/sample_pg.csv`. PG CSV log columns (PG 14-16):
log_time, user_name, database_name, process_id, connection_from, session_id, session_line_num, command_tag, session_start_time, virtual_transaction_id, transaction_id, error_severity, sql_state_code, message, detail, hint, internal_query, internal_query_pos, context, query, query_pos, location, application_name, backend_type, leader_pid, query_id

```csv
2024-03-08 10:00:00.100 UTC,app_user,mydb,1234,192.168.1.10:54321,6600a000.4d2,1,SELECT,2024-03-08 09:59:50.000 UTC,3/100,0,LOG,00000,"duration: 0.450 ms  statement: SELECT id, name FROM users WHERE active = true",,,,,,,,psql,client backend,,0
2024-03-08 10:00:00.200 UTC,app_user,mydb,1234,192.168.1.10:54321,6600a000.4d2,2,UPDATE,2024-03-08 09:59:50.000 UTC,3/101,500,LOG,00000,"duration: 1.200 ms  statement: UPDATE users SET last_login = now() WHERE id = 42",,,,,,,,psql,client backend,,0
2024-03-08 10:00:00.150 UTC,admin,mydb,5678,192.168.1.10:54322,6600a000.162e,1,SELECT,2024-03-08 09:59:55.000 UTC,4/50,0,LOG,00000,"duration: 3.000 ms  statement: SELECT count(*) FROM orders WHERE created_at > '2024-01-01'",,,,,,,,psql,client backend,,0
2024-03-08 10:00:01.000 UTC,admin,mydb,5678,192.168.1.10:54322,6600a000.162e,2,INSERT,2024-03-08 09:59:55.000 UTC,4/51,501,LOG,00000,"duration: 0.800 ms  statement: INSERT INTO audit_log (action, ts) VALUES ('check_orders', now())",,,,,,,,psql,client backend,,0
2024-03-08 10:00:00.500 UTC,app_user,mydb,1234,192.168.1.10:54321,6600a000.4d2,3,SELECT,2024-03-08 09:59:50.000 UTC,3/102,0,LOG,00000,"duration: 2.100 ms  statement: SELECT o.id, o.total FROM orders o JOIN users u ON o.user_id = u.id WHERE u.id = 42",,,,,,,,psql,client backend,,0
```

**Step 2: Write the failing test**

Create `tests/capture_csv_test.rs`:

```rust
use pg_retest::capture::csv_log::CsvLogCapture;
use pg_retest::profile::QueryKind;
use std::path::Path;

#[test]
fn test_csv_log_capture_parses_sessions() {
    let capture = CsvLogCapture;
    let path = Path::new("tests/fixtures/sample_pg.csv");
    let profile = capture.capture_from_file(path, "localhost", "16.2").unwrap();

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
    let profile = capture.capture_from_file(path, "localhost", "16.2").unwrap();

    // Find session for process_id 1234 (session_id 6600a000.4d2)
    // It should have 3 queries, ordered by timestamp
    let session = profile.sessions.iter()
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
    let profile = capture.capture_from_file(path, "localhost", "16.2").unwrap();

    let session = profile.sessions.iter()
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
    let profile = capture.capture_from_file(path, "localhost", "16.2").unwrap();

    let session = profile.sessions.iter()
        .find(|s| s.user == "admin")
        .expect("Should find admin session");

    assert_eq!(session.queries.len(), 2);
    assert_eq!(session.database, "mydb");
    assert_eq!(session.queries[0].kind, QueryKind::Select);
    assert_eq!(session.queries[1].kind, QueryKind::Insert);
}
```

**Step 3: Run tests to verify they fail**

Run: `cargo test --test capture_csv_test`
Expected: FAIL — module `capture` not found

**Step 4: Create capture module trait**

Create `src/capture/mod.rs`:

```rust
pub mod csv_log;
```

**Step 5: Create CSV log parser**

Create `src/capture/csv_log.rs`:

```rust
use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};

use crate::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};

pub struct CsvLogCapture;

/// A raw parsed log entry before grouping into sessions.
struct LogEntry {
    log_time: DateTime<Utc>,
    user_name: String,
    database_name: String,
    session_id: String,
    duration_us: u64,
    sql: String,
}

impl CsvLogCapture {
    pub fn capture_from_file(
        &self,
        path: &Path,
        source_host: &str,
        pg_version: &str,
    ) -> Result<WorkloadProfile> {
        let entries = self.parse_csv(path)?;
        self.build_profile(entries, source_host, pg_version)
    }

    fn parse_csv(&self, path: &Path) -> Result<Vec<LogEntry>> {
        let mut reader = csv::ReaderBuilder::new()
            .has_headers(false)
            .flexible(true)
            .from_path(path)
            .with_context(|| format!("Failed to open CSV log: {}", path.display()))?;

        let mut entries = Vec::new();

        for result in reader.records() {
            let record = result.context("Failed to read CSV record")?;

            // PG CSV log fields (0-indexed):
            // 0: log_time, 1: user_name, 2: database_name, 3: process_id,
            // 4: connection_from, 5: session_id, 6: session_line_num,
            // 7: command_tag, 8: session_start_time, 9: virtual_transaction_id,
            // 10: transaction_id, 11: error_severity, 12: sql_state_code,
            // 13: message, ...

            let severity = record.get(11).unwrap_or("");
            if severity != "LOG" {
                continue;
            }

            let message = match record.get(13) {
                Some(msg) => msg,
                None => continue,
            };

            // Parse "duration: X.XXX ms  statement: SQL..."
            let (duration_us, sql) = match parse_duration_statement(message) {
                Some(parsed) => parsed,
                None => continue,
            };

            let log_time = record
                .get(0)
                .unwrap_or("")
                .parse::<DateTime<Utc>>()
                .or_else(|_| {
                    // Try parsing without timezone suffix variations
                    let ts = record.get(0).unwrap_or("");
                    let ts = ts.trim();
                    chrono::NaiveDateTime::parse_from_str(
                        ts.trim_end_matches(" UTC"),
                        "%Y-%m-%d %H:%M:%S%.f",
                    )
                    .map(|ndt| ndt.and_utc())
                })
                .unwrap_or_else(|_| Utc::now());

            entries.push(LogEntry {
                log_time,
                user_name: record.get(1).unwrap_or("").to_string(),
                database_name: record.get(2).unwrap_or("").to_string(),
                session_id: record.get(5).unwrap_or("").to_string(),
                duration_us,
                sql,
            });
        }

        Ok(entries)
    }

    fn build_profile(
        &self,
        entries: Vec<LogEntry>,
        source_host: &str,
        pg_version: &str,
    ) -> Result<WorkloadProfile> {
        // Group by session_id
        let mut session_map: HashMap<String, Vec<LogEntry>> = HashMap::new();
        for entry in entries {
            session_map
                .entry(entry.session_id.clone())
                .or_default()
                .push(entry);
        }

        let mut sessions = Vec::new();
        let mut total_queries: u64 = 0;
        let mut session_counter: u64 = 0;
        let mut global_min_time: Option<DateTime<Utc>> = None;
        let mut global_max_time: Option<DateTime<Utc>> = None;

        for (_session_id, mut entries) in session_map {
            if entries.is_empty() {
                continue;
            }

            // Sort by log_time within session
            entries.sort_by_key(|e| e.log_time);

            let first_time = entries[0].log_time;
            let user = entries[0].user_name.clone();
            let database = entries[0].database_name.clone();

            // Track global time bounds
            for e in &entries {
                match global_min_time {
                    None => global_min_time = Some(e.log_time),
                    Some(t) if e.log_time < t => global_min_time = Some(e.log_time),
                    _ => {}
                }
                match global_max_time {
                    None => global_max_time = Some(e.log_time),
                    Some(t) if e.log_time > t => global_max_time = Some(e.log_time),
                    _ => {}
                }
            }

            let queries: Vec<Query> = entries
                .iter()
                .map(|e| {
                    let offset = (e.log_time - first_time)
                        .num_microseconds()
                        .unwrap_or(0) as u64;
                    Query {
                        sql: e.sql.clone(),
                        start_offset_us: offset,
                        duration_us: e.duration_us,
                        kind: QueryKind::from_sql(&e.sql),
                    }
                })
                .collect();

            total_queries += queries.len() as u64;
            session_counter += 1;

            sessions.push(Session {
                id: session_counter,
                user,
                database,
                queries,
            });
        }

        // Sort sessions by their first query time for deterministic output
        sessions.sort_by_key(|s| s.queries.first().map(|q| q.start_offset_us).unwrap_or(0));

        let capture_duration_us = match (global_min_time, global_max_time) {
            (Some(min), Some(max)) => (max - min).num_microseconds().unwrap_or(0) as u64,
            _ => 0,
        };

        Ok(WorkloadProfile {
            version: 1,
            captured_at: Utc::now(),
            source_host: source_host.to_string(),
            pg_version: pg_version.to_string(),
            capture_method: "csv_log".to_string(),
            sessions,
            metadata: Metadata {
                total_queries,
                total_sessions: session_counter,
                capture_duration_us,
            },
        })
    }
}

/// Parse PG log message format: "duration: X.XXX ms  statement: SQL..."
fn parse_duration_statement(message: &str) -> Option<(u64, String)> {
    let message = message.trim();

    if !message.starts_with("duration:") {
        return None;
    }

    // Find "statement:" separator
    let stmt_marker = "statement: ";
    let stmt_pos = message.find(stmt_marker)?;
    let sql = message[stmt_pos + stmt_marker.len()..].to_string();

    // Parse duration between "duration: " and " ms"
    let dur_start = "duration: ".len();
    let ms_pos = message.find(" ms")?;
    let dur_str = &message[dur_start..ms_pos];
    let dur_ms: f64 = dur_str.trim().parse().ok()?;
    let dur_us = (dur_ms * 1000.0).round() as u64;

    if sql.is_empty() {
        return None;
    }

    Some((dur_us, sql))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_duration_statement() {
        let (dur, sql) = parse_duration_statement(
            "duration: 1.234 ms  statement: SELECT * FROM users",
        )
        .unwrap();
        assert_eq!(dur, 1234);
        assert_eq!(sql, "SELECT * FROM users");
    }

    #[test]
    fn test_parse_duration_statement_sub_ms() {
        let (dur, sql) = parse_duration_statement(
            "duration: 0.045 ms  statement: SELECT 1",
        )
        .unwrap();
        assert_eq!(dur, 45);
        assert_eq!(sql, "SELECT 1");
    }

    #[test]
    fn test_parse_duration_statement_rejects_non_duration() {
        assert!(parse_duration_statement("connection authorized: user=app").is_none());
        assert!(parse_duration_statement("").is_none());
    }
}
```

**Step 6: Update main.rs**

```rust
pub mod capture;
pub mod profile;

fn main() {
    println!("pg-retest");
}
```

**Step 7: Run tests**

Run: `cargo test --test capture_csv_test && cargo test --lib capture`
Expected: All tests PASS

**Step 8: Commit**

```bash
git add src/capture/ tests/capture_csv_test.rs tests/fixtures/
git commit -m "feat: add CSV log capture backend with PG log parser"
```

---

### Task 4: CLI Structure

**Files:**
- Create: `src/cli.rs`
- Modify: `src/main.rs`

**Step 1: Create CLI arg structs**

Create `src/cli.rs`:

```rust
use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pg-retest")]
#[command(version, about = "Capture, replay, and compare PostgreSQL workloads")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Capture workload from PostgreSQL logs
    Capture(CaptureArgs),

    /// Replay a captured workload against a target database
    Replay(ReplayArgs),

    /// Compare source workload with replay results
    Compare(CompareArgs),

    /// Inspect a workload profile file
    Inspect(InspectArgs),
}

#[derive(clap::Args)]
pub struct CaptureArgs {
    /// Path to PostgreSQL CSV log file
    #[arg(long)]
    pub source_log: PathBuf,

    /// Output workload profile path (.wkl)
    #[arg(short, long, default_value = "workload.wkl")]
    pub output: PathBuf,

    /// Source host identifier (for metadata)
    #[arg(long, default_value = "unknown")]
    pub source_host: String,

    /// PostgreSQL version (for metadata)
    #[arg(long, default_value = "unknown")]
    pub pg_version: String,
}

#[derive(clap::Args)]
pub struct ReplayArgs {
    /// Path to workload profile (.wkl)
    #[arg(long)]
    pub workload: PathBuf,

    /// Target PostgreSQL connection string
    #[arg(long)]
    pub target: String,

    /// Output results profile path (.wkl)
    #[arg(short, long, default_value = "results.wkl")]
    pub output: PathBuf,

    /// Replay only SELECT queries (strip DML)
    #[arg(long, default_value_t = false)]
    pub read_only: bool,

    /// Speed multiplier (e.g., 2.0 = 2x faster)
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,
}

#[derive(clap::Args)]
pub struct CompareArgs {
    /// Source workload profile (.wkl)
    #[arg(long)]
    pub source: PathBuf,

    /// Replay results profile (.wkl)
    #[arg(long)]
    pub replay: PathBuf,

    /// Output JSON report path
    #[arg(long)]
    pub json: Option<PathBuf>,

    /// Regression threshold percentage (flag queries slower by this %)
    #[arg(long, default_value_t = 20.0)]
    pub threshold: f64,
}

#[derive(clap::Args)]
pub struct InspectArgs {
    /// Path to workload profile (.wkl)
    pub path: PathBuf,
}
```

**Step 2: Wire up main.rs**

```rust
pub mod capture;
pub mod cli;
pub mod profile;

use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match cli.command {
        Commands::Capture(args) => cmd_capture(args),
        Commands::Replay(args) => cmd_replay(args),
        Commands::Compare(args) => cmd_compare(args),
        Commands::Inspect(args) => cmd_inspect(args),
    }
}

fn cmd_capture(args: cli::CaptureArgs) -> Result<()> {
    use capture::csv_log::CsvLogCapture;
    use profile::io;

    let capture = CsvLogCapture;
    let profile = capture.capture_from_file(&args.source_log, &args.source_host, &args.pg_version)?;

    println!(
        "Captured {} queries across {} sessions",
        profile.metadata.total_queries, profile.metadata.total_sessions
    );

    io::write_profile(&args.output, &profile)?;
    println!("Wrote workload profile to {}", args.output.display());
    Ok(())
}

fn cmd_replay(_args: cli::ReplayArgs) -> Result<()> {
    anyhow::bail!("Replay not yet implemented")
}

fn cmd_compare(_args: cli::CompareArgs) -> Result<()> {
    anyhow::bail!("Compare not yet implemented")
}

fn cmd_inspect(args: cli::InspectArgs) -> Result<()> {
    use profile::io;

    let profile = io::read_profile(&args.path)?;
    let json = serde_json::to_string_pretty(&profile)?;
    println!("{json}");
    Ok(())
}
```

**Step 3: Build and test CLI help**

Run: `cargo build && cargo run -- --help`
Expected: Shows help with capture, replay, compare, inspect subcommands

Run: `cargo run -- capture --help`
Expected: Shows capture-specific flags

**Step 4: Integration smoke test: capture + inspect**

Run:
```bash
cargo run -- capture --source-log tests/fixtures/sample_pg.csv --output /tmp/test.wkl --source-host localhost --pg-version 16.2
cargo run -- inspect /tmp/test.wkl
```
Expected: Capture prints summary, inspect prints JSON

**Step 5: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat: add CLI with capture and inspect subcommands"
```

---

### Task 5: Replay Engine

**Files:**
- Create: `src/replay/mod.rs`
- Create: `src/replay/session.rs`
- Modify: `src/main.rs` (add module + wire up cmd_replay)
- Create: `tests/replay_test.rs`

**Step 1: Write the failing test for replay result building**

Create `tests/replay_test.rs`:

```rust
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use pg_retest::replay::{QueryResult, ReplayResults, ReplayMode};
use chrono::Utc;

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
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test replay_test`
Expected: FAIL — module `replay` not found

**Step 3: Create replay types and mode logic**

Create `src/replay/mod.rs`:

```rust
pub mod session;

use serde::{Deserialize, Serialize};

use crate::profile::Query;

#[derive(Debug, Clone, Copy)]
pub enum ReplayMode {
    ReadWrite,
    ReadOnly,
}

impl ReplayMode {
    pub fn should_replay(&self, query: &Query) -> bool {
        match self {
            ReplayMode::ReadWrite => true,
            ReplayMode::ReadOnly => query.kind.is_read_only(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayResults {
    pub session_id: u64,
    pub query_results: Vec<QueryResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueryResult {
    pub sql: String,
    pub original_duration_us: u64,
    pub replay_duration_us: u64,
    pub success: bool,
    pub error: Option<String>,
}
```

**Step 4: Create session replay logic**

Create `src/replay/session.rs`:

```rust
use std::time::Instant;

use anyhow::Result;
use tokio::time::{sleep_until, Instant as TokioInstant};
use tokio_postgres::{Client, NoTls};
use tracing::{debug, warn};

use crate::profile::Session;
use crate::replay::{QueryResult, ReplayMode, ReplayResults};

pub async fn replay_session(
    session: &Session,
    connection_string: &str,
    mode: ReplayMode,
    speed: f64,
    replay_start: TokioInstant,
) -> Result<ReplayResults> {
    let (client, connection) = tokio_postgres::connect(connection_string, NoTls).await?;

    // Spawn the connection handler
    tokio::spawn(async move {
        if let Err(e) = connection.await {
            warn!("Connection error for session: {e}");
        }
    });

    let mut query_results = Vec::new();

    for query in &session.queries {
        if !mode.should_replay(query) {
            continue;
        }

        // Wait until the scaled target time
        let target_offset = std::time::Duration::from_micros(
            (query.start_offset_us as f64 / speed) as u64,
        );
        sleep_until(replay_start + target_offset).await;

        let start = Instant::now();
        let result = client.simple_query(&query.sql).await;
        let elapsed_us = start.elapsed().as_micros() as u64;

        let (success, error) = match result {
            Ok(_) => (true, None),
            Err(e) => {
                debug!("Query error in session {}: {e}", session.id);
                (false, Some(e.to_string()))
            }
        };

        query_results.push(QueryResult {
            sql: query.sql.clone(),
            original_duration_us: query.duration_us,
            replay_duration_us: elapsed_us,
            success,
            error,
        });
    }

    Ok(ReplayResults {
        session_id: session.id,
        query_results,
    })
}

pub async fn run_replay(
    profile: &crate::profile::WorkloadProfile,
    connection_string: &str,
    mode: ReplayMode,
    speed: f64,
) -> Result<Vec<ReplayResults>> {
    let replay_start = TokioInstant::now();
    let mut handles = Vec::new();

    for session in &profile.sessions {
        let session = session.clone();
        let conn_str = connection_string.to_string();

        let handle = tokio::spawn(async move {
            replay_session(&session, &conn_str, mode, speed, replay_start).await
        });

        handles.push(handle);
    }

    let mut all_results = Vec::new();
    for handle in handles {
        match handle.await? {
            Ok(results) => all_results.push(results),
            Err(e) => warn!("Session replay failed: {e}"),
        }
    }

    Ok(all_results)
}
```

**Step 5: Update main.rs — add replay module and wire up cmd_replay**

Add to `src/main.rs` (module declaration):

```rust
pub mod replay;
```

Replace `cmd_replay`:

```rust
fn cmd_replay(args: cli::ReplayArgs) -> Result<()> {
    use profile::io;
    use replay::{ReplayMode, session::run_replay};

    let profile = io::read_profile(&args.workload)?;
    let mode = if args.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    println!(
        "Replaying {} sessions ({} queries) against {}",
        profile.metadata.total_sessions,
        profile.metadata.total_queries,
        args.target
    );
    println!("Mode: {:?}, Speed: {}x", mode, args.speed);

    let rt = tokio::runtime::Runtime::new()?;
    let results = rt.block_on(run_replay(&profile, &args.target, mode, args.speed))?;

    // Build a results profile for comparison
    let total_replayed: usize = results.iter().map(|r| r.query_results.len()).sum();
    let total_errors: usize = results
        .iter()
        .flat_map(|r| &r.query_results)
        .filter(|q| !q.success)
        .count();

    println!("Replay complete: {total_replayed} queries replayed, {total_errors} errors");

    // Save results as MessagePack
    let bytes = rmp_serde::to_vec(&results)?;
    std::fs::write(&args.output, bytes)?;
    println!("Results written to {}", args.output.display());

    Ok(())
}
```

**Step 6: Run unit tests**

Run: `cargo test --test replay_test && cargo test --lib replay`
Expected: All PASS

**Step 7: Commit**

```bash
git add src/replay/ tests/replay_test.rs src/main.rs
git commit -m "feat: add async replay engine with read-only mode and speed control"
```

---

### Task 6: Compare / Reporter

**Files:**
- Create: `src/compare/mod.rs`
- Create: `src/compare/report.rs`
- Create: `tests/compare_test.rs`
- Modify: `src/main.rs` (add module + wire up cmd_compare)

**Step 1: Write the failing test**

Create `tests/compare_test.rs`:

```rust
use pg_retest::compare::{ComparisonReport, compute_comparison};
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use pg_retest::replay::{QueryResult, ReplayResults};
use chrono::Utc;

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
                Query { sql: "SELECT 1".into(), start_offset_us: 0, duration_us: 100, kind: QueryKind::Select },
                Query { sql: "SELECT 2".into(), start_offset_us: 500, duration_us: 200, kind: QueryKind::Select },
                Query { sql: "UPDATE t SET x=1".into(), start_offset_us: 1000, duration_us: 300, kind: QueryKind::Update },
                Query { sql: "SELECT 3".into(), start_offset_us: 1500, duration_us: 5000, kind: QueryKind::Select },
            ],
        }],
        metadata: Metadata { total_queries: 4, total_sessions: 1, capture_duration_us: 6500 },
    }
}

fn make_replay_results() -> Vec<ReplayResults> {
    vec![ReplayResults {
        session_id: 1,
        query_results: vec![
            QueryResult { sql: "SELECT 1".into(), original_duration_us: 100, replay_duration_us: 80, success: true, error: None },
            QueryResult { sql: "SELECT 2".into(), original_duration_us: 200, replay_duration_us: 250, success: true, error: None },
            QueryResult { sql: "UPDATE t SET x=1".into(), original_duration_us: 300, replay_duration_us: 280, success: true, error: None },
            QueryResult { sql: "SELECT 3".into(), original_duration_us: 5000, replay_duration_us: 4500, success: false, error: Some("timeout".into()) },
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
    // Replay avg: (80+250+280+4500)/4 = 1277 (rounding)
    assert_eq!(report.replay_avg_latency_us, 1277);
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
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test compare_test`
Expected: FAIL — module `compare` not found

**Step 3: Create comparison logic**

Create `src/compare/mod.rs`:

```rust
pub mod report;

use serde::{Deserialize, Serialize};

use crate::profile::WorkloadProfile;
use crate::replay::ReplayResults;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonReport {
    pub total_queries_source: u64,
    pub total_queries_replayed: u64,
    pub total_errors: u64,
    pub source_avg_latency_us: u64,
    pub replay_avg_latency_us: u64,
    pub source_p50_latency_us: u64,
    pub replay_p50_latency_us: u64,
    pub source_p95_latency_us: u64,
    pub replay_p95_latency_us: u64,
    pub source_p99_latency_us: u64,
    pub replay_p99_latency_us: u64,
    pub regressions: Vec<Regression>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Regression {
    pub sql: String,
    pub original_us: u64,
    pub replay_us: u64,
    pub change_pct: f64,
}

pub fn compute_comparison(
    source: &WorkloadProfile,
    results: &[ReplayResults],
    threshold_pct: f64,
) -> ComparisonReport {
    let mut source_durations: Vec<u64> = Vec::new();
    let mut replay_durations: Vec<u64> = Vec::new();
    let mut regressions = Vec::new();
    let mut total_errors: u64 = 0;

    // Collect all original durations from source
    for session in &source.sessions {
        for query in &session.queries {
            source_durations.push(query.duration_us);
        }
    }

    // Collect replay durations and detect regressions
    for result in results {
        for qr in &result.query_results {
            replay_durations.push(qr.replay_duration_us);

            if !qr.success {
                total_errors += 1;
            }

            if qr.original_duration_us > 0 {
                let change_pct = ((qr.replay_duration_us as f64 - qr.original_duration_us as f64)
                    / qr.original_duration_us as f64)
                    * 100.0;

                if change_pct > threshold_pct {
                    regressions.push(Regression {
                        sql: qr.sql.clone(),
                        original_us: qr.original_duration_us,
                        replay_us: qr.replay_duration_us,
                        change_pct,
                    });
                }
            }
        }
    }

    // Sort regressions by severity (worst first)
    regressions.sort_by(|a, b| b.change_pct.partial_cmp(&a.change_pct).unwrap());

    source_durations.sort();
    replay_durations.sort();

    ComparisonReport {
        total_queries_source: source_durations.len() as u64,
        total_queries_replayed: replay_durations.len() as u64,
        total_errors,
        source_avg_latency_us: avg(&source_durations),
        replay_avg_latency_us: avg(&replay_durations),
        source_p50_latency_us: percentile(&source_durations, 50),
        replay_p50_latency_us: percentile(&replay_durations, 50),
        source_p95_latency_us: percentile(&source_durations, 95),
        replay_p95_latency_us: percentile(&replay_durations, 95),
        source_p99_latency_us: percentile(&source_durations, 99),
        replay_p99_latency_us: percentile(&replay_durations, 99),
        regressions,
    }
}

fn avg(values: &[u64]) -> u64 {
    if values.is_empty() {
        return 0;
    }
    (values.iter().sum::<u64>() as f64 / values.len() as f64).round() as u64
}

fn percentile(sorted: &[u64], pct: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((pct as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
```

**Step 4: Create terminal table report**

Create `src/compare/report.rs`:

```rust
use std::path::Path;

use anyhow::Result;
use tabled::{Table, settings::Style};

use super::ComparisonReport;

pub fn print_terminal_report(report: &ComparisonReport) {
    println!();
    println!("  pg-retest Comparison Report");
    println!("  ===========================");
    println!();

    let rows = vec![
        make_row("Total queries", report.total_queries_source, report.total_queries_replayed),
        make_latency_row("Avg latency", report.source_avg_latency_us, report.replay_avg_latency_us),
        make_latency_row("P50 latency", report.source_p50_latency_us, report.replay_p50_latency_us),
        make_latency_row("P95 latency", report.source_p95_latency_us, report.replay_p95_latency_us),
        make_latency_row("P99 latency", report.source_p99_latency_us, report.replay_p99_latency_us),
        (
            "Errors".to_string(),
            "0".to_string(),
            report.total_errors.to_string(),
            if report.total_errors > 0 {
                format!("+{}", report.total_errors)
            } else {
                "0".to_string()
            },
            if report.total_errors > 0 { "WARN" } else { "OK" }.to_string(),
        ),
    ];

    let table = Table::new(rows.iter().map(|(metric, source, replay, delta, status)| {
        vec![
            metric.as_str(),
            source.as_str(),
            replay.as_str(),
            delta.as_str(),
            status.as_str(),
        ]
    }))
    .with(Style::rounded())
    .to_string();

    // Print header + table manually for better control
    println!("  {:<16} {:>10} {:>10} {:>10} {:>8}", "Metric", "Source", "Replay", "Delta", "Status");
    println!("  {}", "-".repeat(58));
    for (metric, source, replay, delta, status) in &rows {
        println!("  {:<16} {:>10} {:>10} {:>10} {:>8}", metric, source, replay, delta, status);
    }
    println!();

    if !report.regressions.is_empty() {
        let top_n = report.regressions.len().min(10);
        println!("  Top {} Regressions (>{:.0}% slower):", top_n, 0.0);
        println!("  {}", "-".repeat(58));
        for (i, reg) in report.regressions.iter().take(top_n).enumerate() {
            let sql_preview: String = reg.sql.chars().take(50).collect();
            println!(
                "  {}. {} +{:.1}% ({:.1}ms -> {:.1}ms)",
                i + 1,
                sql_preview,
                reg.change_pct,
                reg.original_us as f64 / 1000.0,
                reg.replay_us as f64 / 1000.0,
            );
        }
        println!();
    }
}

pub fn write_json_report(path: &Path, report: &ComparisonReport) -> Result<()> {
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn make_row(metric: &str, source: u64, replay: u64) -> (String, String, String, String, String) {
    let delta = replay as i64 - source as i64;
    let delta_str = if delta == 0 {
        "0".to_string()
    } else {
        format!("{:+}", delta)
    };
    let status = if delta == 0 { "OK" } else { "DIFF" };
    (
        metric.to_string(),
        source.to_string(),
        replay.to_string(),
        delta_str,
        status.to_string(),
    )
}

fn make_latency_row(metric: &str, source_us: u64, replay_us: u64) -> (String, String, String, String, String) {
    let source_ms = source_us as f64 / 1000.0;
    let replay_ms = replay_us as f64 / 1000.0;
    let delta_pct = if source_us > 0 {
        ((replay_us as f64 - source_us as f64) / source_us as f64) * 100.0
    } else {
        0.0
    };
    let status = if delta_pct < -5.0 {
        "FASTER"
    } else if delta_pct > 5.0 {
        "SLOWER"
    } else {
        "OK"
    };
    (
        metric.to_string(),
        format!("{:.1}ms", source_ms),
        format!("{:.1}ms", replay_ms),
        format!("{:+.1}%", delta_pct),
        status.to_string(),
    )
}
```

**Step 5: Update main.rs — add compare module and wire up cmd_compare**

Add module declaration: `pub mod compare;`

Replace `cmd_compare`:

```rust
fn cmd_compare(args: cli::CompareArgs) -> Result<()> {
    use compare::{compute_comparison, report};
    use profile::io;
    use replay::ReplayResults;

    let source = io::read_profile(&args.source)?;

    let replay_bytes = std::fs::read(&args.replay)?;
    let results: Vec<ReplayResults> = rmp_serde::from_slice(&replay_bytes)?;

    let report_data = compute_comparison(&source, &results, args.threshold);
    report::print_terminal_report(&report_data);

    if let Some(json_path) = &args.json {
        report::write_json_report(json_path, &report_data)?;
        println!("  JSON report written to {}", json_path.display());
    }

    Ok(())
}
```

**Step 6: Run tests**

Run: `cargo test --test compare_test`
Expected: All PASS

**Step 7: Commit**

```bash
git add src/compare/ tests/compare_test.rs src/main.rs
git commit -m "feat: add comparison report with terminal table and JSON output"
```

---

### Task 7: Documentation

**Files:**
- Create: `README.md`
- Modify: `CLAUDE.md` (update build section)

**Step 1: Create README.md**

```markdown
# pg-retest

Capture, replay, and compare PostgreSQL workloads.

pg-retest captures SQL workload from PostgreSQL server logs, replays it against a target database, and produces a side-by-side performance comparison report. Use it to validate configuration changes, server migrations, and capacity planning.

## Quick Start

```bash
# Build
cargo build --release

# 1. Capture workload from PG CSV logs
pg-retest capture --source-log /path/to/postgresql.csv --output workload.wkl

# 2. Replay against target database
pg-retest replay --workload workload.wkl --target "host=localhost dbname=mydb user=postgres"

# 3. Compare results
pg-retest compare --source workload.wkl --replay results.wkl --json report.json

# Inspect a workload profile
pg-retest inspect workload.wkl
```

## PostgreSQL Logging Setup

pg-retest captures workload by parsing PostgreSQL CSV logs. You need to configure your PostgreSQL server to produce these logs.

### Check Current Settings

Connect to your PostgreSQL server and check if logging is already configured:

```sql
SHOW logging_collector;   -- Must be 'on'
SHOW log_destination;      -- Must include 'csvlog'
SHOW log_statement;        -- Check current value
SHOW log_min_duration_statement;  -- Check current value
```

### Configure Logging

Add or modify these settings in `postgresql.conf`:

```ini
# Required: enables the log file collector process
# NOTE: changing this requires a PostgreSQL RESTART if not already 'on'
logging_collector = on

# Required: enable CSV log output
# Change takes effect after: pg_ctl reload (no restart needed)
log_destination = 'csvlog'

# Recommended: log all statements with their duration in one line
# Change takes effect after: pg_ctl reload (no restart needed)
log_min_duration_statement = 0    # logs every statement with duration

# Alternative: log statements and duration separately
# log_statement = 'all'           # logs all SQL statements
# log_duration = on               # logs duration of each statement

# Optional: useful log file naming
log_filename = 'postgresql-%Y-%m-%d.log'
log_rotation_age = 1d
```

### Apply Changes

If `logging_collector` was already `on`:

```bash
# No restart needed — just reload config
pg_ctl reload -D /path/to/data
# OR from SQL:
# SELECT pg_reload_conf();
```

If `logging_collector` was `off` (requires restart):

```bash
pg_ctl restart -D /path/to/data
```

### Verify Logging

After applying changes, run a few queries and check that CSV logs appear:

```bash
ls -la /path/to/data/log/*.csv
# You should see files like: postgresql-2024-03-08.csv
```

### Log File Location

- **Default:** `$PGDATA/log/` directory
- **Custom:** Set `log_directory` in postgresql.conf
- **RDS/Aurora:** Download logs via AWS Console, CLI, or RDS API
- **Cloud SQL:** Access via Google Cloud Console or gcloud CLI
- **Azure:** Access via Azure Portal or az CLI

### Performance Impact

Logging with `log_min_duration_statement = 0` has minimal overhead on most workloads (typically <2% throughput impact). For extremely high-throughput systems (>50k queries/sec), consider:

- Setting `log_min_duration_statement = 1` to skip sub-millisecond queries
- Capturing during off-peak windows
- Using `log_statement = 'none'` and relying on `auto_explain` instead

## Replay Modes

### Read-Write (default)

Replays all captured queries including INSERT, UPDATE, DELETE. **Important:** use a backup or snapshot of your database — DML will modify data.

```bash
pg-retest replay --workload workload.wkl --target "host=target dbname=mydb user=postgres"
```

### Read-Only

Strips all DML (INSERT, UPDATE, DELETE) and DDL (CREATE, ALTER, DROP), replaying only SELECT queries. Safe to run against production data.

```bash
pg-retest replay --workload workload.wkl --target "..." --read-only
```

### Speed Control

Compress or stretch timing gaps between queries:

```bash
# 2x faster (halves wait times between queries)
pg-retest replay --workload workload.wkl --target "..." --speed 2.0

# Half speed (doubles wait times)
pg-retest replay --workload workload.wkl --target "..." --speed 0.5
```

## Comparison Report

The compare command produces a terminal summary and optional JSON report:

```bash
pg-retest compare --source workload.wkl --replay results.wkl --json report.json --threshold 20
```

- `--threshold`: Flag queries that are slower by this percentage (default: 20%)

### Report Metrics

| Metric | Description |
|--------|-------------|
| Total queries | Count of queries in source vs. replay |
| Avg/P50/P95/P99 latency | Latency percentiles (microseconds) |
| Errors | Queries that failed during replay |
| Regressions | Individual queries exceeding the threshold |

## Workload Profile Format

Profiles are stored as MessagePack binary files (`.wkl`). Use `inspect` to view as JSON:

```bash
pg-retest inspect workload.wkl | jq .
```

## Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Run a single test
cargo test --test profile_io_test

# Run with verbose logging
RUST_LOG=debug pg-retest capture --source-log ...
```
```

**Step 2: Update CLAUDE.md build section**

Replace the Build & Development section in `CLAUDE.md`:

```markdown
## Build & Development

- **Language:** Rust (2021 edition)
- **Build:** `cargo build` (debug) / `cargo build --release`
- **Test all:** `cargo test`
- **Test single file:** `cargo test --test profile_io_test`
- **Test single function:** `cargo test --test profile_io_test test_profile_roundtrip_messagepack`
- **Test lib unit tests:** `cargo test --lib capture::csv_log`
- **Lint:** `cargo clippy`
- **Format:** `cargo fmt`
- **Run:** `cargo run -- <subcommand> [args]`
- **Verbose logging:** `RUST_LOG=debug cargo run -- -v <subcommand>`
```

**Step 3: Commit**

```bash
git add README.md CLAUDE.md
git commit -m "docs: add README with PG logging setup guide and usage docs"
```

---

### Task 8: Final Integration and Cleanup

**Step 1: Run full test suite**

Run: `cargo test`
Expected: All tests pass

**Step 2: Run clippy**

Run: `cargo clippy -- -W clippy::all`
Expected: No warnings (fix any that appear)

**Step 3: Run formatter**

Run: `cargo fmt`

**Step 4: Verify CLI end-to-end (capture + inspect)**

Run:
```bash
cargo run -- capture --source-log tests/fixtures/sample_pg.csv --output /tmp/e2e-test.wkl --source-host test --pg-version 16.2
cargo run -- inspect /tmp/e2e-test.wkl | head -20
```
Expected: capture succeeds, inspect shows JSON

**Step 5: Final commit**

```bash
git add -A
git commit -m "chore: clippy and fmt cleanup"
```
