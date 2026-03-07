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
    pub variants: Option<Vec<VariantConfig>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct CaptureConfig {
    /// Path to existing .wkl file (skip capture, use this directly)
    pub workload: Option<PathBuf>,
    /// Path to PG CSV log file (run capture from this)
    pub source_log: Option<PathBuf>,
    pub source_host: Option<String>,
    pub pg_version: Option<String>,
    /// Source type: "pg-csv" (default), "mysql-slow", "rds"
    #[serde(default = "default_source_type")]
    pub source_type: String,
    #[serde(default)]
    pub mask_values: bool,
    /// RDS instance identifier (for source_type = "rds")
    pub rds_instance: Option<String>,
    /// AWS region for RDS instance
    #[serde(default = "default_rds_region")]
    pub rds_region: String,
    /// Specific RDS log file to download (omit to use latest)
    pub rds_log_file: Option<String>,
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
    #[serde(default)]
    pub scale_analytical: Option<u32>,
    #[serde(default)]
    pub scale_transactional: Option<u32>,
    #[serde(default)]
    pub scale_mixed: Option<u32>,
    #[serde(default)]
    pub scale_bulk: Option<u32>,
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

#[derive(Debug, Clone, Deserialize)]
pub struct VariantConfig {
    pub label: String,
    pub target: String,
}

fn default_source_type() -> String {
    "pg-csv".to_string()
}
fn default_rds_region() -> String {
    "us-east-1".to_string()
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
    let contents = std::fs::read_to_string(path)
        .with_context(|| format!("Failed to read {}", path.display()))?;
    let config: PipelineConfig =
        toml::from_str(&contents).with_context(|| format!("Failed to parse {}", path.display()))?;
    validate_config(&config)?;
    Ok(config)
}

/// Validate config: ensure we have either a workload file or a source_log to capture from,
/// and either a provision section or a target connection string.
fn validate_config(config: &PipelineConfig) -> Result<()> {
    // Must have a way to get a workload
    let has_workload = config.capture.as_ref().is_some_and(|c| {
        c.workload.is_some()
            || c.source_log.is_some()
            || (c.source_type == "rds" && c.rds_instance.is_some())
    });
    if !has_workload {
        anyhow::bail!(
            "Config must specify either [capture].workload, [capture].source_log, or [capture].rds_instance (with source_type = \"rds\")"
        );
    }

    // Must have a way to connect to target (unless using A/B variants)
    let has_variants = config.variants.as_ref().is_some_and(|v| v.len() >= 2);
    let has_target = has_variants
        || config.replay.target.is_some()
        || config
            .provision
            .as_ref()
            .is_some_and(|p| p.connection_string.is_some() || p.backend == "docker");
    if !has_target {
        anyhow::bail!(
            "Config must specify [replay].target, [[variants]], [provision].connection_string, or [provision].backend = \"docker\""
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
        assert_eq!(
            config.capture.as_ref().unwrap().source_host.as_deref(),
            Some("prod-db-01")
        );
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

    #[test]
    fn test_parse_mysql_config() {
        let toml = r#"
[capture]
source_log = "mysql_slow.log"
source_type = "mysql-slow"
source_host = "mysql-prod"

[replay]
target = "host=localhost dbname=test"
read_only = true
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.capture.as_ref().unwrap().source_type, "mysql-slow");
    }

    #[test]
    fn test_parse_per_category_scaling_config() {
        let toml = r#"
[capture]
workload = "test.wkl"

[replay]
target = "host=localhost dbname=test"
scale_analytical = 2
scale_transactional = 4
scale_mixed = 1
scale_bulk = 0
stagger_ms = 500
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.replay.scale_analytical, Some(2));
        assert_eq!(config.replay.scale_transactional, Some(4));
        assert_eq!(config.replay.scale_mixed, Some(1));
        assert_eq!(config.replay.scale_bulk, Some(0));
        assert_eq!(config.replay.stagger_ms, 500);
    }

    #[test]
    fn test_default_source_type_is_pg_csv() {
        let toml = r#"
[capture]
source_log = "pg_log.csv"

[replay]
target = "host=localhost"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.capture.as_ref().unwrap().source_type, "pg-csv");
    }

    #[test]
    fn test_parse_rds_config() {
        let toml = r#"
[capture]
source_type = "rds"
rds_instance = "mydb-instance"
rds_region = "us-west-2"

[replay]
target = "host=localhost dbname=test"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        let cap = config.capture.as_ref().unwrap();
        assert_eq!(cap.source_type, "rds");
        assert_eq!(cap.rds_instance.as_deref(), Some("mydb-instance"));
        assert_eq!(cap.rds_region, "us-west-2");
        // Validation should pass (rds_instance counts as workload source)
        assert!(validate_config(&config).is_ok());
    }

    #[test]
    fn test_rds_config_default_region() {
        let toml = r#"
[capture]
source_type = "rds"
rds_instance = "mydb-instance"

[replay]
target = "host=localhost dbname=test"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        let cap = config.capture.as_ref().unwrap();
        assert_eq!(cap.rds_region, "us-east-1");
    }

    #[test]
    fn test_parse_variant_config() {
        let toml = r#"
[capture]
workload = "test.wkl"

[[variants]]
label = "pg16-default"
target = "host=db1 dbname=app"

[[variants]]
label = "pg16-tuned"
target = "host=db2 dbname=app"

[replay]
speed = 1.0
read_only = true
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        let variants = config.variants.as_ref().unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].label, "pg16-default");
        assert_eq!(variants[1].target, "host=db2 dbname=app");
        // Variants satisfy the target requirement — validation should pass
        assert!(validate_config(&config).is_ok());
    }
}
