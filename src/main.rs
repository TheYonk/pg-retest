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
    }
}

fn cmd_capture(args: pg_retest::cli::CaptureArgs) -> Result<()> {
    use pg_retest::capture::csv_log::CsvLogCapture;
    use pg_retest::profile::io;

    let capture = CsvLogCapture;
    let profile = capture.capture_from_file(&args.source_log, &args.source_host, &args.pg_version)?;

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
    use pg_retest::replay::{ReplayMode, session::run_replay};

    let profile = io::read_profile(&args.workload)?;
    let mode = if args.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    println!(
        "Replaying {} sessions ({} queries) against {}",
        profile.metadata.total_sessions,
        profile.metadata.total_queries,
        args.target
    );
    println!("Mode: {:?}, Speed: {}x", mode, args.speed);

    let rt = tokio::runtime::Runtime::new()?;
    let results = rt.block_on(run_replay(&profile, &args.target, mode, args.speed))?;

    let total_replayed: usize = results.iter().map(|r| r.query_results.len()).sum();
    let total_errors: usize = results
        .iter()
        .flat_map(|r| &r.query_results)
        .filter(|q| !q.success)
        .count();

    println!("Replay complete: {total_replayed} queries replayed, {total_errors} errors");

    // Save results as MessagePack
    let bytes = rmp_serde::to_vec(&results)?;
    std::fs::write(&args.output, bytes)?;
    println!("Results written to {}", args.output.display());

    Ok(())
}

fn cmd_compare(_args: pg_retest::cli::CompareArgs) -> Result<()> {
    anyhow::bail!("Compare not yet implemented")
}

fn cmd_inspect(args: pg_retest::cli::InspectArgs) -> Result<()> {
    use pg_retest::profile::io;

    let profile = io::read_profile(&args.path)?;
    let json = serde_json::to_string_pretty(&profile)?;
    println!("{json}");
    Ok(())
}
