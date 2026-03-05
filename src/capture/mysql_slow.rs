use std::collections::HashMap;
use std::io::BufRead;
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use tracing::debug;

use crate::profile::{
    assign_transaction_ids, Metadata, Query, QueryKind, Session, WorkloadProfile,
};
use crate::transform::mysql_to_pg::mysql_to_pg_pipeline;
use crate::transform::{TransformPipeline, TransformReport, TransformResult};

pub struct MysqlSlowLogCapture;

/// A raw parsed slow log entry.
struct SlowLogEntry {
    timestamp: DateTime<Utc>,
    user: String,
    thread_id: u64,
    query_time_us: u64,
    sql: String,
}

impl MysqlSlowLogCapture {
    pub fn capture_from_file(
        &self,
        path: &Path,
        source_host: &str,
        transform: bool,
    ) -> Result<WorkloadProfile> {
        let entries = self.parse_slow_log(path)?;
        let pipeline = if transform {
            Some(mysql_to_pg_pipeline())
        } else {
            None
        };
        self.build_profile(entries, source_host, pipeline.as_ref())
    }

    fn parse_slow_log(&self, path: &Path) -> Result<Vec<SlowLogEntry>> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open MySQL slow log: {}", path.display()))?;
        let reader = std::io::BufReader::new(file);

        let mut entries = Vec::new();
        let mut current_time: Option<DateTime<Utc>> = None;
        let mut current_user = String::new();
        let mut current_thread_id: u64 = 0;
        let mut current_query_time_us: u64 = 0;
        let mut current_sql_lines: Vec<String> = Vec::new();
        let mut in_query = false;

        for line in reader.lines() {
            let line = line.context("Failed to read line")?;
            let trimmed = line.trim();

            // Skip header lines
            if trimmed.is_empty()
                || trimmed.starts_with("/usr/")
                || trimmed.starts_with("Tcp port:")
                || trimmed.starts_with("Time ")
            {
                // If we were accumulating a query, flush it
                if in_query && !current_sql_lines.is_empty() {
                    let sql = current_sql_lines.join("\n").trim().to_string();
                    if !sql.is_empty() {
                        entries.push(SlowLogEntry {
                            timestamp: current_time.unwrap_or_else(Utc::now),
                            user: current_user.clone(),
                            thread_id: current_thread_id,
                            query_time_us: current_query_time_us,
                            sql,
                        });
                    }
                    current_sql_lines.clear();
                    in_query = false;
                }
                continue;
            }

            // # Time: 2024-03-08T10:00:00.100000Z
            if let Some(time_str) = trimmed.strip_prefix("# Time: ") {
                // Flush previous query if any
                if in_query && !current_sql_lines.is_empty() {
                    let sql = current_sql_lines.join("\n").trim().to_string();
                    if !sql.is_empty() {
                        entries.push(SlowLogEntry {
                            timestamp: current_time.unwrap_or_else(Utc::now),
                            user: current_user.clone(),
                            thread_id: current_thread_id,
                            query_time_us: current_query_time_us,
                            sql,
                        });
                    }
                    current_sql_lines.clear();
                    in_query = false;
                }

                current_time = parse_mysql_timestamp(time_str);
                if current_time.is_none() {
                    debug!("Failed to parse timestamp: {time_str}");
                }
                continue;
            }

            // # User@Host: app_user[app_user] @ localhost []  Id:    42
            if trimmed.starts_with("# User@Host:") {
                if let Some((user, thread_id)) = parse_user_host(trimmed) {
                    current_user = user;
                    current_thread_id = thread_id;
                }
                continue;
            }

            // # Query_time: 0.001234  Lock_time: 0.000100 ...
            if trimmed.starts_with("# Query_time:") {
                if let Some(qt_us) = parse_query_time(trimmed) {
                    current_query_time_us = qt_us;
                }
                continue;
            }

            // SET timestamp=...; — extract epoch for time reference, don't include as query
            if trimmed.starts_with("SET timestamp=") {
                if let Some(ts) = parse_set_timestamp(trimmed) {
                    // Only use this if we don't have a # Time: line
                    if current_time.is_none() {
                        current_time = Some(ts);
                    }
                }
                in_query = true;
                continue;
            }

            // Any other line is part of the SQL query
            if in_query || !trimmed.starts_with('#') {
                in_query = true;
                current_sql_lines.push(trimmed.to_string());
            }
        }

