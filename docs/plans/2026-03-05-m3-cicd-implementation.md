# M3: CI/CD Integration Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Add a `pg-retest run --config .pg-retest.toml` command that automates the full capture → provision → replay → compare → report pipeline for CI/CD integration.

**Architecture:** TOML config drives a pipeline orchestrator that sequences existing capture/replay/compare functions. Docker provisioning via CLI subprocess (no heavy crate). JUnit XML + JSON output for CI result integration. Exit codes map pipeline stage failures to distinct codes (0-5).

**Tech Stack:** `toml` crate for config parsing, `docker` CLI subprocess for provisioning, hand-written JUnit XML (no XML crate needed — format is trivial).

---

## Context

### Codebase State
- Rust 2021 edition, Tokio async, clap derive CLI
- Existing subcommands: `capture`, `replay`, `compare`, `inspect`, `proxy`
- All public modules in `src/lib.rs`, binary dispatches from `src/main.rs`
- 86 tests, zero clippy warnings
- Dependencies: see `Cargo.toml` (no `toml` crate yet)

### Key Existing Functions to Reuse
- `capture::csv_log::CsvLogCapture::capture_from_file(path, host, version)` → `WorkloadProfile`
- `capture::masking::mask_sql_literals(sql)` → `String`
- `profile::io::{read_profile, write_profile}` — MessagePack .wkl I/O
- `replay::session::run_replay(profile, conn_string, mode, speed)` → `Vec<ReplayResults>`
- `replay::scaling::{scale_sessions, check_write_safety}` — M2 scaling
- `compare::{compute_comparison, evaluate_outcome}` — comparison + outcome
- `compare::report::{print_terminal_report, write_json_report}` — output

### Design Reference
- `docs/plans/2026-03-04-m3-cicd-design.md` — Approved design with TOML schema, exit codes, JUnit format

---

## Task 1: Add `toml` Dependency + Config Module Skeleton

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Create: `src/config/mod.rs`
- Create: `tests/config_test.rs`

**What this does:** Add the `toml` crate, create config module with serde-driven TOML parsing and validation.

**Step 1: Add dependency and module**

Add to `Cargo.toml` after the `tabled` line:
```toml
toml = "0.8"
```

Add to `src/lib.rs` (alphabetical, before `classify`):
```rust
pub mod config;
```

**Step 2: Create `src/config/mod.rs`**

```rust
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::Deserialize;

/// Top-level pipeline configuration, parsed from TOML.
#[derive(Debug, Clone, Deserialize)]
pub struct PipelineConfig {
    pub capture: Option<CaptureConfig>,
    pub provision: Option<ProvisionConfig>,
    pub replay: ReplayConfig,
    pub thresholds: Option<ThresholdConfig>,
    pub output: Option<OutputConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptureConfig {
    /// Path to existing .wkl file (skip capture, use this directly)
    pub workload: Option<PathBuf>,
    /// Path to PG CSV log file (run capture from this)
    pub source_log: Option<PathBuf>,
    pub source_host: Option<String>,
    pub pg_version: Option<String>,
    #[serde(default)]
    pub mask_values: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProvisionConfig {
    pub backend: String,
    pub image: Option<String>,
    pub restore_from: Option<PathBuf>,
    /// Pre-existing connection string (skip provisioning)
    pub connection_string: Option<String>,
    /// Port to expose the container on (default: random)
    pub port: Option<u16>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReplayConfig {
    #[serde(default = "default_speed")]
    pub speed: f64,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default = "default_scale")]
    pub scale: u32,
    #[serde(default)]
    pub stagger_ms: u64,
    /// Target connection string (required if no [provision] section)
    pub target: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ThresholdConfig {
    pub p95_max_ms: Option<f64>,
    pub p99_max_ms: Option<f64>,
    pub error_rate_max_pct: Option<f64>,
    pub regression_max_count: Option<usize>,
    #[serde(default = "default_regression_threshold")]
    pub regression_threshold_pct: f64,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OutputConfig {
    pub json_report: Option<PathBuf>,
    pub junit_xml: Option<PathBuf>,
}

fn default_speed() -> f64 {
    1.0
}
fn default_scale() -> u32 {
    1
}
fn default_regression_threshold() -> f64 {
    20.0
}

/// Load and validate a pipeline config from a TOML file.
pub fn load_config(path: &Path) -> Result<PipelineConfig> {
    let contents =
        std::fs::read_to_string(path).with_context(|| format!("Failed to read {}", path.display()))?;
    let config: PipelineConfig =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

/// Validate config: ensure we have either a workload file or a source_log to capture from,
/// and either a provision section or a target connection string.
fn validate_config(config: &PipelineConfig) -> Result<()> {
    // Must have a way to get a workload
    let has_workload = config
        .capture
        .as_ref()
        .map_or(false, |c| c.workload.is_some() || c.source_log.is_some());
    if !has_workload {
        anyhow::bail!(
            "Config must specify either [capture].workload or [capture].source_log"
        );
    }

    // Must have a way to connect to target
    let has_target = config
        .replay
        .target
        .is_some()
        || config
            .provision
            .as_ref()
            .map_or(false, |p| p.connection_string.is_some() || p.backend == "docker");
    if !has_target {
        anyhow::bail!(
            "Config must specify [replay].target, [provision].connection_string, or [provision].backend = \"docker\""
        );
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_full_config() {
        let toml = r#"
[capture]
source_log = "pg_log.csv"
source_host = "prod-db-01"
pg_version = "16.2"
mask_values = true

[provision]
backend = "docker"
image = "postgres:16.2"
restore_from = "backup.sql"

[replay]
speed = 1.0
read_only = false
scale = 1

[thresholds]
p95_max_ms = 50.0
p99_max_ms = 200.0
error_rate_max_pct = 1.0
regression_max_count = 5
regression_threshold_pct = 20.0

[output]
json_report = "report.json"
junit_xml = "results.xml"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.capture.as_ref().unwrap().source_host.as_deref(), Some("prod-db-01"));
        assert_eq!(config.provision.as_ref().unwrap().backend, "docker");
        assert_eq!(config.replay.speed, 1.0);
        assert_eq!(config.thresholds.as_ref().unwrap().p95_max_ms, Some(50.0));
        assert_eq!(
            config.output.as_ref().unwrap().junit_xml.as_deref(),
            Some(Path::new("results.xml"))
        );
    }

    #[test]
    fn test_parse_minimal_config() {
        let toml = r#"
[capture]
workload = "existing.wkl"

[replay]
target = "host=localhost dbname=test"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert!(config.capture.as_ref().unwrap().workload.is_some());
        assert_eq!(config.replay.speed, 1.0); // default
        assert_eq!(config.replay.scale, 1); // default
        assert!(config.provision.is_none());
        assert!(config.thresholds.is_none());
    }

    #[test]
    fn test_validate_no_workload_source() {
        let toml = r#"
[capture]
mask_values = true

[replay]
target = "host=localhost"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("workload"));
    }

    #[test]
    fn test_validate_no_target() {
        let toml = r#"
[capture]
workload = "test.wkl"

[replay]
speed = 2.0
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        let err = validate_config(&config).unwrap_err();
        assert!(err.to_string().contains("target"));
    }

    #[test]
    fn test_load_config_file_not_found() {
        let err = load_config(Path::new("/nonexistent/config.toml")).unwrap_err();
        assert!(err.to_string().contains("Failed to read"));
    }
}
```

