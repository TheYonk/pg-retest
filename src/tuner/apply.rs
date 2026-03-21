use super::types::{AppliedChange, Recommendation};
use anyhow::Result;
use tokio_postgres::Client;

/// Apply a single recommendation to the target database.
///
/// - **ConfigChange**: `ALTER SYSTEM SET` + `pg_reload_conf()`, with rollback to previous value.
/// - **CreateIndex**: Execute the CREATE INDEX SQL, with rollback via `DROP INDEX IF EXISTS`.
/// - **QueryRewrite**: Informational only, always succeeds, no rollback.
/// - **SchemaChange**: Execute the SQL, no automatic rollback.
pub async fn apply_recommendation(client: &Client, rec: &Recommendation) -> AppliedChange {
    match rec {
        Recommendation::ConfigChange {
            parameter,
            current_value,
            recommended_value,
            ..
        } => {
            let set_sql = format!(
                "ALTER SYSTEM SET {} = '{}'",
                parameter,
                escape_pg_value(recommended_value)
            );
            let reload_sql = "SELECT pg_reload_conf()";
            let rollback_sql = format!(
                "ALTER SYSTEM SET {} = '{}'",
                parameter,
                escape_pg_value(current_value)
            );

            match execute_sql(client, &set_sql).await {
                Ok(()) => match execute_sql(client, reload_sql).await {
                    Ok(()) => AppliedChange {
                        recommendation: rec.clone(),
                        success: true,
                        error: None,
                        rollback_sql: Some(rollback_sql),
                    },
                    Err(e) => AppliedChange {
                        recommendation: rec.clone(),
                        success: false,
                        error: Some(format!("pg_reload_conf failed: {}", e)),
                        rollback_sql: Some(rollback_sql),
                    },
                },
                Err(e) => AppliedChange {
                    recommendation: rec.clone(),
                    success: false,
                    error: Some(format!("ALTER SYSTEM failed: {}", e)),
                    rollback_sql: None,
                },
            }
        }

        Recommendation::CreateIndex { sql, .. } => {
            let rollback =
                extract_index_name(sql).map(|name| format!("DROP INDEX IF EXISTS {}", name));

            match execute_sql(client, sql).await {
                Ok(()) => AppliedChange {
                    recommendation: rec.clone(),
                    success: true,
                    error: None,
                    rollback_sql: rollback,
                },
                Err(e) => AppliedChange {
                    recommendation: rec.clone(),
                    success: false,
                    error: Some(format!("CREATE INDEX failed: {}", e)),
                    rollback_sql: None,
                },
            }
        }

        Recommendation::QueryRewrite { .. } => {
            // Query rewrites are informational only — nothing to execute
            AppliedChange {
                recommendation: rec.clone(),
                success: true,
                error: None,
                rollback_sql: None,
            }
        }

        Recommendation::SchemaChange { sql, .. } => {
            // Split multi-statement SQL and execute each individually
            // so we get per-statement error reporting and partial success.
            let statements: Vec<&str> = sql
                .split(';')
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect();

            let mut errors = Vec::new();
            for stmt in &statements {
                if let Err(e) = execute_sql(client, stmt).await {
                    errors.push(format!("{}: {}", stmt, e));
                }
            }

            if errors.is_empty() {
                AppliedChange {
                    recommendation: rec.clone(),
                    success: true,
                    error: None,
                    rollback_sql: None,
                }
            } else {
                AppliedChange {
                    recommendation: rec.clone(),
                    success: false,
                    error: Some(errors.join("; ")),
                    rollback_sql: None,
                }
            }
        }
    }
}

/// Apply all recommendations sequentially and return the results.
pub async fn apply_all(client: &Client, recs: &[Recommendation]) -> Vec<AppliedChange> {
    let mut results = Vec::with_capacity(recs.len());
    for rec in recs {
        results.push(apply_recommendation(client, rec).await);
    }
    results
}

/// Result of rolling back a single applied change.
#[derive(Debug, Clone)]
pub struct RollbackResult {
    pub summary: String,
    pub success: bool,
    pub error: Option<String>,
}

