use anyhow::Result;
use clap::Parser;
use tracing_subscriber::EnvFilter;

use pg_retest::cli::{Cli, Commands};

fn main() -> Result<()> {
    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("debug")
    } else {
        EnvFilter::new("info")
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match cli.command {
        Commands::Capture(args) => cmd_capture(args),
        Commands::Replay(args) => cmd_replay(args),
        Commands::Compare(args) => cmd_compare(args),
        Commands::Inspect(args) => cmd_inspect(args),
        Commands::Proxy(args) => cmd_proxy(args),
        Commands::Run(args) => cmd_run(args),
        Commands::AB(args) => cmd_ab(args),
        Commands::Web(args) => cmd_web(args),
        Commands::Transform(args) => cmd_transform(args),
        Commands::Tune(args) => cmd_tune(args),
        Commands::ProxyCtl(args) => cmd_proxy_ctl(args),
    }
}

fn cmd_capture(args: pg_retest::cli::CaptureArgs) -> Result<()> {
    use pg_retest::capture::csv_log::CsvLogCapture;
    use pg_retest::capture::masking::mask_sql_literals;
    use pg_retest::capture::mysql_slow::MysqlSlowLogCapture;
    use pg_retest::profile::io;

    let mut profile = match args.source_type.as_str() {
        "pg-csv" => {
            let source_log = args
                .source_log
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--source-log is required for pg-csv capture"))?;
            let capture = CsvLogCapture;
            capture.capture_from_file(source_log, &args.source_host, &args.pg_version)?
        }
        "mysql-slow" => {
            let source_log = args.source_log.as_deref().ok_or_else(|| {
                anyhow::anyhow!("--source-log is required for mysql-slow capture")
            })?;
            let capture = MysqlSlowLogCapture;
            capture.capture_from_file(source_log, &args.source_host, true)?
        }
        "rds" => {
            use pg_retest::capture::rds::RdsCapture;
            let instance_id = args.rds_instance.as_deref().ok_or_else(|| {
                anyhow::anyhow!("--rds-instance is required for --source-type rds")
            })?;
            let capture = RdsCapture;
            capture.capture_from_instance(
                instance_id,
                &args.rds_region,
                args.rds_log_file.as_deref(),
                &args.source_host,
            )?
        }
        other => anyhow::bail!("Unknown source type: {other}. Supported: pg-csv, mysql-slow, rds"),
    };

    if args.mask_values {
        for session in &mut profile.sessions {
            for query in &mut session.queries {
                query.sql = mask_sql_literals(&query.sql);
            }
        }
        println!("Applied PII masking to SQL literals");
    }

    println!(
        "Captured {} queries across {} sessions",
        profile.metadata.total_queries, profile.metadata.total_sessions
    );

    io::write_profile(&args.output, &profile)?;
    println!("Wrote workload profile to {}", args.output.display());
    Ok(())
}

