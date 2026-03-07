use crate::profile::WorkloadProfile;
use anyhow::Result;
use serde::Serialize;
use std::collections::HashMap;
use tokio_postgres::{Client, NoTls};

/// A PostgreSQL configuration setting with its current value and source.
#[derive(Debug, Clone, Serialize)]
pub struct PgSetting {
    pub name: String,
    pub setting: String,
    pub unit: Option<String>,
    pub source: String,
}

/// A slow query extracted from the captured workload profile.
#[derive(Debug, Clone, Serialize)]
pub struct SlowQuery {
    pub sql: String,
    pub duration_us: u64,
    pub occurrences: usize,
}

/// A row from pg_stat_statements.
#[derive(Debug, Clone, Serialize)]
pub struct StatStatement {
    pub query: String,
    pub calls: i64,
    pub mean_exec_time_ms: f64,
    pub total_exec_time_ms: f64,
}

/// Schema information for a single table.
#[derive(Debug, Clone, Serialize)]
pub struct TableSchema {
    pub table_name: String,
    pub columns: Vec<ColumnInfo>,
    pub indexes: Vec<IndexInfo>,
}

/// Column metadata.
#[derive(Debug, Clone, Serialize)]
pub struct ColumnInfo {
    pub name: String,
    pub data_type: String,
    pub is_nullable: bool,
}

/// Index metadata.
#[derive(Debug, Clone, Serialize)]
pub struct IndexInfo {
    pub index_name: String,
    pub columns: String,
    pub is_unique: bool,
}

/// Index usage statistics from pg_stat_user_indexes.
#[derive(Debug, Clone, Serialize)]
pub struct IndexUsage {
    pub index_name: String,
    pub table_name: String,
    pub idx_scan: i64,
    pub idx_tup_read: i64,
}

/// Table-level statistics from pg_stat_user_tables.
#[derive(Debug, Clone, Serialize)]
pub struct TableStats {
    pub table_name: String,
    pub seq_scan: i64,
    pub idx_scan: i64,
    pub n_live_tup: i64,
    pub n_dead_tup: i64,
}

/// An EXPLAIN plan for a query.
#[derive(Debug, Clone, Serialize)]
pub struct ExplainPlan {
    pub sql: String,
    pub plan_json: serde_json::Value,
}

/// Complete PostgreSQL context collected for the tuning advisor.
#[derive(Debug, Clone, Serialize)]
pub struct PgContext {
    pub pg_version: String,
    pub non_default_settings: Vec<PgSetting>,
    pub top_slow_queries: Vec<SlowQuery>,
    pub stat_statements: Option<Vec<StatStatement>>,
    pub schema: Vec<TableSchema>,
    pub index_usage: Vec<IndexUsage>,
    pub table_stats: Vec<TableStats>,
    pub explain_plans: Vec<ExplainPlan>,
}

/// Connect to a PostgreSQL instance and return a client.
pub async fn connect(connection_string: &str) -> Result<Client> {
    let (client, connection) = tokio_postgres::connect(connection_string, NoTls).await?;

    tokio::spawn(async move {
        if let Err(e) = connection.await {
            tracing::error!("PG connection error: {}", e);
        }
    });

    Ok(client)
}

/// Collect all PG context needed for the tuning advisor.
pub async fn collect_context(
    client: &Client,
    profile: &WorkloadProfile,
    max_slow_queries: usize,
) -> Result<PgContext> {
    let pg_version = collect_version(client).await?;
    let non_default_settings = collect_settings(client).await?;
    let top_slow_queries = extract_slow_queries(profile, max_slow_queries);
    let stat_statements = collect_stat_statements(client).await;
    let schema = collect_schema(client).await?;
    let index_usage = collect_index_usage(client).await?;
    let table_stats = collect_table_stats(client).await?;
    let explain_plans = collect_explain_plans(client, &top_slow_queries).await;

    Ok(PgContext {
        pg_version,
        non_default_settings,
        top_slow_queries,
        stat_statements,
        schema,
        index_usage,
        table_stats,
        explain_plans,
    })
}