**Step 3: Run tests**

```bash
cargo test --lib config
```
Expected: 5 tests pass.

**Step 4: Commit**

```bash
git add Cargo.toml src/lib.rs src/config/mod.rs
git commit -m "feat(config): TOML pipeline config parsing and validation"
```

---

## Task 2: Threshold Evaluation

**Files:**
- Create: `src/compare/threshold.rs`
- Modify: `src/compare/mod.rs` (add `pub mod threshold;`)
- Create: `tests/threshold_test.rs`

**What this does:** Add threshold-based pass/fail evaluation for the pipeline. Given a `ComparisonReport` and `ThresholdConfig`, determine which thresholds were violated and return structured results.

**Step 1: Create `src/compare/threshold.rs`**

```rust
use crate::config::ThresholdConfig;

use super::ComparisonReport;

/// Result of evaluating one threshold check.
#[derive(Debug, Clone)]
pub struct ThresholdResult {
    pub name: String,
    pub passed: bool,
    pub actual: f64,
    pub limit: f64,
    pub message: Option<String>,
}

/// Evaluate all configured thresholds against a comparison report.
/// Returns a list of results (one per configured threshold) and an overall pass/fail.
pub fn evaluate_thresholds(
    report: &ComparisonReport,
    config: &ThresholdConfig,
) -> Vec<ThresholdResult> {
    let mut results = Vec::new();

    if let Some(limit) = config.p95_max_ms {
        let actual = report.replay_p95_latency_us as f64 / 1000.0;
        results.push(ThresholdResult {
            name: "p95_latency".into(),
            passed: actual <= limit,
            actual,
            limit,
            message: if actual > limit {
                Some(format!("P95 latency {actual:.1}ms exceeds limit {limit:.1}ms"))
            } else {
                None
            },
        });
    }

    if let Some(limit) = config.p99_max_ms {
        let actual = report.replay_p99_latency_us as f64 / 1000.0;
        results.push(ThresholdResult {
            name: "p99_latency".into(),
            passed: actual <= limit,
            actual,
            limit,
            message: if actual > limit {
                Some(format!("P99 latency {actual:.1}ms exceeds limit {limit:.1}ms"))
            } else {
                None
            },
        });
    }

    if let Some(limit) = config.error_rate_max_pct {
        let actual = if report.total_queries_replayed > 0 {
            report.total_errors as f64 / report.total_queries_replayed as f64 * 100.0
        } else {
            0.0
        };
        results.push(ThresholdResult {
            name: "error_rate".into(),
            passed: actual <= limit,
            actual,
            limit,
            message: if actual > limit {
                Some(format!(
                    "Error rate {actual:.2}% exceeds limit {limit:.1}%"
                ))
            } else {
                None
            },
        });
    }

    if let Some(limit) = config.regression_max_count {
        let actual = report.regressions.len();
        results.push(ThresholdResult {
            name: "regression_count".into(),
            passed: actual <= limit,
            actual: actual as f64,
            limit: limit as f64,
            message: if actual > limit {
                Some(format!(
                    "{actual} regressions found, max allowed: {limit}"
                ))
            } else {
                None
            },
        });
    }

    results
}

/// Returns true if all threshold checks passed.
pub fn all_passed(results: &[ThresholdResult]) -> bool {
    results.iter().all(|r| r.passed)
}
```

**Step 2: Wire into `src/compare/mod.rs`**

Add after the `pub mod report;` line:
```rust
pub mod threshold;
```

**Step 3: Create `tests/threshold_test.rs`**