fn cmd_replay(args: pg_retest::cli::ReplayArgs) -> Result<()> {
    use pg_retest::profile::io;
    use pg_retest::replay::scaling::{check_write_safety, scale_sessions};
    use pg_retest::replay::{session::run_replay, ReplayMode};

    let profile = io::read_profile(&args.workload)?;
    let mode = if args.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    // Scale sessions if requested (per-category takes priority over uniform)
    let has_class_scaling = args.scale_analytical.is_some()
        || args.scale_transactional.is_some()
        || args.scale_mixed.is_some()
        || args.scale_bulk.is_some();

    let replay_profile = if has_class_scaling {
        use pg_retest::classify::WorkloadClass;
        use pg_retest::replay::scaling::scale_sessions_by_class;
        use std::collections::HashMap;

        let mut class_scales = HashMap::new();
        class_scales.insert(
            WorkloadClass::Analytical,
            args.scale_analytical.unwrap_or(1),
        );
        class_scales.insert(
            WorkloadClass::Transactional,
            args.scale_transactional.unwrap_or(1),
        );
        class_scales.insert(WorkloadClass::Mixed, args.scale_mixed.unwrap_or(1));
        class_scales.insert(WorkloadClass::Bulk, args.scale_bulk.unwrap_or(1));

        if let Some(warning) = check_write_safety(&profile) {
            println!("{warning}");
        }

        let scaled_sessions = scale_sessions_by_class(&profile, &class_scales, args.stagger_ms);

        println!("Per-category scaling:");
        for (class, scale) in &class_scales {
            println!("  {:?}: {}x", class, scale);
        }
        println!(
            "Scaled workload: {} original sessions -> {} total",
            profile.sessions.len(),
            scaled_sessions.len(),
        );

        let mut scaled = profile.clone();
        scaled.sessions = scaled_sessions;
        scaled.metadata.total_sessions = scaled.sessions.len() as u64;
        scaled.metadata.total_queries =
            scaled.sessions.iter().map(|s| s.queries.len() as u64).sum();
        scaled
    } else if args.scale > 1 {
        if let Some(warning) = check_write_safety(&profile) {
            println!("{warning}");
        }
        let scaled_sessions = scale_sessions(&profile, args.scale, args.stagger_ms);
        println!(
            "Scaled workload: {} original sessions -> {} total ({}x, {}ms stagger)",
            profile.sessions.len(),
            scaled_sessions.len(),
            args.scale,
            args.stagger_ms
        );
        let mut scaled = profile.clone();
        scaled.sessions = scaled_sessions;
        scaled.metadata.total_sessions = scaled.sessions.len() as u64;
        scaled.metadata.total_queries =
            scaled.sessions.iter().map(|s| s.queries.len() as u64).sum();
        scaled
    } else {
        profile.clone()
    };

    println!(
        "Replaying {} sessions ({} queries) against {}",
        replay_profile.metadata.total_sessions, replay_profile.metadata.total_queries, args.target
    );
    println!("Mode: {:?}, Speed: {}x", mode, args.speed);

    let tls_mode = pg_retest::tls::parse_tls_mode(&args.tls_mode)?;
    let tls = pg_retest::tls::make_tls_connector(tls_mode, args.tls_ca_cert.as_deref())?;

    let rt = tokio::runtime::Runtime::new()?;
    let replay_start = std::time::Instant::now();
    let results = rt.block_on(run_replay(
        &replay_profile,
        &args.target,
        mode,
        args.speed,
        args.max_connections,
        tls,
    ))?;
    let elapsed_us = replay_start.elapsed().as_micros() as u64;

    let total_replayed: usize = results.iter().map(|r| r.query_results.len()).sum();
    let total_errors: usize = results
        .iter()
        .flat_map(|r| &r.query_results)
        .filter(|q| !q.success)
        .count();

    println!("Replay complete: {total_replayed} queries replayed, {total_errors} errors");

    // Print scale report if scaled
    if args.scale > 1 {
        use pg_retest::compare::capacity::{compute_scale_report, print_scale_report};
        let scale_report = compute_scale_report(&results, args.scale, elapsed_us);
        print_scale_report(&scale_report);
    }

    // Save results as MessagePack
    let bytes = rmp_serde::to_vec(&results)?;
    std::fs::write(&args.output, bytes)?;
    println!("Results written to {}", args.output.display());

    Ok(())
}

fn cmd_compare(args: pg_retest::cli::CompareArgs) -> Result<()> {
    use pg_retest::compare::{compute_comparison, evaluate_outcome, report};
    use pg_retest::profile::io;
    use pg_retest::replay::ReplayResults;

    let source = io::read_profile(&args.source)?;

    let replay_bytes = std::fs::read(&args.replay)?;
    let results: Vec<ReplayResults> = rmp_serde::from_slice(&replay_bytes)?;

    let report_data = compute_comparison(&source, &results, args.threshold);
    report::print_terminal_report(&report_data);

    if let Some(json_path) = &args.json {
        report::write_json_report(json_path, &report_data)?;
        println!("  JSON report written to {}", json_path.display());
    }

    let outcome = evaluate_outcome(&report_data, args.fail_on_regression, args.fail_on_error);
    println!("  Result: {}", outcome.label());

    let code = outcome.exit_code();
    if code != 0 {
        std::process::exit(code);
    }

    Ok(())
}

fn cmd_inspect(args: pg_retest::cli::InspectArgs) -> Result<()> {
    use pg_retest::classify::{classify_workload, print_classification};
    use pg_retest::profile::io;

    let profile = io::read_profile(&args.path)?;
    let json = serde_json::to_string_pretty(&profile)?;
    println!("{json}");

    if args.classify {
        let classification = classify_workload(&profile);
        print_classification(&classification);
    }

    Ok(())
}

