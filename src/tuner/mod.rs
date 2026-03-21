pub mod advisor;
pub mod apply;
pub mod context;
pub mod safety;
pub mod types;

use anyhow::Result;

use crate::compare;
use crate::replay::{self, ReplayMode};
use crate::transform::analyze;

use self::advisor::{create_advisor, AdvisorConfig};
use self::apply::{apply_all, rollback_all};
use self::context::{collect_context, connect};
use self::safety::{check_production_hostname, validate_recommendations};
use self::types::*;

/// Run the tuning loop. Returns a TuningReport.
///
/// If `config.apply` is false (dry-run), only the first iteration's
/// recommendations are generated and printed — nothing is applied.
pub async fn run_tuning(config: &TuningConfig) -> Result<TuningReport> {
    run_tuning_with_events(config, None).await
}

/// Run the tuning loop with an optional event channel for progress reporting.
pub async fn run_tuning_with_events(
    config: &TuningConfig,
    events_tx: Option<tokio::sync::mpsc::UnboundedSender<TuningEvent>>,
) -> Result<TuningReport> {
    let send_event = |event: TuningEvent| {
        if let Some(ref tx) = events_tx {
            let _ = tx.send(event);
        }
    };
    // 1. Safety: check production hostname
    check_production_hostname(&config.target, config.force)?;

    // 2. Load workload profile
    let profile = crate::profile::io::read_profile(&config.workload_path)?;

    // 3. Analyze workload
    let workload_analysis = analyze::analyze_workload(&profile);

    // 4. Resolve API key
    let api_key = config
        .api_key
        .clone()
        .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
        .or_else(|| std::env::var("OPENAI_API_KEY").ok())
        .or_else(|| std::env::var("GEMINI_API_KEY").ok())
        .unwrap_or_default();

    let provider: crate::transform::planner::LlmProvider = config.provider.parse()?;

    // 5. Create advisor
    let advisor = create_advisor(AdvisorConfig {
        provider,
        api_key,
        api_url: config.api_url.clone(),
        model: config.model.clone(),
    });

    // 6. Connect to target
    let client = connect(&config.target, config.tls.clone()).await?;

    // 7. Collect baseline: replay once to get baseline metrics
    let replay_mode = if config.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    println!("  Collecting baseline replay...");
    send_event(TuningEvent::BaselineStarted);
    let baseline_results = replay::session::run_replay(
        &profile,
        &config.target,
        replay_mode,
        config.speed,
        None,
        config.tls.clone(),
    )
    .await?;

    let baseline_report = compare::compute_comparison(&profile, &baseline_results, 20.0);

    let mut iterations: Vec<TuningIteration> = Vec::new();
    let mut all_changes: Vec<AppliedChange> = Vec::new();

    for i in 1..=config.max_iterations {
        println!(
            "\n  === Tuning Iteration {}/{} ===",
            i, config.max_iterations
        );
        send_event(TuningEvent::IterationStarted {
            iteration: i,
            max_iterations: config.max_iterations,
        });

        // Collect context (re-collect each iteration to see impact of changes)
        println!("  Collecting PG context...");
        let context = collect_context(&client, &profile, 10).await?;

        // Call LLM
        println!("  Requesting recommendations from {}...", advisor.name());
        let recommendations = advisor
            .recommend(
                &context,
                &workload_analysis,
                config.hint.as_deref(),
                &iterations,
            )
            .await?;

        if recommendations.is_empty() {
            println!("  No recommendations from LLM. Stopping.");
            break;
        }

        println!("  Received {} recommendations:", recommendations.len());
        send_event(TuningEvent::RecommendationsReceived {
            iteration: i,
            recommendations: recommendations.clone(),
        });
        for (j, rec) in recommendations.iter().enumerate() {
            print_recommendation(j + 1, rec);
        }

        // Validate safety
        let (safe_recs, rejected) = validate_recommendations(&recommendations);
        if !rejected.is_empty() {
            println!("\n  Rejected {} recommendations:", rejected.len());
            for (rec, reason) in &rejected {
                println!("    - {}: {}", rec_summary(rec), reason);
            }
        }

        if safe_recs.is_empty() {
            println!("  All recommendations rejected by safety layer. Stopping.");
            iterations.push(TuningIteration {
                iteration: i,
                recommendations,
                applied: vec![],
                comparison: None,
                llm_feedback: "All recommendations rejected by safety layer.".into(),
            });
            break;
        }

        // Dry-run: stop after printing
        if !config.apply {
            println!("\n  Dry-run mode — not applying changes. Use --apply to execute.");
            iterations.push(TuningIteration {
                iteration: i,
                recommendations,
                applied: vec![],
                comparison: None,
                llm_feedback: "Dry-run — not applied.".into(),
            });
            break;
        }

        // Apply recommendations
        println!("\n  Applying {} recommendations...", safe_recs.len());
        let applied = apply_all(&client, &safe_recs).await;

        let successes = applied.iter().filter(|a| a.success).count();
        let failures = applied.iter().filter(|a| !a.success).count();
        println!("  Applied: {} success, {} failed", successes, failures);

        for a in &applied {
            send_event(TuningEvent::ChangeApplied {
                iteration: i,
                change: a.clone(),
            });
            if !a.success {
                println!(
                    "    FAILED: {} — {}",
                    rec_summary(&a.recommendation),
                    a.error.as_deref().unwrap_or("unknown")
                );
            }
        }

        all_changes.extend(applied.clone());

        if successes == 0 {
            println!("  No changes applied successfully. Stopping.");
            iterations.push(TuningIteration {
                iteration: i,
                recommendations,
                applied,
                comparison: None,
                llm_feedback: "No changes applied successfully.".into(),
            });
            break;
        }

        // Replay after changes
        println!("  Replaying workload...");
        let replay_results = replay::session::run_replay(
            &profile,
            &config.target,
            replay_mode,
            config.speed,
            None,
            config.tls.clone(),
        )
        .await?;

        let iter_report = compare::compute_comparison(&profile, &replay_results, 20.0);

        // Compare vs baseline
        let comparison = ComparisonSummary {
            p50_change_pct: pct_change(
                baseline_report.source_p50_latency_us,
                iter_report.replay_p50_latency_us,
            ),
            p95_change_pct: pct_change(
                baseline_report.source_p95_latency_us,
                iter_report.replay_p95_latency_us,
            ),
            p99_change_pct: pct_change(
                baseline_report.source_p99_latency_us,
                iter_report.replay_p99_latency_us,
            ),
            regressions: iter_report.regressions.len(),
            improvements: 0,
            errors_delta: iter_report.total_errors as i64 - baseline_report.total_errors as i64,
        };

        println!(
            "  Results: p50={:+.1}%, p95={:+.1}%, p99={:+.1}%",
            comparison.p50_change_pct, comparison.p95_change_pct, comparison.p99_change_pct
        );
        send_event(TuningEvent::ReplayCompleted {
            iteration: i,
            comparison: comparison.clone(),
        });

        // Build feedback for next iteration
        let feedback = format!(
            "p50: {:+.1}%, p95: {:+.1}%, p99: {:+.1}%, {} regressions, {} errors delta.",
            comparison.p50_change_pct,
            comparison.p95_change_pct,
            comparison.p99_change_pct,
            comparison.regressions,
            comparison.errors_delta,
        );

        // Check for regression — rollback and stop if p95 got worse
        let should_stop = comparison.p95_change_pct > 5.0;

        if should_stop {
            println!(
                "  p95 latency regressed by {:.1}%. Rolling back changes...",
                comparison.p95_change_pct
            );
            send_event(TuningEvent::RollbackStarted { iteration: i });

            let rollback_results = rollback_all(&client, &applied).await;
            let rolled_back = rollback_results.iter().filter(|r| r.success).count() as u32;
            let failed = rollback_results.iter().filter(|r| !r.success).count() as u32;

            for r in &rollback_results {
                if r.success {
                    println!("    Rolled back: {}", r.summary);
                } else {
                    println!(
                        "    Rollback FAILED: {} — {}",
                        r.summary,
                        r.error.as_deref().unwrap_or("unknown")
                    );
                }
            }
            println!("  Rollback: {} succeeded, {} failed", rolled_back, failed);
            send_event(TuningEvent::RollbackCompleted {
                iteration: i,
                rolled_back,
                failed,
            });

            // Reload config after rollback
            let _ = client.batch_execute("SELECT pg_reload_conf()").await;

            iterations.push(TuningIteration {
                iteration: i,
                recommendations,
                applied,
                comparison: Some(comparison),
                llm_feedback: format!("{} — ROLLED BACK due to regression.", feedback),
            });
            break;
        }

        iterations.push(TuningIteration {
            iteration: i,
            recommendations,
            applied,
            comparison: Some(comparison),
            llm_feedback: feedback,
        });
    }

    // Calculate total improvement
    let total_improvement_pct = iterations
        .last()
        .and_then(|i| i.comparison.as_ref())
        .map(|c| -c.p95_change_pct) // negative change = improvement
        .unwrap_or(0.0);

    let report = TuningReport {
        workload: config.workload_path.display().to_string(),
        target: config.target.clone(),
        provider: config.provider.clone(),
        hint: config.hint.clone(),
        iterations,
        total_improvement_pct,
        all_changes,
    };

    print_tuning_summary(&report);

    Ok(report)
}

