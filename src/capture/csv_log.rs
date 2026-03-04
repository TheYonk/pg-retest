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

            entries.sort_by_key(|e| e.log_time);

            let first_time = entries[0].log_time;
            let user = entries[0].user_name.clone();
            let database = entries[0].database_name.clone();

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
                    let offset = (e.log_time - first_time).num_microseconds().unwrap_or(0) as u64;
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

    let stmt_marker = "statement: ";
    let stmt_pos = message.find(stmt_marker)?;
    let sql = message[stmt_pos + stmt_marker.len()..].to_string();

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
        let (dur, sql) =
            parse_duration_statement("duration: 1.234 ms  statement: SELECT * FROM users").unwrap();
        assert_eq!(dur, 1234);
        assert_eq!(sql, "SELECT * FROM users");
    }

    #[test]
    fn test_parse_duration_statement_sub_ms() {
        let (dur, sql) =
            parse_duration_statement("duration: 0.045 ms  statement: SELECT 1").unwrap();
        assert_eq!(dur, 45);
        assert_eq!(sql, "SELECT 1");
    }

    #[test]
    fn test_parse_duration_statement_rejects_non_duration() {
        assert!(parse_duration_statement("connection authorized: user=app").is_none());
        assert!(parse_duration_statement("").is_none());
    }
}