fn cmd_proxy(args: pg_retest::cli::ProxyArgs) -> Result<()> {
    use pg_retest::proxy::{run_proxy, ProxyConfig};

    let duration = args.duration.as_deref().map(parse_duration).transpose()?;

    // Default output to "workload.wkl" when not persistent and no output specified
    let output = match (&args.output, args.persistent) {
        (Some(p), _) => Some(p.clone()),
        (None, true) => None,
        (None, false) => Some("workload.wkl".into()),
    };

    let config = ProxyConfig {
        listen_addr: args.listen,
        target_addr: args.target,
        output,
        pool_size: args.pool_size,
        pool_timeout_secs: args.pool_timeout,
        mask_values: args.mask_values,
        no_capture: args.no_capture,
        duration,
        persistent: args.persistent,
        control_port: if args.persistent {
            Some(args.control_port)
        } else {
            None
        },
        max_capture_queries: args.max_capture_queries,
        max_capture_bytes: parse_size_string(&args.max_capture_size),
        max_capture_duration: args
            .max_capture_duration
            .as_deref()
            .and_then(|s| parse_duration(s).ok()),
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_proxy(config))
}

/// Parse a human-readable size string (e.g., "500MB", "1GB", "0") into bytes.
fn parse_size_string(s: &str) -> u64 {
    let s = s.trim().to_uppercase();
    if s == "0" || s.is_empty() {
        return 0;
    }
    let (num_str, multiplier) = if s.ends_with("GB") {
        (&s[..s.len() - 2], 1_073_741_824u64)
    } else if s.ends_with("MB") {
        (&s[..s.len() - 2], 1_048_576u64)
    } else if s.ends_with("KB") {
        (&s[..s.len() - 2], 1_024u64)
    } else {
        (s.as_str(), 1u64) // raw bytes
    };
    num_str
        .trim()
        .parse::<u64>()
        .unwrap_or(0)
        .saturating_mul(multiplier)
}

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