async fn collect_version(client: &Client) -> Result<String> {
    let row = client.query_one("SELECT version()", &[]).await?;
    let version: String = row.get(0);
    Ok(version)
}

async fn collect_settings(client: &Client) -> Result<Vec<PgSetting>> {
    let rows = client
        .query(
            "SELECT name, setting, unit, source FROM pg_settings WHERE source != 'default' AND source != 'override' ORDER BY name",
            &[],
        )
        .await?;

    let settings = rows
        .iter()
        .map(|row| PgSetting {
            name: row.get(0),
            setting: row.get(1),
            unit: row.get(2),
            source: row.get(3),
        })
        .collect();

    Ok(settings)
}

async fn collect_stat_statements(client: &Client) -> Option<Vec<StatStatement>> {
    let rows = client
        .query(
            "SELECT query, calls, mean_exec_time, total_exec_time \
             FROM pg_stat_statements \
             ORDER BY total_exec_time DESC \
             LIMIT 50",
            &[],
        )
        .await
        .ok()?;

    let stmts = rows
        .iter()
        .map(|row| StatStatement {
            query: row.get(0),
            calls: row.get(1),
            mean_exec_time_ms: row.get(2),
            total_exec_time_ms: row.get(3),
        })
        .collect();

    Some(stmts)
}

async fn collect_schema(client: &Client) -> Result<Vec<TableSchema>> {
    let tables = client
        .query(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'public' AND table_type = 'BASE TABLE' \
             ORDER BY table_name",
            &[],
        )
        .await?;

    let mut schemas = Vec::new();
    for table_row in &tables {
        let table_name: String = table_row.get(0);

        let col_rows = client
            .query(
                "SELECT column_name, data_type, is_nullable \
                 FROM information_schema.columns \
                 WHERE table_schema = 'public' AND table_name = $1 \
                 ORDER BY ordinal_position",
                &[&table_name],
            )
            .await?;

        let columns: Vec<ColumnInfo> = col_rows
            .iter()
            .map(|r| {
                let nullable: String = r.get(2);
                ColumnInfo {
                    name: r.get(0),
                    data_type: r.get(1),
                    is_nullable: nullable == "YES",
                }
            })
            .collect();

        let idx_rows = client
            .query(
                "SELECT i.relname, pg_get_indexdef(i.oid), ix.indisunique \
                 FROM pg_index ix \
                 JOIN pg_class t ON t.oid = ix.indrelid \
                 JOIN pg_class i ON i.oid = ix.indexrelid \
                 JOIN pg_namespace n ON n.oid = t.relnamespace \
                 WHERE n.nspname = 'public' AND t.relname = $1 \
                 ORDER BY i.relname",
                &[&table_name],
            )
            .await?;

        let indexes: Vec<IndexInfo> = idx_rows
            .iter()
            .map(|r| IndexInfo {
                index_name: r.get(0),
                columns: r.get(1),
                is_unique: r.get(2),
            })
            .collect();

        schemas.push(TableSchema {
            table_name,
            columns,
            indexes,
        });
    }

    Ok(schemas)
}

async fn collect_index_usage(client: &Client) -> Result<Vec<IndexUsage>> {
    let rows = client
        .query(
            "SELECT indexrelname, relname, idx_scan, idx_tup_read \
             FROM pg_stat_user_indexes \
             ORDER BY idx_scan DESC \
             LIMIT 100",
            &[],
        )
        .await?;

    let usage = rows
        .iter()
        .map(|row| IndexUsage {
            index_name: row.get(0),
            table_name: row.get(1),
            idx_scan: row.get(2),
            idx_tup_read: row.get(3),
        })
        .collect();

    Ok(usage)
}

async fn collect_table_stats(client: &Client) -> Result<Vec<TableStats>> {
    let rows = client
        .query(
            "SELECT relname, seq_scan, idx_scan, n_live_tup, n_dead_tup \
             FROM pg_stat_user_tables \
             ORDER BY seq_scan DESC \
             LIMIT 100",
            &[],
        )
        .await?;

    let stats = rows
        .iter()
        .map(|row| TableStats {
            table_name: row.get(0),
            seq_scan: row.get(1),
            idx_scan: row.get(2),
            n_live_tup: row.get(3),
            n_dead_tup: row.get(4),
        })
        .collect();

    Ok(stats)
}