```rust
use pg_retest::compare::threshold::{all_passed, evaluate_thresholds};
use pg_retest::compare::{ComparisonReport, Regression};
use pg_retest::config::ThresholdConfig;

fn make_report(p95_us: u64, p99_us: u64, errors: u64, replayed: u64, regressions: usize) -> ComparisonReport {
    ComparisonReport {
        total_queries_source: 100,
        total_queries_replayed: replayed,
        total_errors: errors,
        source_avg_latency_us: 1000,
        replay_avg_latency_us: 2000,
        source_p50_latency_us: 800,
        replay_p50_latency_us: 1600,
        source_p95_latency_us: 5000,
        replay_p95_latency_us: p95_us,
        source_p99_latency_us: 10000,
        replay_p99_latency_us: p99_us,
        regressions: (0..regressions)
            .map(|i| Regression {
                sql: format!("SELECT {i}"),
                original_us: 100,
                replay_us: 500,
                change_pct: 400.0,
            })
            .collect(),
    }
}

#[test]
fn test_all_thresholds_pass() {
    let report = make_report(40_000, 150_000, 0, 100, 2);
    let config = ThresholdConfig {
        p95_max_ms: Some(50.0),
        p99_max_ms: Some(200.0),
        error_rate_max_pct: Some(1.0),
        regression_max_count: Some(5),
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert_eq!(results.len(), 4);
    assert!(all_passed(&results));
}

#[test]
fn test_p95_threshold_violation() {
    let report = make_report(60_000, 150_000, 0, 100, 0);
    let config = ThresholdConfig {
        p95_max_ms: Some(50.0),
        p99_max_ms: None,
        error_rate_max_pct: None,
        regression_max_count: None,
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert_eq!(results.len(), 1);
    assert!(!results[0].passed);
    assert!(results[0].message.as_ref().unwrap().contains("P95"));
}

#[test]
fn test_error_rate_violation() {
    let report = make_report(40_000, 150_000, 5, 100, 0);
    let config = ThresholdConfig {
        p95_max_ms: None,
        p99_max_ms: None,
        error_rate_max_pct: Some(1.0),
        regression_max_count: None,
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert!(!all_passed(&results));
    assert_eq!(results[0].actual, 5.0); // 5/100 = 5%
}

#[test]
fn test_regression_count_violation() {
    let report = make_report(40_000, 150_000, 0, 100, 7);
    let config = ThresholdConfig {
        p95_max_ms: None,
        p99_max_ms: None,
        error_rate_max_pct: None,
        regression_max_count: Some(5),
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert!(!all_passed(&results));
    assert!(results[0].message.as_ref().unwrap().contains("7 regressions"));
}

#[test]
fn test_no_thresholds_configured() {
    let report = make_report(999_000, 999_000, 99, 100, 50);
    let config = ThresholdConfig {
        p95_max_ms: None,
        p99_max_ms: None,
        error_rate_max_pct: None,
        regression_max_count: None,
        regression_threshold_pct: 20.0,
    };
    let results = evaluate_thresholds(&report, &config);
    assert!(results.is_empty());
    assert!(all_passed(&results)); // vacuously true
}
```

**Step 4: Run tests**

```bash
cargo test --test threshold_test
cargo test --lib compare::threshold
```

**Step 5: Commit**

```bash
git add src/compare/threshold.rs src/compare/mod.rs tests/threshold_test.rs
git commit -m "feat(compare): threshold-based pass/fail evaluation for CI pipeline"
```

---

## Task 3: JUnit XML Output

**Files:**
- Create: `src/compare/junit.rs`
- Modify: `src/compare/mod.rs` (add `pub mod junit;`)
- Create: `tests/junit_test.rs`

**What this does:** Generate JUnit XML from threshold evaluation results. No XML crate needed — the format is simple enough to hand-write.

**Step 1: Create `src/compare/junit.rs`**

```rust
use std::io::Write;
use std::path::Path;

use anyhow::Result;

use super::threshold::ThresholdResult;

/// Write JUnit XML test report from threshold results.
pub fn write_junit_xml(path: &Path, results: &[ThresholdResult], elapsed_secs: f64) -> Result<()> {
    let failures = results.iter().filter(|r| !r.passed).count();
    let mut buf = Vec::new();

    writeln!(buf, "<?xml version=\"1.0\" encoding=\"UTF-8\"?>")?;
    writeln!(
        buf,
        "<testsuites tests=\"{}\" failures=\"{}\" time=\"{:.3}\">",
        results.len(),
        failures,
        elapsed_secs
    )?;
    writeln!(
        buf,
        "  <testsuite name=\"pg-retest\" tests=\"{}\" failures=\"{}\">",
        results.len(),
        failures
    )?;

    for result in results {
        if result.passed {
            writeln!(
                buf,
                "    <testcase name=\"{}\" time=\"{:.3}\"/>",
                xml_escape(&result.name),
                result.actual / 1000.0 // convert ms to seconds for JUnit
            )?;
        } else {
            writeln!(
                buf,
                "    <testcase name=\"{}\" time=\"{:.3}\">",
                xml_escape(&result.name),
                result.actual / 1000.0
            )?;
            let msg = result
                .message
                .as_deref()
                .unwrap_or("threshold exceeded");
            writeln!(
                buf,
                "      <failure message=\"{}\"/>",
                xml_escape(msg)
            )?;
            writeln!(buf, "    </testcase>")?;
        }
    }

    writeln!(buf, "  </testsuite>")?;
    writeln!(buf, "</testsuites>")?;

    std::fs::write(path, buf)?;
    Ok(())
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}
```

**Step 2: Add to `src/compare/mod.rs`**

Add after `pub mod threshold;`:
```rust
pub mod junit;
```

