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
    }
}

fn cmd_capture(args: pg_retest::cli::CaptureArgs) -> Result<()> {
    use pg_retest::capture::csv_log::CsvLogCapture;
    use pg_retest::capture::masking::mask_sql_literals;
    use pg_retest::profile::io;

    let capture = CsvLogCapture;
    let mut profile =
        capture.capture_from_file(&args.source_log, &args.source_host, &args.pg_version)?;

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

    // Scale sessions if requested
    let replay_profile = if args.scale > 1 {
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

    let rt = tokio::runtime::Runtime::new()?;
    let replay_start = std::time::Instant::now();
    let results = rt.block_on(run_replay(&replay_profile, &args.target, mode, args.speed))?;
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

    let config = ProxyConfig {
        listen_addr: args.listen,
        target_addr: args.target,
        output: args.output,
        pool_size: args.pool_size,
        pool_timeout_secs: args.pool_timeout,
        mask_values: args.mask_values,
        no_capture: args.no_capture,
        duration,
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_proxy(config))
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