/// Rollback all successfully applied changes in reverse order.
///
/// Only changes that have `success == true` and `rollback_sql.is_some()` are rolled back.
/// Config changes also trigger `pg_reload_conf()` after the ALTER SYSTEM RESET.
pub async fn rollback_all(client: &Client, changes: &[AppliedChange]) -> Vec<RollbackResult> {
    let mut results = Vec::new();
    let mut needs_reload = false;

    // Reverse order: undo last change first
    for change in changes.iter().rev() {
        if !change.success {
            continue;
        }
        let sql = match &change.rollback_sql {
            Some(s) => s,
            None => continue,
        };

        let summary = rec_summary(&change.recommendation);
        match execute_sql(client, sql).await {
            Ok(()) => {
                if matches!(change.recommendation, Recommendation::ConfigChange { .. }) {
                    needs_reload = true;
                }
                results.push(RollbackResult {
                    summary,
                    success: true,
                    error: None,
                });
            }
            Err(e) => {
                results.push(RollbackResult {
                    summary,
                    success: false,
                    error: Some(e.to_string()),
                });
            }
        }
    }

    // Reload config once if any config changes were rolled back
    if needs_reload {
        let _ = execute_sql(client, "SELECT pg_reload_conf()").await;
    }

    results
}

fn rec_summary(rec: &Recommendation) -> String {
    match rec {
        Recommendation::ConfigChange {
            parameter,
            current_value,
            ..
        } => format!("{} -> {}", parameter, current_value),
        Recommendation::CreateIndex { sql, .. } => {
            let preview: String = sql.chars().take(50).collect();
            preview
        }
        Recommendation::QueryRewrite { .. } => "query rewrite".into(),
        Recommendation::SchemaChange { description, .. } => description.clone(),
    }
}

/// Escape a value for use in a single-quoted SQL string.
/// Doubles any single quotes to prevent SQL injection.
fn escape_pg_value(value: &str) -> String {
    value.replace('\'', "''")
}

/// Execute a SQL statement via batch_execute (no result rows expected).
async fn execute_sql(client: &Client, sql: &str) -> Result<()> {
    client.batch_execute(sql).await?;
    Ok(())
}

/// Extract the index name from a CREATE INDEX statement.
///
/// Handles variations:
/// - `CREATE INDEX idx ON ...`
/// - `CREATE INDEX IF NOT EXISTS idx ON ...`
/// - `CREATE UNIQUE INDEX idx ON ...`
/// - `CREATE INDEX CONCURRENTLY idx ON ...`
/// - `CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx ON ...`
fn extract_index_name(sql: &str) -> Option<String> {
    let upper = sql.to_uppercase();
    let tokens: Vec<&str> = upper.split_whitespace().collect();
    let orig_tokens: Vec<&str> = sql.split_whitespace().collect();

    // Find the position of "INDEX" keyword
    let idx_pos = tokens.iter().position(|&t| t == "INDEX")?;

    // Walk forward, skipping known modifiers until we find the index name
    let mut pos = idx_pos + 1;
    while pos < tokens.len() {
        match tokens[pos] {
            "IF" | "NOT" | "EXISTS" | "CONCURRENTLY" => {
                pos += 1;
            }
            "ON" => {
                // We passed the name — shouldn't happen, but bail
                return None;
            }
            _ => {
                // This is the index name
                return Some(orig_tokens[pos].to_string());
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_index_name() {
        // Basic CREATE INDEX
        assert_eq!(
            extract_index_name("CREATE INDEX idx_foo ON bar (baz)"),
            Some("idx_foo".into())
        );

        // With IF NOT EXISTS
        assert_eq!(
            extract_index_name("CREATE INDEX IF NOT EXISTS idx_bar ON t (c)"),
            Some("idx_bar".into())
        );

        // UNIQUE INDEX
        assert_eq!(
            extract_index_name("CREATE UNIQUE INDEX idx_u ON t (c)"),
            Some("idx_u".into())
        );

        // CONCURRENTLY
        assert_eq!(
            extract_index_name("CREATE INDEX CONCURRENTLY idx_conc ON t (c)"),
            Some("idx_conc".into())
        );

        // UNIQUE + CONCURRENTLY + IF NOT EXISTS
        assert_eq!(
            extract_index_name("CREATE UNIQUE INDEX CONCURRENTLY IF NOT EXISTS idx_all ON t (c)"),
            Some("idx_all".into())
        );

        // Not a CREATE INDEX statement
        assert_eq!(extract_index_name("SELECT 1"), None);
    }

    #[test]
    fn test_escape_pg_value() {
        assert_eq!(escape_pg_value("128MB"), "128MB");
        assert_eq!(escape_pg_value("it's"), "it''s");
        assert_eq!(escape_pg_value("val'ue"), "val''ue");
        assert_eq!(escape_pg_value("normal"), "normal");
        assert_eq!(escape_pg_value(""), "");
    }
}