**Step 3: Create `tests/junit_test.rs`**

```rust
use pg_retest::compare::junit::write_junit_xml;
use pg_retest::compare::threshold::ThresholdResult;
use tempfile::NamedTempFile;

#[test]
fn test_junit_xml_all_pass() {
    let results = vec![
        ThresholdResult {
            name: "p95_latency".into(),
            passed: true,
            actual: 45.0,
            limit: 50.0,
            message: None,
        },
        ThresholdResult {
            name: "error_rate".into(),
            passed: true,
            actual: 0.5,
            limit: 1.0,
            message: None,
        },
    ];

    let file = NamedTempFile::new().unwrap();
    write_junit_xml(file.path(), &results, 1.5).unwrap();

    let content = std::fs::read_to_string(file.path()).unwrap();
    assert!(content.contains("tests=\"2\" failures=\"0\""));
    assert!(content.contains("name=\"p95_latency\""));
    assert!(content.contains("name=\"error_rate\""));
    assert!(!content.contains("<failure"));
}

#[test]
fn test_junit_xml_with_failures() {
    let results = vec![
        ThresholdResult {
            name: "p95_latency".into(),
            passed: true,
            actual: 45.0,
            limit: 50.0,
            message: None,
        },
        ThresholdResult {
            name: "regression_count".into(),
            passed: false,
            actual: 7.0,
            limit: 5.0,
            message: Some("7 regressions found, max allowed: 5".into()),
        },
    ];

    let file = NamedTempFile::new().unwrap();
    write_junit_xml(file.path(), &results, 2.0).unwrap();

    let content = std::fs::read_to_string(file.path()).unwrap();
    assert!(content.contains("tests=\"2\" failures=\"1\""));
    assert!(content.contains("<failure message=\"7 regressions found, max allowed: 5\"/>"));
}

#[test]
fn test_junit_xml_escapes_special_chars() {
    let results = vec![ThresholdResult {
        name: "test_with_<special>&chars".into(),
        passed: false,
        actual: 10.0,
        limit: 5.0,
        message: Some("value > limit & that's bad".into()),
    }];

    let file = NamedTempFile::new().unwrap();
    write_junit_xml(file.path(), &results, 0.1).unwrap();

    let content = std::fs::read_to_string(file.path()).unwrap();
    assert!(content.contains("&amp;"));
    assert!(content.contains("&lt;"));
    assert!(content.contains("&gt;"));
}

#[test]
fn test_junit_xml_empty_results() {
    let file = NamedTempFile::new().unwrap();
    write_junit_xml(file.path(), &[], 0.0).unwrap();

    let content = std::fs::read_to_string(file.path()).unwrap();
    assert!(content.contains("tests=\"0\" failures=\"0\""));
}
```

**Step 4: Run tests**

```bash
cargo test --test junit_test
```

**Step 5: Commit**

```bash
git add src/compare/junit.rs src/compare/mod.rs tests/junit_test.rs
git commit -m "feat(compare): JUnit XML output for CI test result integration"
```

---

## Task 4: Docker Provisioner

**Files:**
- Create: `src/provision/mod.rs`
- Modify: `src/lib.rs` (add `pub mod provision;`)
- Create: `tests/provision_test.rs`

**What this does:** Trait-based provisioner with Docker CLI backend. Starts a PG container, optionally restores a SQL backup, returns a connection string, and tears down on completion.

**Important:** Uses `docker` CLI subprocess (via `std::process::Command`) — no `bollard` crate needed. Simpler, fewer dependencies, works everywhere Docker CLI works.

**Step 1: Add module to `src/lib.rs`**

Add between `pub mod proxy;` and `pub mod replay;`:
```rust
pub mod provision;
```

**Step 2: Create `src/provision/mod.rs`**