        // Flush final query
        if in_query && !current_sql_lines.is_empty() {
            let sql = current_sql_lines.join("\n").trim().to_string();
            if !sql.is_empty() {
                entries.push(SlowLogEntry {
                    timestamp: current_time.unwrap_or_else(Utc::now),
                    user: current_user.clone(),
                    thread_id: current_thread_id,
                    query_time_us: current_query_time_us,
                    sql,
                });
            }
        }

        Ok(entries)
    }

    fn build_profile(
        &self,
        entries: Vec<SlowLogEntry>,
        source_host: &str,
        pipeline: Option<&TransformPipeline>,
    ) -> Result<WorkloadProfile> {
        // Group by thread_id (MySQL's equivalent of session)
        let mut session_map: HashMap<u64, Vec<SlowLogEntry>> = HashMap::new();
        for entry in entries {
            session_map.entry(entry.thread_id).or_default().push(entry);
        }

        let mut sessions = Vec::new();
        let mut total_queries: u64 = 0;
        let mut next_txn_id: u64 = 1;
        let mut transform_report = TransformReport::default();

        let mut global_min_time: Option<DateTime<Utc>> = None;
        let mut global_max_time: Option<DateTime<Utc>> = None;

        for (thread_id, mut entries) in session_map {
            if entries.is_empty() {
                continue;
            }

            entries.sort_by_key(|e| e.timestamp);

            let first_time = entries[0].timestamp;
            let user = entries[0].user.clone();

            // Track global time range
            for e in &entries {
                match global_min_time {
                    None => global_min_time = Some(e.timestamp),
                    Some(t) if e.timestamp < t => global_min_time = Some(e.timestamp),
                    _ => {}
                }
                match global_max_time {
                    None => global_max_time = Some(e.timestamp),
                    Some(t) if e.timestamp > t => global_max_time = Some(e.timestamp),
                    _ => {}
                }
            }

            let mut queries: Vec<Query> = Vec::new();

            for entry in &entries {
                let sql = if let Some(pipe) = pipeline {
                    let result = pipe.apply(&entry.sql);
                    transform_report.record(&entry.sql, &result);
                    match result {
                        TransformResult::Transformed(sql) => sql,
                        TransformResult::Unchanged => entry.sql.clone(),
                        TransformResult::Skipped { reason } => {
                            debug!("Skipped query: {reason}");
                            continue; // Don't include skipped queries
                        }
                    }
                } else {
                    entry.sql.clone()
                };

                let offset = (entry.timestamp - first_time)
                    .num_microseconds()
                    .unwrap_or(0) as u64;
                queries.push(Query {
                    sql: sql.clone(),
                    start_offset_us: offset,
                    duration_us: entry.query_time_us,
                    kind: QueryKind::from_sql(&sql),
                    transaction_id: None,
                });
            }

            // Assign transaction IDs
            assign_transaction_ids(&mut queries, &mut next_txn_id);

            total_queries += queries.len() as u64;

            if !queries.is_empty() {
                sessions.push(Session {
                    id: thread_id,
                    user,
                    database: String::new(), // MySQL slow log doesn't include DB per query
                    queries,
                });
            }
        }

        sessions.sort_by_key(|s| s.queries.first().map(|q| q.start_offset_us).unwrap_or(0));

        let capture_duration_us = match (global_min_time, global_max_time) {
            (Some(min), Some(max)) => (max - min).num_microseconds().unwrap_or(0) as u64,
            _ => 0,
        };

        // Print transform report if we ran transforms
        if pipeline.is_some() {
            transform_report.print_summary();
        }

        let total_sessions = sessions.len() as u64;
        Ok(WorkloadProfile {
            version: 2,
            captured_at: Utc::now(),
            source_host: source_host.to_string(),
            pg_version: "unknown".to_string(),
            capture_method: "mysql_slow_log".to_string(),
            sessions,
            metadata: Metadata {
                total_queries,
                total_sessions,
                capture_duration_us,
            },
        })
    }
}

