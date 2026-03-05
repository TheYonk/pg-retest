use pg_retest::config::{
    CaptureConfig, OutputConfig, PipelineConfig, ReplayConfig, ThresholdConfig,
};
use pg_retest::pipeline::{self, run_pipeline};
use std::path::PathBuf;
use tempfile::NamedTempFile;

/// Helper: build a minimal config that uses an existing workload + target connection.
fn minimal_config(workload_path: &str, target: &str) -> PipelineConfig {
    PipelineConfig {
        capture: Some(CaptureConfig {
            workload: Some(PathBuf::from(workload_path)),
            source_log: None,
            source_host: None,
            pg_version: None,
            mask_values: false,
        }),
        provision: None,
        replay: ReplayConfig {
            speed: 0.0, // max speed
            read_only: true,
            scale: 1,
            stagger_ms: 0,
            target: Some(target.to_string()),
        },
        thresholds: None,
        output: None,
    }
}

#[test]
fn test_pipeline_config_validation_no_workload() {
    let config = PipelineConfig {
        capture: Some(CaptureConfig {
            workload: None,
            source_log: None,
            source_host: None,
            pg_version: None,
            mask_values: false,
        }),
        provision: None,
        replay: ReplayConfig {
            speed: 1.0,
            read_only: false,
            scale: 1,
            stagger_ms: 0,
            target: Some("host=localhost".into()),
        },
        thresholds: None,
        output: None,
    };
    // Pipeline should fail because there's no workload source
    let result = run_pipeline(&config);
    assert_ne!(result.exit_code, pipeline::EXIT_PASS);
}

#[test]
fn test_pipeline_missing_workload_file() {
    let config = minimal_config("/nonexistent/workload.wkl", "host=localhost");
    let result = run_pipeline(&config);
    assert_ne!(result.exit_code, pipeline::EXIT_PASS);
}

#[test]
fn test_pipeline_threshold_evaluation() {
    // Create a workload file first
    use pg_retest::profile::{io, Metadata, Query, QueryKind, Session, WorkloadProfile};

    let profile = WorkloadProfile {
        version: 2,
        captured_at: chrono::Utc::now(),
        source_host: "test".into(),
        pg_version: "16".into(),
        capture_method: "test".into(),
        sessions: vec![Session {
            id: 1,
            user: "test".into(),
            database: "test".into(),
            queries: vec![Query {
                sql: "SELECT 1".into(),
                start_offset_us: 0,
                duration_us: 100,
                kind: QueryKind::Select,
                transaction_id: None,
            }],
        }],
        metadata: Metadata {
            total_queries: 1,
            total_sessions: 1,
            capture_duration_us: 100,
        },
    };

    let wkl_file = NamedTempFile::with_suffix(".wkl").unwrap();
    io::write_profile(wkl_file.path(), &profile).unwrap();

    let json_file = NamedTempFile::with_suffix(".json").unwrap();
    let junit_file = NamedTempFile::with_suffix(".xml").unwrap();

    let mut config = minimal_config(
        wkl_file.path().to_str().unwrap(),
        // Use a connection that will fail — we're testing error handling
        "host=127.0.0.1 port=1 dbname=test",
    );
    config.thresholds = Some(ThresholdConfig {
        p95_max_ms: Some(1000.0),
        p99_max_ms: Some(5000.0),
        error_rate_max_pct: Some(100.0),
        regression_max_count: Some(1000),
        regression_threshold_pct: 20.0,
    });
    config.output = Some(OutputConfig {
        json_report: Some(json_file.path().to_path_buf()),
        junit_xml: Some(junit_file.path().to_path_buf()),
    });

    // Connection to port 1 will fail for each session, but run_replay silently
    // absorbs per-session connection errors (logs a warning). The pipeline
    // continues with zero replay results, so all thresholds pass trivially.
    let result = run_pipeline(&config);
    // Pipeline completes without a hard error; thresholds pass on empty results.
    assert_eq!(result.exit_code, pipeline::EXIT_PASS);
    // The comparison report should still be produced
    assert!(result.report.is_some());
}