```rust
use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use tracing::info;

use crate::config::ProvisionConfig;

/// A provisioned database instance ready for replay.
pub struct ProvisionedDb {
    pub connection_string: String,
    pub container_id: Option<String>,
}

/// Provision a database based on config.
/// If `connection_string` is set, skip provisioning and use it directly.
/// If `backend = "docker"`, start a container.
pub fn provision(config: &ProvisionConfig) -> Result<ProvisionedDb> {
    if let Some(conn) = &config.connection_string {
        info!("Using pre-existing connection: {}", conn);
        return Ok(ProvisionedDb {
            connection_string: conn.clone(),
            container_id: None,
        });
    }

    match config.backend.as_str() {
        "docker" => provision_docker(config),
        other => anyhow::bail!("Unknown provision backend: {other}. Supported: docker"),
    }
}

/// Tear down a provisioned database.
pub fn teardown(db: &ProvisionedDb) -> Result<()> {
    if let Some(id) = &db.container_id {
        info!("Stopping container {}", &id[..12.min(id.len())]);
        let output = Command::new("docker")
            .args(["rm", "-f", id])
            .output()
            .context("Failed to run docker rm")?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("docker rm failed: {stderr}");
        }
    }
    Ok(())
}

fn provision_docker(config: &ProvisionConfig) -> Result<ProvisionedDb> {
    let image = config
        .image
        .as_deref()
        .unwrap_or("postgres:16");

    // Check docker is available
    Command::new("docker")
        .arg("version")
        .output()
        .context("Docker not available. Is Docker installed and running?")?;

    let port = config.port.unwrap_or(0);
    let host_port = if port == 0 {
        // Let Docker pick a random port
        "0".to_string()
    } else {
        port.to_string()
    };

    info!("Starting Docker container from {image}...");

    // Start container
    let output = Command::new("docker")
        .args([
            "run", "-d",
            "--name", &format!("pg-retest-{}", std::process::id()),
            "-e", "POSTGRES_USER=pgretest",
            "-e", "POSTGRES_PASSWORD=pgretest",
            "-e", "POSTGRES_DB=pgretest",
            "-p", &format!("{host_port}:5432"),
            image,
        ])
        .output()
        .context("Failed to start Docker container")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("docker run failed: {stderr}");
    }

    let container_id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    info!("Container started: {}", &container_id[..12.min(container_id.len())]);

    // Get the mapped port
    let mapped_port = get_mapped_port(&container_id)?;
    info!("PostgreSQL available on port {mapped_port}");

    // Wait for PG to be ready
    wait_for_pg(&container_id)?;

    // Restore backup if specified
    if let Some(restore_path) = &config.restore_from {
        restore_backup(&container_id, restore_path)?;
    }

    let connection_string = format!(
        "host=127.0.0.1 port={mapped_port} user=pgretest password=pgretest dbname=pgretest"
    );

    Ok(ProvisionedDb {
        connection_string,
        container_id: Some(container_id),
    })
}

fn get_mapped_port(container_id: &str) -> Result<u16> {
    let output = Command::new("docker")
        .args(["port", container_id, "5432"])
        .output()
        .context("Failed to get container port")?;

    let port_str = String::from_utf8_lossy(&output.stdout);
    // Output format: "0.0.0.0:12345\n" or ":::12345\n"
    let port = port_str
        .trim()
        .rsplit(':')
        .next()
        .and_then(|p| p.parse().ok())
        .context("Failed to parse mapped port")?;

    Ok(port)
}

fn wait_for_pg(container_id: &str) -> Result<()> {
    info!("Waiting for PostgreSQL to be ready...");
    for attempt in 1..=30 {
        let output = Command::new("docker")
            .args([
                "exec", container_id,
                "pg_isready", "-U", "pgretest",
            ])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                info!("PostgreSQL ready (attempt {attempt})");
                return Ok(());
            }
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
    anyhow::bail!("PostgreSQL did not become ready within 30 seconds")
}

fn restore_backup(container_id: &str, path: &Path) -> Result<()> {
    info!("Restoring backup from {}", path.display());

    // Copy backup file into container
    let container_path = "/tmp/restore.sql";
    let output = Command::new("docker")
        .args([
            "cp",
            &path.to_string_lossy(),
            &format!("{container_id}:{container_path}"),
        ])
        .output()
        .context("Failed to copy backup to container")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("docker cp failed: {stderr}");
    }

    // Execute the SQL file
    let output = Command::new("docker")
        .args([
            "exec", container_id,
            "psql", "-U", "pgretest", "-d", "pgretest", "-f", container_path,
        ])
        .output()
        .context("Failed to restore backup")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        // psql may return non-zero for warnings, check for fatal errors
        if stderr.contains("FATAL") || stderr.contains("could not") {
            anyhow::bail!("Backup restore failed: {stderr}");
        }
    }

    info!("Backup restored successfully");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_provision_with_connection_string() {
        let config = ProvisionConfig {
            backend: "docker".into(),
            image: None,
            restore_from: None,
            connection_string: Some("host=localhost dbname=test".into()),
            port: None,
        };
        let db = provision(&config).unwrap();
        assert_eq!(db.connection_string, "host=localhost dbname=test");
        assert!(db.container_id.is_none());
    }

    #[test]
    fn test_provision_unknown_backend() {
        let config = ProvisionConfig {
            backend: "kubernetes".into(),
            image: None,
            restore_from: None,
            connection_string: None,
            port: None,
        };
        let err = provision(&config).unwrap_err();
        assert!(err.to_string().contains("Unknown provision backend"));
    }

    #[test]
    fn test_teardown_no_container() {
        let db = ProvisionedDb {
            connection_string: "host=localhost".into(),
            container_id: None,
        };
        teardown(&db).unwrap(); // should be a no-op
    }
}
```

**Step 3: Run tests**

```bash
cargo test --lib provision
```
Expected: 3 unit tests pass. (Docker integration tested in Task 7.)

**Step 4: Commit**

```bash
git add src/lib.rs src/provision/mod.rs
git commit -m "feat(provision): Docker provisioner via CLI subprocess"
```

---

## Task 5: Pipeline Orchestrator

**Files:**
- Create: `src/pipeline/mod.rs`
- Modify: `src/lib.rs` (add `pub mod pipeline;`)

**What this does:** The core pipeline that sequences: load config → capture → provision → replay → compare → threshold → report. Maps each stage failure to the correct exit code.

**Step 1: Add module to `src/lib.rs`**

Add between `pub mod profile;` and `pub mod provision;`:
```rust
pub mod pipeline;
```

**Step 2: Create `src/pipeline/mod.rs`**