/// Extract the top-N slowest distinct queries from a workload profile.
/// Groups by SQL text, tracks max duration and occurrence count.
fn extract_slow_queries(profile: &WorkloadProfile, max: usize) -> Vec<SlowQuery> {
    let mut query_map: HashMap<String, (u64, usize)> = HashMap::new();

    for session in &profile.sessions {
        for query in &session.queries {
            let entry = query_map.entry(query.sql.clone()).or_insert((0, 0));
            if query.duration_us > entry.0 {
                entry.0 = query.duration_us;
            }
            entry.1 += 1;
        }
    }

    let mut slow_queries: Vec<SlowQuery> = query_map
        .into_iter()
        .map(|(sql, (duration_us, occurrences))| SlowQuery {
            sql,
            duration_us,
            occurrences,
        })
        .collect();

    slow_queries.sort_by(|a, b| b.duration_us.cmp(&a.duration_us));
    slow_queries.truncate(max);
    slow_queries
}

/// Collect EXPLAIN plans for SELECT queries without bind parameters.
async fn collect_explain_plans(client: &Client, queries: &[SlowQuery]) -> Vec<ExplainPlan> {
    let mut plans = Vec::new();

    for query in queries {
        let upper = query.sql.trim().to_uppercase();
        // Only EXPLAIN SELECT queries
        if !upper.starts_with("SELECT") && !upper.starts_with("WITH") {
            continue;
        }
        // Skip queries with bind parameters ($1, $2, etc.)
        if query.sql.contains('$') {
            continue;
        }

        let explain_sql = format!("EXPLAIN (FORMAT JSON, COSTS) {}", query.sql);
        match client.query_one(&explain_sql, &[]).await {
            Ok(row) => {
                // EXPLAIN FORMAT JSON returns a text column containing JSON
                let plan_text: String = row.get(0);
                let plan_json: serde_json::Value =
                    serde_json::from_str(&plan_text).unwrap_or(serde_json::Value::Null);
                plans.push(ExplainPlan {
                    sql: query.sql.clone(),
                    plan_json,
                });
            }
            Err(_) => {
                // Silently skip queries that fail to EXPLAIN
                // (e.g., referencing tables that don't exist on the target)
            }
        }
    }

    plans
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
    use chrono::Utc;

    #[test]
    fn test_extract_slow_queries() {
        let profile = WorkloadProfile {
            version: 2,
            captured_at: Utc::now(),
            source_host: "localhost".into(),
            pg_version: "16.0".into(),
            capture_method: "csv_log".into(),
            sessions: vec![Session {
                id: 1,
                user: "test".into(),
                database: "testdb".into(),
                queries: vec![
                    Query {
                        sql: "SELECT * FROM orders".into(),
                        start_offset_us: 0,
                        duration_us: 5000,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT * FROM users".into(),
                        start_offset_us: 1000,
                        duration_us: 10000,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT * FROM orders".into(),
                        start_offset_us: 2000,
                        duration_us: 8000,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT * FROM users".into(),
                        start_offset_us: 3000,
                        duration_us: 3000,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                ],
            }],
            metadata: Metadata {
                total_queries: 4,
                total_sessions: 1,
                capture_duration_us: 13000,
            },
        };

        let slow = extract_slow_queries(&profile, 10);

        // Should return 2 distinct queries
        assert_eq!(slow.len(), 2);

        // Sorted by max duration_us descending
        assert_eq!(slow[0].sql, "SELECT * FROM users");
        assert_eq!(slow[0].duration_us, 10000); // max of 10000 and 3000
        assert_eq!(slow[0].occurrences, 2);

        assert_eq!(slow[1].sql, "SELECT * FROM orders");
        assert_eq!(slow[1].duration_us, 8000); // max of 5000 and 8000
        assert_eq!(slow[1].occurrences, 2);
    }
}