fn pct_change(baseline: u64, current: u64) -> f64 {
    if baseline == 0 {
        return 0.0;
    }
    ((current as f64 - baseline as f64) / baseline as f64) * 100.0
}

fn rec_summary(rec: &Recommendation) -> String {
    match rec {
        Recommendation::ConfigChange {
            parameter,
            recommended_value,
            ..
        } => {
            format!("config: {} = {}", parameter, recommended_value)
        }
        Recommendation::CreateIndex { sql, .. } => {
            let preview: String = sql.chars().take(60).collect();
            format!("index: {}", preview)
        }
        Recommendation::QueryRewrite { original_sql, .. } => {
            let preview: String = original_sql.chars().take(60).collect();
            format!("rewrite: {}", preview)
        }
        Recommendation::SchemaChange { description, .. } => {
            format!("schema: {}", description)
        }
    }
}

fn print_recommendation(num: usize, rec: &Recommendation) {
    match rec {
        Recommendation::ConfigChange {
            parameter,
            current_value,
            recommended_value,
            rationale,
        } => {
            println!(
                "    {}. [CONFIG] {} = {} -> {}",
                num, parameter, current_value, recommended_value
            );
            println!("       Rationale: {}", rationale);
        }
        Recommendation::CreateIndex { sql, rationale, .. } => {
            println!("    {}. [INDEX] {}", num, sql);
            println!("       Rationale: {}", rationale);
        }
        Recommendation::QueryRewrite {
            original_sql,
            rewritten_sql,
            rationale,
        } => {
            let orig_preview: String = original_sql.chars().take(80).collect();
            let new_preview: String = rewritten_sql.chars().take(80).collect();
            println!("    {}. [REWRITE] {} -> {}", num, orig_preview, new_preview);
            println!("       Rationale: {}", rationale);
        }
        Recommendation::SchemaChange {
            sql,
            description,
            rationale,
        } => {
            println!("    {}. [SCHEMA] {} — {}", num, description, sql);
            println!("       Rationale: {}", rationale);
        }
    }
}

fn print_tuning_summary(report: &TuningReport) {
    println!("\n  Tuning Summary");
    println!("  ==============");
    println!("  Workload:       {}", report.workload);
    println!("  Target:         {}", report.target);
    println!("  Provider:       {}", report.provider);
    if let Some(ref hint) = report.hint {
        println!("  Hint:           {}", hint);
    }
    println!("  Iterations:     {}", report.iterations.len());
    println!(
        "  Changes applied: {}",
        report.all_changes.iter().filter(|c| c.success).count()
    );
    println!("  Total improvement: {:+.1}%", report.total_improvement_pct);
}