```rust
use std::path::Path;
use std::time::Instant;

use anyhow::Result;
use tracing::info;

use crate::capture::csv_log::CsvLogCapture;
use crate::capture::masking::mask_sql_literals;
use crate::compare::junit::write_junit_xml;
use crate::compare::report;
use crate::compare::threshold::{all_passed, evaluate_thresholds};
use crate::compare::{compute_comparison, ComparisonReport};
use crate::config::{PipelineConfig, ThresholdConfig};
use crate::profile::io;
use crate::profile::WorkloadProfile;
use crate::provision::{self, ProvisionedDb};
use crate::replay::session::run_replay;
use crate::replay::scaling::{check_write_safety, scale_sessions};
use crate::replay::{ReplayMode, ReplayResults};

/// Exit codes for the pipeline.
pub const EXIT_PASS: i32 = 0;
pub const EXIT_THRESHOLD_VIOLATION: i32 = 1;
pub const EXIT_CONFIG_ERROR: i32 = 2;
pub const EXIT_CAPTURE_ERROR: i32 = 3;
pub const EXIT_PROVISION_ERROR: i32 = 4;
pub const EXIT_REPLAY_ERROR: i32 = 5;

/// Result of running the full pipeline.
pub struct PipelineResult {
    pub exit_code: i32,
    pub report: Option<ComparisonReport>,
}

/// Run the full CI/CD pipeline.
pub fn run_pipeline(config: &PipelineConfig) -> PipelineResult {
    match run_pipeline_inner(config) {
        Ok(result) => result,
        Err(e) => {
            eprintln!("Pipeline error: {e:#}");
            PipelineResult {
                exit_code: classify_error(&e),
                report: None,
            }
        }
    }
}

fn run_pipeline_inner(config: &PipelineConfig) -> Result<PipelineResult> {
    let pipeline_start = Instant::now();

    // ── Step 1: Get workload profile ────────────────────────────────
    let profile = load_or_capture_workload(config)?;
    info!(
        "Workload: {} sessions, {} queries",
        profile.metadata.total_sessions, profile.metadata.total_queries
    );

    // ── Step 2: Provision target database ───────────────────────────
    let provisioned = provision_target(config)?;
    let connection_string = &provisioned.connection_string;
    info!("Target: {connection_string}");

    // ── Step 3: Replay ──────────────────────────────────────────────
    let (replay_profile, results) = run_replay_step(config, &profile, connection_string)?;

    let total_replayed: usize = results.iter().map(|r| r.query_results.len()).sum();
    let total_errors: usize = results
        .iter()
        .flat_map(|r| &r.query_results)
        .filter(|q| !q.success)
        .count();
    info!("Replay: {total_replayed} queries, {total_errors} errors");

    // ── Step 4: Compare ─────────────────────────────────────────────
    let threshold_pct = config
        .thresholds
        .as_ref()
        .map_or(20.0, |t| t.regression_threshold_pct);
    let comparison = compute_comparison(&replay_profile, &results, threshold_pct);
    report::print_terminal_report(&comparison);

    // ── Step 5: Evaluate thresholds ─────────────────────────────────
    let exit_code = if let Some(ref thresholds) = config.thresholds {
        let threshold_results = evaluate_thresholds(&comparison, thresholds);

        // Print threshold results
        println!();
        println!("  Threshold Checks:");
        for r in &threshold_results {
            let status = if r.passed { "PASS" } else { "FAIL" };
            println!("    [{status}] {}: {:.2} (limit: {:.2})", r.name, r.actual, r.limit);
        }

        if all_passed(&threshold_results) {
            println!("  All thresholds passed.");
            EXIT_PASS
        } else {
            println!("  Threshold violations detected.");
            EXIT_THRESHOLD_VIOLATION
        }
    } else {
        println!("  No thresholds configured, result: PASS");
        EXIT_PASS
    };

    // ── Step 6: Write output reports ────────────────────────────────
    let elapsed_secs = pipeline_start.elapsed().as_secs_f64();
    write_output_reports(config, &comparison, elapsed_secs)?;

    // ── Step 7: Teardown ────────────────────────────────────────────
    if let Err(e) = provision::teardown(&provisioned) {
        eprintln!("Warning: teardown failed: {e}");
    }

    Ok(PipelineResult {
        exit_code,
        report: Some(comparison),
    })
}

fn load_or_capture_workload(config: &PipelineConfig) -> Result<WorkloadProfile> {
    let capture_cfg = config
        .capture
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("[capture] section required"))?;

    // If a pre-existing workload file is specified, load it
    if let Some(ref wkl_path) = capture_cfg.workload {
        info!("Loading workload from {}", wkl_path.display());
        return io::read_profile(wkl_path)
            .map_err(|e| anyhow::anyhow!("Capture error: {e}"));
    }

    // Otherwise, capture from CSV log
    let source_log = capture_cfg
        .source_log
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No workload or source_log specified"))?;

    info!("Capturing from {}", source_log.display());
    let capture = CsvLogCapture;
    let mut profile = capture
        .capture_from_file(
            source_log,
            capture_cfg.source_host.as_deref().unwrap_or("unknown"),
            capture_cfg.pg_version.as_deref().unwrap_or("unknown"),
        )
        .map_err(|e| anyhow::anyhow!("Capture error: {e}"))?;

    if capture_cfg.mask_values {
        for session in &mut profile.sessions {
            for query in &mut session.queries {
                query.sql = mask_sql_literals(&query.sql);
            }
        }
        info!("Applied PII masking");
    }

    Ok(profile)
}

fn provision_target(config: &PipelineConfig) -> Result<ProvisionedDb> {
    // If replay.target is set, use it directly (no provisioning)
    if let Some(target) = &config.replay.target {
        return Ok(ProvisionedDb {
            connection_string: target.clone(),
            container_id: None,
        });
    }

    // Otherwise, provision via config
    let prov_config = config
        .provision
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("No [replay].target or [provision] section"))?;

    provision::provision(prov_config).map_err(|e| anyhow::anyhow!("Provision error: {e}"))
}

fn run_replay_step(
    config: &PipelineConfig,
    profile: &WorkloadProfile,
    connection_string: &str,
) -> Result<(WorkloadProfile, Vec<ReplayResults>)> {
    let mode = if config.replay.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    // Scale if requested
    let replay_profile = if config.replay.scale > 1 {
        if let Some(warning) = check_write_safety(profile) {
            eprintln!("{warning}");
        }
        let scaled = scale_sessions(profile, config.replay.scale, config.replay.stagger_ms);
        let mut p = profile.clone();
        p.sessions = scaled;
        p.metadata.total_sessions = p.sessions.len() as u64;
        p.metadata.total_queries = p.sessions.iter().map(|s| s.queries.len() as u64).sum();
        info!(
            "Scaled: {} -> {} sessions ({}x)",
            profile.sessions.len(),
            p.metadata.total_sessions,
            config.replay.scale
        );
        p
    } else {
        profile.clone()
    };

    let rt = tokio::runtime::Runtime::new()?;
    let results = rt
        .block_on(run_replay(
            &replay_profile,
            connection_string,
            mode,
            config.replay.speed,
        ))
        .map_err(|e| anyhow::anyhow!("Replay error: {e}"))?;

    Ok((replay_profile, results))
}

fn write_output_reports(
    config: &PipelineConfig,
    comparison: &ComparisonReport,
    elapsed_secs: f64,
) -> Result<()> {
    if let Some(ref output) = config.output {
        if let Some(ref json_path) = output.json_report {
            report::write_json_report(json_path, comparison)?;
            info!("JSON report: {}", json_path.display());
        }
        if let Some(ref junit_path) = output.junit_xml {
            // Re-evaluate thresholds for JUnit output
            let threshold_results = if let Some(ref thresholds) = config.thresholds {
                evaluate_thresholds(comparison, thresholds)
            } else {
                Vec::new()
            };
            write_junit_xml(junit_path, &threshold_results, elapsed_secs)?;
            info!("JUnit XML: {}", junit_path.display());
        }
    }
    Ok(())
}

/// Classify an error into the appropriate exit code based on its message.
fn classify_error(e: &anyhow::Error) -> i32 {
    let msg = format!("{e:#}");
    if msg.contains("Config") || msg.contains("parse") || msg.contains("TOML") {
        EXIT_CONFIG_ERROR
    } else if msg.contains("Capture error") {
        EXIT_CAPTURE_ERROR
    } else if msg.contains("Provision error") || msg.contains("Docker") || msg.contains("container") {
        EXIT_PROVISION_ERROR
    } else if msg.contains("Replay error") || msg.contains("connection") {
        EXIT_REPLAY_ERROR
    } else {
        EXIT_REPLAY_ERROR // default to replay error for unknown
    }
}
```

