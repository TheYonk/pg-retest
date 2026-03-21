use super::types::Recommendation;
use anyhow::{bail, Result};

/// PostgreSQL configuration parameters that are safe to change via ALTER SYSTEM.
/// These are performance-tuning knobs that do not affect data integrity,
/// security, or connectivity.
pub const SAFE_CONFIG_PARAMS: &[&str] = &[
    "shared_buffers",
    "work_mem",
    "maintenance_work_mem",
    "effective_cache_size",
    "temp_buffers",
    "huge_pages",
    "random_page_cost",
    "seq_page_cost",
    "cpu_tuple_cost",
    "cpu_index_tuple_cost",
    "cpu_operator_cost",
    "default_statistics_target",
    "enable_seqscan",
    "enable_indexscan",
    "enable_bitmapscan",
    "enable_hashjoin",
    "enable_mergejoin",
    "enable_nestloop",
    "enable_hashagg",
    "enable_material",
    "enable_sort",
    "max_parallel_workers_per_gather",
    "max_parallel_workers",
    "max_parallel_maintenance_workers",
    "parallel_tuple_cost",
    "parallel_setup_cost",
    "min_parallel_table_scan_size",
    "min_parallel_index_scan_size",
    "checkpoint_completion_target",
    "wal_buffers",
    "commit_delay",
    "commit_siblings",
    "jit",
    "jit_above_cost",
    "jit_inline_above_cost",
    "jit_optimize_above_cost",
    "autovacuum_vacuum_scale_factor",
    "autovacuum_analyze_scale_factor",
    "autovacuum_vacuum_cost_delay",
    "autovacuum_vacuum_cost_limit",
    "log_min_duration_statement",
];

/// Configuration parameters that must never be changed by the tuner.
/// These affect connectivity, security, and data directory layout.
pub const BLOCKED_CONFIG_PARAMS: &[&str] = &[
    "data_directory",
    "listen_addresses",
    "port",
    "hba_file",
    "pg_hba_file",
    "ident_file",
    "ssl_cert_file",
    "ssl_key_file",
    "ssl_ca_file",
    "password_encryption",
    "log_directory",
];

/// Hostname patterns that suggest a production environment.
const PRODUCTION_PATTERNS: &[&str] = &["prod", "production", "primary", "master", "main"];

/// Check whether the target connection string contains a production hostname pattern.
/// If it does and `force` is false, return an error to prevent accidental tuning
/// of production databases.
pub fn check_production_hostname(target: &str, force: bool) -> Result<()> {
    let lower = target.to_lowercase();
    for pattern in PRODUCTION_PATTERNS {
        if lower.contains(pattern) {
            if force {
                return Ok(());
            }
            bail!(
                "Target '{}' looks like a production server (matched pattern '{}').\n\
                 Use --force to override this safety check.",
                target,
                pattern
            );
        }
    }
    Ok(())
}

/// Check whether a PG parameter name is on the safe allowlist.
/// Comparison is case-insensitive.
pub fn is_safe_param(param: &str) -> bool {
    let lower = param.to_lowercase();
    SAFE_CONFIG_PARAMS.iter().any(|&safe| safe == lower)
}

/// Check whether a SQL statement is on the allowlist for SchemaChange execution.
/// Only these operations are safe to auto-apply from LLM recommendations:
/// - CREATE INDEX (including CONCURRENTLY, UNIQUE, IF NOT EXISTS)
/// - ANALYZE
/// - REINDEX
///
/// Everything else is rejected and presented as a suggestion for human review.
pub fn is_allowed_schema_sql(sql: &str) -> bool {
    let upper = sql.trim().to_uppercase();

    if upper.starts_with("CREATE INDEX") || upper.starts_with("CREATE UNIQUE INDEX") {
        return true;
    }

    if upper.starts_with("ANALYZE") {
        return true;
    }

    if upper.starts_with("REINDEX") {
        return true;
    }

    false
}

