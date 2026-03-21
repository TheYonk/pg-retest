use pg_retest::tuner::safety::*;
use pg_retest::tuner::types::*;

#[test]
fn test_safety_validates_allowlist() {
    let recs = vec![
        Recommendation::ConfigChange {
            parameter: "shared_buffers".into(),
            current_value: "128MB".into(),
            recommended_value: "1GB".into(),
            rationale: "".into(),
        },
        Recommendation::ConfigChange {
            parameter: "data_directory".into(),
            current_value: "/data".into(),
            recommended_value: "/tmp".into(),
            rationale: "".into(),
        },
    ];
    let (safe, rejected) = validate_recommendations(&recs);
    assert_eq!(safe.len(), 1);
    assert_eq!(rejected.len(), 1);
}

#[test]
fn test_production_hostname_blocked() {
    assert!(check_production_hostname("host=production.db", false).is_err());
    assert!(check_production_hostname("host=test.db", false).is_ok());
    assert!(check_production_hostname("host=production.db", true).is_ok());
}

#[test]
fn test_tuning_report_json_output() {
    let report = TuningReport {
        workload: "test.wkl".into(),
        target: "postgresql://localhost/test".into(),
        provider: "claude".into(),
        hint: Some("focus on reads".into()),
        iterations: vec![TuningIteration {
            iteration: 1,
            recommendations: vec![Recommendation::ConfigChange {
                parameter: "work_mem".into(),
                current_value: "4MB".into(),
                recommended_value: "64MB".into(),
                rationale: "Large sorts".into(),
            }],
            applied: vec![AppliedChange {
                recommendation: Recommendation::ConfigChange {
                    parameter: "work_mem".into(),
                    current_value: "4MB".into(),
                    recommended_value: "64MB".into(),
                    rationale: "Large sorts".into(),
                },
                success: true,
                error: None,
                rollback_sql: Some("ALTER SYSTEM SET work_mem = '4MB'".into()),
            }],
            comparison: Some(ComparisonSummary {
                p50_change_pct: -5.0,
                p95_change_pct: -12.0,
                p99_change_pct: -8.0,
                regressions: 0,
                improvements: 3,
                errors_delta: 0,
            }),
            llm_feedback: "p95 improved by 12%".into(),
        }],
        total_improvement_pct: 12.0,
        all_changes: vec![],
    };

    let json = serde_json::to_string_pretty(&report).unwrap();
    let parsed: TuningReport = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.iterations.len(), 1);
    assert_eq!(parsed.total_improvement_pct, 12.0);
    assert!(
        parsed.iterations[0]
            .comparison
            .as_ref()
            .unwrap()
            .p95_change_pct
            < 0.0
    );
}

#[test]
fn test_schema_change_allowlist_integration() {
    let recs = vec![
        Recommendation::SchemaChange {
            sql: "DROP TABLE users".into(),
            description: "Remove".into(),
            rationale: "".into(),
        },
        Recommendation::SchemaChange {
            sql: "ALTER TABLE users ADD COLUMN age int".into(),
            description: "Add column".into(),
            rationale: "".into(),
        },
        Recommendation::CreateIndex {
            table: "orders".into(),
            columns: vec!["status".into()],
            index_type: None,
            sql: "CREATE INDEX idx_status ON orders (status)".into(),
            rationale: "".into(),
        },
    ];
    let (safe, rejected) = validate_recommendations(&recs);
    assert_eq!(safe.len(), 1); // only CREATE INDEX is on allowlist
    assert_eq!(rejected.len(), 2); // DROP TABLE + ALTER TABLE not on allowlist
}