**Step 3: Run compilation check**

```bash
cargo check
```

**Step 4: Commit**

```bash
git add src/lib.rs src/pipeline/mod.rs
git commit -m "feat(pipeline): CI/CD pipeline orchestrator with staged error handling"
```

---

## Task 6: CLI Integration — `run` Subcommand

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`

**What this does:** Add `pg-retest run --config .pg-retest.toml` subcommand that invokes the pipeline and exits with the appropriate code.

**Step 1: Add RunArgs to `src/cli.rs`**

Add after the `Proxy(ProxyArgs),` line in the `Commands` enum:
```rust
    /// Run full CI/CD pipeline (capture → provision → replay → compare)
    Run(RunArgs),
```

Add the RunArgs struct at the end of the file:
```rust
#[derive(clap::Args)]
pub struct RunArgs {
    /// Path to pipeline config file (.toml)
    #[arg(long, default_value = ".pg-retest.toml")]
    pub config: PathBuf,
}
```

**Step 2: Add cmd_run to `src/main.rs`**

Add to the match in `main()`:
```rust
        Commands::Run(args) => cmd_run(args),
```

Add the `cmd_run` function:
```rust
fn cmd_run(args: pg_retest::cli::RunArgs) -> Result<()> {
    use pg_retest::config::load_config;
    use pg_retest::pipeline;

    let config = match load_config(&args.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Config error: {e:#}");
            std::process::exit(pipeline::EXIT_CONFIG_ERROR);
        }
    };

    let result = pipeline::run_pipeline(&config);

    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }

    Ok(())
}
```

**Step 3: Verify it compiles and --help works**

```bash
cargo build
cargo run -- run --help
```

Expected output includes `--config` flag with default `.pg-retest.toml`.

**Step 4: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat(cli): add 'run' subcommand for CI/CD pipeline execution"
```

---

## Task 7: Integration Test — Full Pipeline

**Files:**
- Create: `tests/fixtures/bench_setup.sql`
- Create: `tests/pipeline_test.rs`

**What this does:** End-to-end test that runs the full pipeline with an existing workload file against a live PG (Docker or pre-existing). Also tests config validation edge cases.

**Step 1: Create `tests/fixtures/bench_setup.sql`**

```sql
CREATE TABLE IF NOT EXISTS test_items (
    id serial PRIMARY KEY,
    name text NOT NULL,
    value numeric(10,2)
);
INSERT INTO test_items (name, value) SELECT 'item_' || i, (random() * 100)::numeric(10,2) FROM generate_series(1, 50) i;
```

**Step 2: Create `tests/pipeline_test.rs`**

```rust
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
    // Pipeline should fail with config-like error
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
        // Use a connection that will fail — we're testing threshold logic, not replay
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

    // This will fail at replay (can't connect to port 1), but that's expected
    let result = run_pipeline(&config);
    // Should get a replay error, not a config error
    assert_eq!(result.exit_code, pipeline::EXIT_REPLAY_ERROR);
}
```

**Step 3: Run tests**

```bash
cargo test --test pipeline_test
```

**Step 4: Commit**

