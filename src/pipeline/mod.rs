use std::collections::HashMap;
use std::time::Instant;

use anyhow::Result;
use tracing::{error, info, warn};

use crate::capture::csv_log::CsvLogCapture;
use crate::capture::masking::mask_sql_literals;
use crate::classify::WorkloadClass;
use crate::compare::junit::write_junit_xml;
use crate::compare::report;
use crate::compare::threshold::{all_passed, evaluate_thresholds};
use crate::compare::{compute_comparison, ComparisonReport};
use crate::config::PipelineConfig;
use crate::profile::io;
use crate::profile::WorkloadProfile;
use crate::provision::{self, ProvisionedDb};
use crate::replay::scaling::{check_write_safety, scale_sessions, scale_sessions_by_class};
use crate::replay::session::run_replay;
use crate::replay::ReplayMode;

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
            error!("Pipeline error: {:#}", e);
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

    // ── Check for A/B variant mode ──────────────────────────────────
    if let Some(ref variants) = config.variants {
        if variants.len() >= 2 {
            return run_ab_pipeline(config, &profile, variants, pipeline_start);
        }
    }

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
    let comparison = compute_comparison(&replay_profile, &results, threshold_pct, None);
    report::print_terminal_report(&comparison);

    // ── Step 5: Evaluate thresholds ─────────────────────────────────
    let exit_code = if let Some(ref thresholds) = config.thresholds {
        let threshold_results = evaluate_thresholds(&comparison, thresholds);

        // Log threshold results
        for r in &threshold_results {
            let status = if r.passed { "PASS" } else { "FAIL" };
            info!(
                "[{status}] {}: {:.2} (limit: {:.2})",
                r.name, r.actual, r.limit
            );
        }

        if all_passed(&threshold_results) {
            info!("All thresholds passed");
            EXIT_PASS
        } else {
            warn!("Threshold violations detected");
            EXIT_THRESHOLD_VIOLATION
        }
    } else {
        info!("No thresholds configured, result: PASS");
        EXIT_PASS
    };

    // ── Step 6: Write output reports ────────────────────────────────
    let elapsed_secs = pipeline_start.elapsed().as_secs_f64();
    write_output_reports(config, &comparison, elapsed_secs)?;

    // ── Step 7: Teardown ────────────────────────────────────────────
    if let Err(e) = provision::teardown(&provisioned) {
        warn!("Teardown failed: {}", e);
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
        return io::read_profile(wkl_path).map_err(|e| anyhow::anyhow!("Capture error: {e}"));
    }

    // Otherwise, capture from log source
    info!("Capturing (type: {})", capture_cfg.source_type);
    let mut profile = match capture_cfg.source_type.as_str() {
        "pg-csv" => {
            let source_log = capture_cfg.source_log.as_ref().ok_or_else(|| {
                anyhow::anyhow!("Capture error: source_log required for source_type = \"pg-csv\"")
            })?;
            info!("Capturing from {}", source_log.display());
            let capture = CsvLogCapture;
            capture
                .capture_from_file(
                    source_log,
                    capture_cfg.source_host.as_deref().unwrap_or("unknown"),
                    capture_cfg.pg_version.as_deref().unwrap_or("unknown"),
                )
                .map_err(|e| anyhow::anyhow!("Capture error: {e}"))?
        }
        "mysql-slow" => {
            let source_log = capture_cfg.source_log.as_ref().ok_or_else(|| {
                anyhow::anyhow!(
                    "Capture error: source_log required for source_type = \"mysql-slow\""
                )
            })?;
            info!("Capturing from {}", source_log.display());
            use crate::capture::mysql_slow::MysqlSlowLogCapture;
            let capture = MysqlSlowLogCapture;
            capture
                .capture_from_file(
                    source_log,
                    capture_cfg.source_host.as_deref().unwrap_or("unknown"),
                    true, // always transform MySQL→PG
                )
                .map_err(|e| anyhow::anyhow!("Capture error: {e}"))?
        }
        "rds" => {
            use crate::capture::rds::RdsCapture;
            let instance_id = capture_cfg.rds_instance.as_deref().ok_or_else(|| {
                anyhow::anyhow!("Capture error: rds_instance required for source_type = \"rds\"")
            })?;
            info!("Capturing from RDS instance {}", instance_id);
            let capture = RdsCapture;
            capture
                .capture_from_instance(
                    instance_id,
                    &capture_cfg.rds_region,
                    capture_cfg.rds_log_file.as_deref(),
                    capture_cfg.source_host.as_deref().unwrap_or("rds"),
                )
                .map_err(|e| anyhow::anyhow!("Capture error: {e}"))?
        }
        other => anyhow::bail!("Capture error: unknown source_type: {other}"),
    };

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
) -> Result<(WorkloadProfile, Vec<crate::replay::ReplayResults>)> {
    let mode = if config.replay.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    // Scale if requested (per-category takes priority over uniform)
    let replay_profile = if let Some(class_scales) = build_class_scales(&config.replay) {
        if let Some(warning) = check_write_safety(profile) {
            warn!("{}", warning);
        }
        let scaled = scale_sessions_by_class(profile, &class_scales, config.replay.stagger_ms);
        let mut p = profile.clone();
        p.sessions = scaled;
        p.metadata.total_sessions = p.sessions.len() as u64;
        p.metadata.total_queries = p.sessions.iter().map(|s| s.queries.len() as u64).sum();
        info!(
            "Per-category scaled: {} -> {} sessions",
            profile.sessions.len(),
            p.metadata.total_sessions,
        );
        p
    } else if config.replay.scale > 1 {
        if let Some(warning) = check_write_safety(profile) {
            warn!("{}", warning);
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
            None,
            None,
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

/// Run the pipeline in A/B variant mode.
fn run_ab_pipeline(
    config: &PipelineConfig,
    profile: &WorkloadProfile,
    variants: &[crate::config::VariantConfig],
    pipeline_start: Instant,
) -> Result<PipelineResult> {
    use crate::compare::ab::{
        compute_ab_comparison, print_ab_report, write_ab_json, VariantResult,
    };

    let mode = if config.replay.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    let rt = tokio::runtime::Runtime::new()?;
    let mut variant_results = Vec::new();

    for variant in variants {
        info!(
            "A/B: replaying variant '{}' against {}",
            variant.label, variant.target
        );
        let results = rt
            .block_on(run_replay(
                profile,
                &variant.target,
                mode,
                config.replay.speed,
                None,
                None,
            ))
            .map_err(|e| anyhow::anyhow!("Replay error for '{}': {e}", variant.label))?;
        variant_results.push(VariantResult::from_results(variant.label.clone(), results));
    }

    let threshold_pct = config
        .thresholds
        .as_ref()
        .map_or(20.0, |t| t.regression_threshold_pct);
    let report = compute_ab_comparison(variant_results, threshold_pct);
    print_ab_report(&report);

    // Write output reports if configured
    if let Some(ref output) = config.output {
        if let Some(ref json_path) = output.json_report {
            write_ab_json(json_path, &report)?;
            info!("A/B JSON report: {}", json_path.display());
        }
    }

    let elapsed_secs = pipeline_start.elapsed().as_secs_f64();
    info!("A/B pipeline completed in {elapsed_secs:.1}s");

    Ok(PipelineResult {
        exit_code: EXIT_PASS,
        report: None, // A/B mode doesn't produce a standard ComparisonReport
    })
}

/// Build per-category scale factors from replay config.
/// Returns `None` if no per-category fields are set.
fn build_class_scales(replay: &crate::config::ReplayConfig) -> Option<HashMap<WorkloadClass, u32>> {
    let has_any = replay.scale_analytical.is_some()
        || replay.scale_transactional.is_some()
        || replay.scale_mixed.is_some()
        || replay.scale_bulk.is_some();

    if !has_any {
        return None;
    }

    let mut scales = HashMap::new();
    scales.insert(
        WorkloadClass::Analytical,
        replay.scale_analytical.unwrap_or(1),
    );
    scales.insert(
        WorkloadClass::Transactional,
        replay.scale_transactional.unwrap_or(1),
    );
    scales.insert(WorkloadClass::Mixed, replay.scale_mixed.unwrap_or(1));
    scales.insert(WorkloadClass::Bulk, replay.scale_bulk.unwrap_or(1));
    Some(scales)
}

/// Classify an error into the appropriate exit code based on its message.
fn classify_error(e: &anyhow::Error) -> i32 {
    let msg = format!("{e:#}");
    if msg.contains("Config") || msg.contains("parse") || msg.contains("TOML") {
        EXIT_CONFIG_ERROR
    } else if msg.contains("Capture error") {
        EXIT_CAPTURE_ERROR
    } else if msg.contains("Provision error") || msg.contains("Docker") || msg.contains("container")
    {
        EXIT_PROVISION_ERROR
    } else {
        // Covers explicit replay/connection errors and any unclassified errors
        EXIT_REPLAY_ERROR
    }
}