/// Parse MySQL slow log timestamp: "2024-03-08T10:00:00.100000Z"
fn parse_mysql_timestamp(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    // Try ISO 8601 first
    s.parse::<DateTime<Utc>>().ok().or_else(|| {
        // Try without timezone suffix
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
            .ok()
            .map(|ndt| ndt.and_utc())
    })
}

/// Parse "# User@Host: app_user[app_user] @ localhost []  Id:    42"
/// Returns (username, thread_id)
fn parse_user_host(line: &str) -> Option<(String, u64)> {
    let rest = line.strip_prefix("# User@Host:")?.trim();

    // Username is before the first '['
    let bracket_pos = rest.find('[')?;
    let user = rest[..bracket_pos].trim().to_string();

    // Thread ID is after "Id:" at the end
    let id_pos = rest.rfind("Id:")?;
    let id_str = rest[id_pos + 3..].trim();
    let thread_id: u64 = id_str.parse().ok()?;

    Some((user, thread_id))
}

/// Parse "# Query_time: 0.001234  Lock_time: 0.000100 ..."
/// Returns query time in microseconds
fn parse_query_time(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("# Query_time:")?.trim();
    let end = rest.find(|c: char| c.is_whitespace())?;
    let qt_str = &rest[..end];
    let qt_secs: f64 = qt_str.parse().ok()?;
    Some((qt_secs * 1_000_000.0).round() as u64)
}

/// Parse "SET timestamp=1709892000;" and return as DateTime<Utc>
fn parse_set_timestamp(line: &str) -> Option<DateTime<Utc>> {
    let rest = line.strip_prefix("SET timestamp=")?;
    let rest = rest.trim_end_matches(';').trim();
    let epoch: i64 = rest.parse().ok()?;
    DateTime::from_timestamp(epoch, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mysql_timestamp() {
        let ts = parse_mysql_timestamp("2024-03-08T10:00:00.100000Z").unwrap();
        assert_eq!(
            ts.format("%Y-%m-%d %H:%M:%S").to_string(),
            "2024-03-08 10:00:00"
        );
    }

    #[test]
    fn test_parse_user_host() {
        let (user, id) =
            parse_user_host("# User@Host: app_user[app_user] @ localhost []  Id:    42").unwrap();
        assert_eq!(user, "app_user");
        assert_eq!(id, 42);
    }

    #[test]
    fn test_parse_user_host_with_ip() {
        let (user, id) =
            parse_user_host("# User@Host: admin[admin] @ 192.168.1.10 []  Id:    99").unwrap();
        assert_eq!(user, "admin");
        assert_eq!(id, 99);
    }

    #[test]
    fn test_parse_query_time() {
        assert_eq!(
            parse_query_time(
                "# Query_time: 0.001234  Lock_time: 0.000100 Rows_sent: 1  Rows_examined: 100"
            ),
            Some(1234)
        );
        assert_eq!(
            parse_query_time(
                "# Query_time: 0.050000  Lock_time: 0.001000 Rows_sent: 5000  Rows_examined: 100000"
            ),
            Some(50000)
        );
    }

    #[test]
    fn test_parse_set_timestamp() {
        let ts = parse_set_timestamp("SET timestamp=1709892000;").unwrap();
        assert_eq!(ts.format("%Y-%m-%d").to_string(), "2024-03-08");
    }
}