fn cmd_ab(args: pg_retest::cli::ABArgs) -> Result<()> {
    use pg_retest::compare::ab::{
        compute_ab_comparison, print_ab_report, write_ab_json, VariantResult,
    };
    use pg_retest::profile::io;
    use pg_retest::replay::{session::run_replay, ReplayMode};

    let profile = io::read_profile(&args.workload)?;
    let mode = if args.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    // Parse variant definitions: "label=connection_string"
    let parsed_variants: Vec<(String, String)> = args
        .variants
        .iter()
        .map(|v| {
            let parts: Vec<&str> = v.splitn(2, '=').collect();
            if parts.len() != 2 {
                anyhow::bail!("Invalid variant format: {v}. Expected: label=connection_string");
            }
            Ok((parts[0].to_string(), parts[1].to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    println!(
        "A/B test: {} variants, {} sessions, {} queries",
        parsed_variants.len(),
        profile.metadata.total_sessions,
        profile.metadata.total_queries,
    );

    let rt = tokio::runtime::Runtime::new()?;
    let mut variant_results = Vec::new();

    for (label, conn_string) in &parsed_variants {
        println!("Replaying variant '{label}' against {conn_string}...");
        let results = rt.block_on(run_replay(
            &profile,
            conn_string,
            mode,
            args.speed,
            None,
            None,
        ))?;
        variant_results.push(VariantResult::from_results(label.clone(), results));
    }

    let report = compute_ab_comparison(variant_results, args.threshold);
    print_ab_report(&report);

    if let Some(json_path) = &args.json {
        write_ab_json(json_path, &report)?;
        println!("  JSON report written to {}", json_path.display());
    }

    Ok(())
}

fn cmd_web(args: pg_retest::cli::WebArgs) -> Result<()> {
    use pg_retest::web;

    let auth_token = if args.no_auth {
        None
    } else {
        Some(
            args.auth_token
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
        )
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(web::run_server(
        args.port,
        args.data_dir,
        args.bind,
        auth_token,
    ))
}

fn cmd_transform(args: pg_retest::cli::TransformArgs) -> Result<()> {
    use pg_retest::cli::TransformAction;
    use pg_retest::profile::io;
    use pg_retest::transform::analyze::analyze_workload;

    match args.action {
        TransformAction::Analyze { workload, json } => {
            let profile = io::read_profile(&workload)?;
            let analysis = analyze_workload(&profile);

            if json {
                println!("{}", serde_json::to_string_pretty(&analysis)?);
            } else {
                println!("Workload Analysis");
                println!("=================");
                println!(
                    "  Queries: {}  Sessions: {}  Duration: {:.1}s",
                    analysis.profile_summary.total_queries,
                    analysis.profile_summary.total_sessions,
                    analysis.profile_summary.capture_duration_s,
                );
                println!();
                println!(
                    "Query Groups ({} identified, {} ungrouped):",
                    analysis.query_groups.len(),
                    analysis.ungrouped_queries
                );
                for group in &analysis.query_groups {
                    println!();
                    println!(
                        "  Group {}: {} queries ({:.1}%)",
                        group.id, group.query_count, group.pct_of_total
                    );
                    println!("    Tables: {}", group.tables.join(", "));
                    println!("    Sessions: {:?}", group.sessions);
                    println!("    Avg latency: {}us", group.avg_duration_us);
                    if !group.parameter_patterns.common_filters.is_empty() {
                        println!(
                            "    Filters: {}",
                            group.parameter_patterns.common_filters.join(", ")
                        );
                    }
                    println!("    Sample queries:");
                    for sq in &group.sample_queries {
                        println!("      - {sq}");
                    }
                }
            }
            Ok(())
        }

        TransformAction::Plan {
            workload,
            prompt,
            provider,
            api_key,
            api_url,
            model,
            output,
            dry_run,
        } => {
            let profile = io::read_profile(&workload)?;
            let analysis = analyze_workload(&profile);

            if dry_run {
                println!("Dry run — showing what AI would receive:");
                println!();
                println!("{}", serde_json::to_string_pretty(&analysis)?);
                return Ok(());
            }

            let api_key = api_key
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok())
                .or_else(|| std::env::var("OPENAI_API_KEY").ok())
                .or_else(|| std::env::var("GEMINI_API_KEY").ok())
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "API key required. Use --api-key or set ANTHROPIC_API_KEY/OPENAI_API_KEY/GEMINI_API_KEY env var"
                    )
                })?;

            use pg_retest::transform::planner::{create_planner, PlannerConfig};
            let provider = provider.parse().map_err(|e: anyhow::Error| e)?;
            let planner = create_planner(PlannerConfig {
                provider,
                api_key,
                api_url,
                model,
            });

            println!("Generating transform plan with {}...", planner.name());

            let rt = tokio::runtime::Runtime::new()?;
            let plan = rt.block_on(planner.generate_plan(&analysis, &prompt))?;

            let toml_str = toml::to_string_pretty(&plan)?;
            std::fs::write(&output, &toml_str)?;
            println!("Plan written to {}", output.display());
            println!();
            println!("  Groups: {}", plan.groups.len());
            println!("  Transforms: {}", plan.transforms.len());
            println!();
            println!("Review the plan, then apply:");
            println!(
                "  pg-retest transform apply --workload {} --plan {}",
                workload.display(),
                output.display()
            );
            Ok(())
        }

        TransformAction::Apply {
            workload,
            plan,
            output,
            seed,
        } => {
            let profile = io::read_profile(&workload)?;
            let plan_str = std::fs::read_to_string(&plan)?;
            let transform_plan: pg_retest::transform::plan::TransformPlan =
                toml::from_str(&plan_str)?;

            println!("Applying transform plan...");
            println!("  Groups: {}", transform_plan.groups.len());
            println!("  Transforms: {}", transform_plan.transforms.len());

            let result =
                pg_retest::transform::engine::apply_transform(&profile, &transform_plan, seed)?;

            println!(
                "  Result: {} sessions, {} queries (was: {} sessions, {} queries)",
                result.metadata.total_sessions,
                result.metadata.total_queries,
                profile.metadata.total_sessions,
                profile.metadata.total_queries
            );

            io::write_profile(&output, &result)?;
            println!("Wrote transformed workload to {}", output.display());
            Ok(())
        }
    }
}

fn cmd_tune(args: pg_retest::cli::TuneArgs) -> Result<()> {
    let tls_mode = pg_retest::tls::parse_tls_mode(&args.tls_mode)?;
    let tls = pg_retest::tls::make_tls_connector(tls_mode, args.tls_ca_cert.as_deref())?;

    // Validate API key for providers that need one
    let api_key = match args.provider.as_str() {
        "bedrock" | "ollama" => args.api_key, // No API key required
        "claude" => {
            let key = args
                .api_key
                .or_else(|| std::env::var("ANTHROPIC_API_KEY").ok());
            if key.is_none() {
                anyhow::bail!(
                    "Claude provider requires an API key.\n\
                     Set it with: export ANTHROPIC_API_KEY=sk-ant-...\n\
                     Or use: --api-key <key>"
                );
            }
            key
        }
        "openai" => {
            let key = args
                .api_key
                .or_else(|| std::env::var("OPENAI_API_KEY").ok());
            if key.is_none() {
                anyhow::bail!(
                    "OpenAI provider requires an API key.\n\
                     Set it with: export OPENAI_API_KEY=sk-...\n\
                     Or use: --api-key <key>"
                );
            }
            key
        }
        "gemini" => {
            let key = args
                .api_key
                .or_else(|| std::env::var("GEMINI_API_KEY").ok());
            if key.is_none() {
                anyhow::bail!(
                    "Gemini provider requires an API key.\n\
                     Set it with: export GEMINI_API_KEY=...\n\
                     Or use: --api-key <key>"
                );
            }
            key
        }
        other => {
            anyhow::bail!(
                "Unknown provider '{}'. Use: claude, openai, gemini, bedrock, ollama",
                other
            );
        }
    };

    let config = pg_retest::tuner::types::TuningConfig {
        workload_path: args.workload,
        target: args.target,
        provider: args.provider,
        api_key,
        api_url: args.api_url,
        model: args.model,
        max_iterations: args.max_iterations,
        hint: args.hint,
        apply: args.apply,
        force: args.force,
        speed: args.speed,
        read_only: args.read_only,
        tls,
    };

    let rt = tokio::runtime::Runtime::new()?;
    let report = rt.block_on(pg_retest::tuner::run_tuning(&config))?;

    if let Some(json_path) = args.json {
        let json = serde_json::to_string_pretty(&report)?;
        std::fs::write(&json_path, json)?;
        println!("\n  Report written to {}", json_path.display());
    }

    Ok(())
}

fn cmd_proxy_ctl(args: pg_retest::cli::ProxyCtlArgs) -> Result<()> {
    use pg_retest::cli::ProxyCtlAction;

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        let base_url = format!("http://{}", args.proxy);
        let client = reqwest::Client::new();

        // Auto-detect: try web API health first
        let is_web = client
            .get(format!("{}/api/v1/health", base_url))
            .timeout(std::time::Duration::from_secs(2))
            .send()
            .await
            .map(|r| r.status().is_success())
            .unwrap_or(false);

        let api_prefix = if is_web { "/api/v1/proxy" } else { "" };

        match args.action {
            ProxyCtlAction::Status => {
                let resp = client
                    .get(format!("{}{}/status", base_url, api_prefix))
                    .send()
                    .await?;
                let body: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
            ProxyCtlAction::StartCapture => {
                let resp = client
                    .post(format!("{}{}/start-capture", base_url, api_prefix))
                    .send()
                    .await?;
                let body: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
            ProxyCtlAction::StopCapture { output } => {
                let mut req = client.post(format!("{}{}/stop-capture", base_url, api_prefix));
                if let Some(path) = output {
                    req = req.json(&serde_json::json!({ "output": path.to_string_lossy() }));
                }
                let resp = req.send().await?;
                let body: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
            ProxyCtlAction::Recover => {
                let resp = client
                    .post(format!("{}{}/recover", base_url, api_prefix))
                    .send()
                    .await?;
                let body: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
            ProxyCtlAction::Discard => {
                let resp = client
                    .post(format!("{}{}/discard", base_url, api_prefix))
                    .send()
                    .await?;
                let body: serde_json::Value = resp.json().await?;
                println!("{}", serde_json::to_string_pretty(&body)?);
            }
        }

        Ok(())
    })
}

fn parse_duration(s: &str) -> Result<std::time::Duration> {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        Ok(std::time::Duration::from_secs(secs.parse()?))
    } else if let Some(mins) = s.strip_suffix('m') {
        Ok(std::time::Duration::from_secs(mins.parse::<u64>()? * 60))
    } else {
        Ok(std::time::Duration::from_secs(s.parse()?))
    }
}
