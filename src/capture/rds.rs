use std::path::Path;
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::capture::csv_log::CsvLogCapture;
use crate::profile::WorkloadProfile;

/// Metadata for an RDS log file, parsed from `aws rds describe-db-log-files`.
#[derive(Debug, Clone, Deserialize)]
pub struct RdsLogFile {
    #[serde(rename = "LogFileName")]
    pub log_file_name: String,
    #[serde(rename = "LastWritten")]
    pub last_written: u64,
    #[serde(rename = "Size")]
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct DescribeLogFilesResponse {
    #[serde(rename = "DescribeDBLogFiles")]
    describe_db_log_files: Vec<RdsLogFile>,
}

/// Parse the JSON output of `aws rds describe-db-log-files`.
pub fn parse_log_file_list(json: &str) -> Result<Vec<RdsLogFile>> {
    let resp: DescribeLogFilesResponse =
        serde_json::from_str(json).context("Failed to parse RDS log file list")?;
    Ok(resp.describe_db_log_files)
}

/// Select the most recent log file by `LastWritten` timestamp.
pub fn select_latest_log_file(files: &[RdsLogFile]) -> Option<String> {
    files
        .iter()
        .max_by_key(|f| f.last_written)
        .map(|f| f.log_file_name.clone())
}

pub struct RdsCapture;

impl RdsCapture {
    /// Capture a workload from an RDS/Aurora instance.
    ///
    /// 1. Validate `aws` CLI is available
    /// 2. List or select log file
    /// 3. Download log file (with pagination)
    /// 4. Parse as PG CSV log
    pub fn capture_from_instance(
        &self,
        instance_id: &str,
        region: &str,
        log_file: Option<&str>,
        source_host: &str,
    ) -> Result<WorkloadProfile> {
        // Step 1: Validate AWS CLI
        check_aws_cli()?;

        // Step 2: Determine which log file to download
        let log_file_name = match log_file {
            Some(name) => name.to_string(),
            None => {
                let files = list_log_files(instance_id, region)?;
                select_latest_log_file(&files).ok_or_else(|| {
                    anyhow::anyhow!(
                        "No log files found for RDS instance '{instance_id}'. \
                         Check that logging is enabled: log_destination = 'csvlog', \
                         log_statement = 'all'"
                    )
                })?
            }
        };

        tracing::info!("Downloading RDS log file: {log_file_name}");

        // Step 3: Download with pagination
        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let temp_path = temp_dir.path().join("rds_log.csv");
        download_log_file(instance_id, region, &log_file_name, &temp_path)?;

        // Step 4: Parse as PG CSV log
        let capture = CsvLogCapture;
        let mut profile = capture
            .capture_from_file(&temp_path, source_host, "unknown")
            .context(
                "Failed to parse RDS log. Ensure the instance uses log_destination = 'csvlog'",
            )?;

        profile.capture_method = "rds".to_string();
        Ok(profile)
    }
}

/// Check that the `aws` CLI is installed and accessible.
fn check_aws_cli() -> Result<()> {
    let output = Command::new("aws")
        .arg("--version")
        .output()
        .context(
            "AWS CLI not found. Install it: https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html",
        )?;

    if !output.status.success() {
        anyhow::bail!("AWS CLI returned an error. Ensure it is installed and configured.");
    }
    Ok(())
}

/// List available log files for an RDS instance.
fn list_log_files(instance_id: &str, region: &str) -> Result<Vec<RdsLogFile>> {
    let output = Command::new("aws")
        .args([
            "rds",
            "describe-db-log-files",
            "--db-instance-identifier",
            instance_id,
            "--region",
            region,
            "--output",
            "json",
        ])
        .output()
        .context("Failed to run aws rds describe-db-log-files")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("aws rds describe-db-log-files failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_log_file_list(&stdout)
}

/// Download an RDS log file with pagination support.
/// RDS returns max 1MB per call; we loop until `AdditionalDataPending` is false.
fn download_log_file(
    instance_id: &str,
    region: &str,
    log_file_name: &str,
    output_path: &Path,
) -> Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(output_path)
        .with_context(|| format!("Failed to create {}", output_path.display()))?;

    let mut marker = String::from("0");
    let mut total_bytes: usize = 0;

    loop {
        let output = run_download_portion(instance_id, region, log_file_name, &marker)?;

        let json: serde_json::Value =
            serde_json::from_slice(&output).context("Failed to parse download response")?;

        if let Some(data) = json.get("LogFileData").and_then(|v| v.as_str()) {
            file.write_all(data.as_bytes())?;
            total_bytes += data.len();
        }

        let pending = json
            .get("AdditionalDataPending")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !pending {
            break;
        }

        if let Some(m) = json.get("Marker").and_then(|v| v.as_str()) {
            marker = m.to_string();
        } else {
            break;
        }
    }

    tracing::info!("Downloaded {total_bytes} bytes from RDS log");
    Ok(())
}

/// Run a single `aws rds download-db-log-file-portion` call with retry.
fn run_download_portion(
    instance_id: &str,
    region: &str,
    log_file_name: &str,
    marker: &str,
) -> Result<Vec<u8>> {
    let args = [
        "rds",
        "download-db-log-file-portion",
        "--db-instance-identifier",
        instance_id,
        "--region",
        region,
        "--log-file-name",
        log_file_name,
        "--marker",
        marker,
        "--output",
        "json",
    ];

    let output = Command::new("aws")
        .args(args)
        .output()
        .context("Failed to run aws rds download-db-log-file-portion")?;

    if output.status.success() {
        return Ok(output.stdout);
    }

    // Retry once on failure
    let retry = Command::new("aws")
        .args(args)
        .output()
        .context("Retry failed for aws rds download-db-log-file-portion")?;

    if !retry.status.success() {
        let stderr = String::from_utf8_lossy(&retry.stderr);
        anyhow::bail!("aws rds download-db-log-file-portion failed after retry: {stderr}");
    }

    Ok(retry.stdout)
}