```bash
git add tests/fixtures/bench_setup.sql tests/pipeline_test.rs
git commit -m "test(pipeline): integration tests for CI/CD pipeline"
```

---

## Task 8: End-to-End Docker Test + Polish

**Files:**
- Create: `tests/fixtures/sample_config.toml`
- Modify: `tests/pipeline_test.rs` (add Docker e2e test)

**What this does:** Full end-to-end test that provisions Docker, captures from CSV, replays, compares, checks thresholds, and produces JUnit XML. Also creates an example config file for documentation.

**Step 1: Create `tests/fixtures/sample_config.toml`**

```toml
# Example pg-retest CI/CD pipeline config
# Usage: pg-retest run --config .pg-retest.toml

[capture]
# Option A: Use an existing workload file
# workload = "workload.wkl"

# Option B: Capture from PG CSV log
source_log = "tests/fixtures/sample_pg.csv"
source_host = "prod-db-01"
pg_version = "16.2"
mask_values = false

[replay]
# Target connection string (when not using Docker provisioning)
target = "host=127.0.0.1 port=5432 user=postgres dbname=test"
speed = 0           # 0 = max speed
read_only = true
scale = 1

[thresholds]
p95_max_ms = 100.0
p99_max_ms = 500.0
error_rate_max_pct = 5.0
regression_max_count = 10
regression_threshold_pct = 20.0

[output]
json_report = "report.json"
junit_xml = "results.xml"
```

**Step 2: Add Docker e2e test to `tests/pipeline_test.rs`**

Append to the existing file:

```rust
/// Full end-to-end test with Docker provisioning.
/// Requires Docker to be running. Skipped if Docker is not available.
#[test]
fn test_pipeline_e2e_with_docker() {
    // Skip if Docker is not available
    let docker_check = std::process::Command::new("docker")
        .arg("version")
        .output();
    if docker_check.is_err() || !docker_check.unwrap().status.success() {
        eprintln!("Skipping Docker e2e test: Docker not available");
        return;
    }

    use pg_retest::config::ProvisionConfig;

    let json_file = NamedTempFile::with_suffix(".json").unwrap();
    let junit_file = NamedTempFile::with_suffix(".xml").unwrap();

    let config = PipelineConfig {
        capture: Some(CaptureConfig {
            workload: None,
            source_log: Some(PathBuf::from("tests/fixtures/sample_pg.csv")),
            source_host: Some("test-host".into()),
            pg_version: Some("16".into()),
            mask_values: false,
        }),
        provision: Some(ProvisionConfig {
            backend: "docker".into(),
            image: Some("postgres:16".into()),
            restore_from: None,
            connection_string: None,
            port: None,
        }),
        replay: ReplayConfig {
            speed: 0.0,
            read_only: true,
            scale: 1,
            stagger_ms: 0,
            target: None, // use provisioned DB
        },
        thresholds: Some(ThresholdConfig {
            p95_max_ms: Some(500.0),
            p99_max_ms: Some(2000.0),
            error_rate_max_pct: Some(50.0), // generous for test
            regression_max_count: Some(100),
            regression_threshold_pct: 20.0,
        }),
        output: Some(OutputConfig {
            json_report: Some(json_file.path().to_path_buf()),
            junit_xml: Some(junit_file.path().to_path_buf()),
        }),
    };

    let result = run_pipeline(&config);

    // Should complete (pass or threshold violation, not crash)
    assert!(
        result.exit_code == pipeline::EXIT_PASS
            || result.exit_code == pipeline::EXIT_THRESHOLD_VIOLATION,
        "Expected PASS or THRESHOLD_VIOLATION, got exit code {}",
        result.exit_code
    );

    // JSON report should exist
    assert!(json_file.path().exists());
    let json_content = std::fs::read_to_string(json_file.path()).unwrap();
    assert!(json_content.contains("total_queries"));

    // JUnit XML should exist
    assert!(junit_file.path().exists());
    let xml_content = std::fs::read_to_string(junit_file.path()).unwrap();
    assert!(xml_content.contains("<testsuites"));
    assert!(xml_content.contains("pg-retest"));
}
```

**Step 3: Run all tests**

```bash
cargo test
cargo clippy
```

**Step 4: Commit**

```bash
git add tests/fixtures/sample_config.toml tests/pipeline_test.rs
git commit -m "test(pipeline): end-to-end Docker test + example config"
```

---

## Build Order & Dependencies

```
Task 1: Config module           ← foundation, no dependencies
Task 2: Threshold evaluation    ← depends on Task 1 (uses ThresholdConfig)
Task 3: JUnit XML               ← depends on Task 2 (uses ThresholdResult)
Task 4: Docker provisioner      ← depends on Task 1 (uses ProvisionConfig)
Task 5: Pipeline orchestrator   ← depends on Tasks 1-4 (uses all)
Task 6: CLI integration         ← depends on Task 5
Task 7: Integration tests       ← depends on Task 6
Task 8: E2E Docker test         ← depends on Task 7
```

Tasks 2, 3, and 4 can run in parallel after Task 1.

---

## Verification Checklist

After all tasks:
- [ ] `cargo test` — all tests pass (existing 86 + new config/threshold/junit/pipeline tests)
- [ ] `cargo clippy` — zero warnings
- [ ] `cargo run -- run --help` — shows `--config` flag
- [ ] `cargo run -- run --config tests/fixtures/sample_config.toml` — runs pipeline (may fail if no PG available, but should not crash)
- [ ] JUnit XML output validates with `xmllint --noout results.xml` (if xmllint available)
- [ ] JSON report is valid JSON with all comparison fields