/// Validate a list of recommendations, splitting them into safe and rejected.
/// Returns `(safe, rejected_with_reasons)`.
pub fn validate_recommendations(
    recs: &[Recommendation],
) -> (Vec<Recommendation>, Vec<(Recommendation, String)>) {
    let mut safe = Vec::new();
    let mut rejected: Vec<(Recommendation, String)> = Vec::new();

    for rec in recs {
        match rec {
            Recommendation::ConfigChange { parameter, .. } => {
                if is_safe_param(parameter) {
                    safe.push(rec.clone());
                } else {
                    rejected.push((
                        rec.clone(),
                        format!("Parameter '{}' is not on the safe allowlist", parameter),
                    ));
                }
            }
            Recommendation::CreateIndex { sql, .. } => {
                if is_allowed_schema_sql(sql) {
                    safe.push(rec.clone());
                } else {
                    rejected.push((
                        rec.clone(),
                        format!(
                            "CreateIndex SQL is not on the allowed list: {}",
                            sql.chars().take(60).collect::<String>()
                        ),
                    ));
                }
            }
            Recommendation::QueryRewrite { .. } => {
                // Query rewrites are always safe (informational only)
                safe.push(rec.clone());
            }
            Recommendation::SchemaChange { sql, .. } => {
                if is_allowed_schema_sql(sql) {
                    safe.push(rec.clone());
                } else {
                    rejected.push((
                        rec.clone(),
                        format!(
                            "SchemaChange SQL is not on the allowed list (only CREATE INDEX, ANALYZE, REINDEX are auto-applied): {}",
                            sql.chars().take(60).collect::<String>()
                        ),
                    ));
                }
            }
        }
    }

    (safe, rejected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_safe_params() {
        // Known safe param
        assert!(is_safe_param("shared_buffers"));
        // Blocked param should not be safe
        assert!(!is_safe_param("data_directory"));
        // Unknown param should not be safe
        assert!(!is_safe_param("some_unknown_param"));
        // Case insensitive
        assert!(is_safe_param("Shared_Buffers"));
        assert!(is_safe_param("WORK_MEM"));
    }

    #[test]
    fn test_production_hostname_check() {
        // Production hostname should be blocked
        assert!(check_production_hostname("postgresql://prod-db:5432/mydb", false).is_err());
        assert!(
            check_production_hostname("postgresql://production.example.com/mydb", false).is_err()
        );
        // Localhost should be fine
        assert!(check_production_hostname("postgresql://localhost/mydb", false).is_ok());
        // Force overrides the check
        assert!(check_production_hostname("postgresql://prod-db:5432/mydb", true).is_ok());
    }

    #[test]
    fn test_validate_recommendations() {
        let recs = vec![
            // Safe config change
            Recommendation::ConfigChange {
                parameter: "shared_buffers".into(),
                current_value: "128MB".into(),
                recommended_value: "1GB".into(),
                rationale: "More memory".into(),
            },
            // Unsafe config change (blocked param)
            Recommendation::ConfigChange {
                parameter: "data_directory".into(),
                current_value: "/var/lib/pg".into(),
                recommended_value: "/mnt/fast/pg".into(),
                rationale: "Faster disk".into(),
            },
            // Safe index creation
            Recommendation::CreateIndex {
                table: "orders".into(),
                columns: vec!["status".into()],
                index_type: None,
                sql: "CREATE INDEX idx_orders_status ON orders (status)".into(),
                rationale: "Frequent filter".into(),
            },
            // Unsafe schema change (DROP TABLE)
            Recommendation::SchemaChange {
                sql: "DROP TABLE old_data".into(),
                description: "Remove old table".into(),
                rationale: "Cleanup".into(),
            },
            // Safe query rewrite (always safe)
            Recommendation::QueryRewrite {
                original_sql: "SELECT *".into(),
                rewritten_sql: "SELECT id".into(),
                rationale: "Narrow select".into(),
            },
        ];

        let (safe, rejected) = validate_recommendations(&recs);
        assert_eq!(safe.len(), 3);
        assert_eq!(rejected.len(), 2);

        // Verify reasons
        assert!(rejected[0].1.contains("not on the safe allowlist"));
        assert!(rejected[1].1.contains("not on the allowed list"));
    }

    #[test]
    fn test_schema_change_allowlist() {
        // Allowed operations
        assert!(is_allowed_schema_sql("CREATE INDEX idx_foo ON bar (baz)"));
        assert!(is_allowed_schema_sql(
            "CREATE INDEX CONCURRENTLY idx_foo ON bar (baz)"
        ));
        assert!(is_allowed_schema_sql(
            "CREATE UNIQUE INDEX idx_foo ON bar (baz)"
        ));
        assert!(is_allowed_schema_sql("ANALYZE users"));
        assert!(is_allowed_schema_sql("ANALYZE"));
        assert!(is_allowed_schema_sql("REINDEX TABLE users"));
        assert!(is_allowed_schema_sql("REINDEX INDEX idx_foo"));

        // Blocked operations (not on allowlist)
        assert!(!is_allowed_schema_sql(
            "ALTER TABLE users ADD COLUMN archived boolean"
        ));
        assert!(!is_allowed_schema_sql("DROP TABLE users"));
        assert!(!is_allowed_schema_sql("CREATE TABLE foo (id int)"));
        assert!(!is_allowed_schema_sql("TRUNCATE orders"));
        assert!(!is_allowed_schema_sql("DROP INDEX idx_foo"));
        assert!(!is_allowed_schema_sql(
            "ALTER INDEX idx_foo RENAME TO idx_bar"
        ));
        assert!(!is_allowed_schema_sql("GRANT ALL ON TABLE users TO admin"));
    }

    #[test]
    fn test_validate_schema_change_uses_allowlist() {
        let recs = vec![
            Recommendation::SchemaChange {
                sql: "CREATE INDEX idx_test ON orders (status)".into(),
                description: "Add index".into(),
                rationale: "Speed up".into(),
            },
            Recommendation::SchemaChange {
                sql: "ALTER TABLE users ADD COLUMN archived boolean".into(),
                description: "Add column".into(),
                rationale: "Feature".into(),
            },
        ];

        let (safe, rejected) = validate_recommendations(&recs);
        assert_eq!(safe.len(), 1);
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].1.contains("not on the allowed list"));
    }
}
