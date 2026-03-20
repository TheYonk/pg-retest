use std::path::PathBuf;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "pg-retest")]
#[command(version, about = "Capture, replay, and compare PostgreSQL workloads")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,

    /// Enable verbose logging
    #[arg(short, long, global = true)]
    pub verbose: bool,
}

#[derive(Subcommand)]
pub enum Commands {
    /// Capture workload from PostgreSQL logs
    Capture(CaptureArgs),

    /// Replay a captured workload against a target database
    Replay(ReplayArgs),

    /// Compare source workload with replay results
    Compare(CompareArgs),

    /// Inspect a workload profile file
    Inspect(InspectArgs),

    /// Run a capture proxy between clients and PostgreSQL
    Proxy(ProxyArgs),

    /// Run full CI/CD pipeline (capture → provision → replay → compare)
    Run(RunArgs),

    /// Compare replay performance across different database targets
    #[command(name = "ab")]
    AB(ABArgs),

    /// Launch the web dashboard
    Web(WebArgs),

    /// Transform a workload using AI-generated plans
    Transform(TransformArgs),

    /// Run AI-assisted database tuning
    Tune(TuneArgs),

    /// Control a running persistent proxy
    #[command(name = "proxy-ctl")]
    ProxyCtl(ProxyCtlArgs),
}

#[derive(clap::Args)]
pub struct CaptureArgs {
    /// Path to source log file (required for pg-csv, mysql-slow)
    #[arg(long)]
    pub source_log: Option<PathBuf>,

    /// Source log type: pg-csv (default), mysql-slow, rds
    #[arg(long, default_value = "pg-csv")]
    pub source_type: String,

    /// Output workload profile path (.wkl)
    #[arg(short, long, default_value = "workload.wkl")]
    pub output: PathBuf,

    /// Source host identifier (for metadata)
    #[arg(long, default_value = "unknown")]
    pub source_host: String,

    /// PostgreSQL version (for metadata)
    #[arg(long, default_value = "unknown")]
    pub pg_version: String,

    /// Mask string and numeric literals in captured SQL (PII protection)
    #[arg(long, default_value_t = false)]
    pub mask_values: bool,

    /// RDS instance identifier (for --source-type rds)
    #[arg(long)]
    pub rds_instance: Option<String>,

    /// AWS region for RDS instance
    #[arg(long, default_value = "us-east-1")]
    pub rds_region: String,

    /// Specific RDS log file to download (omit to use latest)
    #[arg(long)]
    pub rds_log_file: Option<String>,
}

#[derive(clap::Args)]
pub struct ReplayArgs {
    /// Path to workload profile (.wkl)
    #[arg(long)]
    pub workload: PathBuf,

    /// Target PostgreSQL connection string
    #[arg(long)]
    pub target: String,

    /// Output results profile path (.wkl)
    #[arg(short, long, default_value = "results.wkl")]
    pub output: PathBuf,

    /// Replay only SELECT queries (strip DML)
    #[arg(long, default_value_t = false)]
    pub read_only: bool,

    /// Speed multiplier (e.g., 2.0 = 2x faster)
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,

    /// Scale factor: duplicate sessions N times for load testing
    #[arg(long, default_value_t = 1)]
    pub scale: u32,

    /// Stagger interval between scaled copies (milliseconds)
    #[arg(long, default_value_t = 0)]
    pub stagger_ms: u64,

    /// Scale analytical sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_analytical: Option<u32>,

    /// Scale transactional sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_transactional: Option<u32>,

    /// Scale mixed sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_mixed: Option<u32>,

    /// Scale bulk sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_bulk: Option<u32>,
}

#[derive(clap::Args)]
pub struct CompareArgs {
    /// Source workload profile (.wkl)
    #[arg(long)]
    pub source: PathBuf,

    /// Replay results profile (.wkl)
    #[arg(long)]
    pub replay: PathBuf,

    /// Output JSON report path
    #[arg(long)]
    pub json: Option<PathBuf>,

    /// Regression threshold percentage (flag queries slower by this %)
    #[arg(long, default_value_t = 20.0)]
    pub threshold: f64,

    /// Exit non-zero if regressions are detected
    #[arg(long, default_value_t = false)]
    pub fail_on_regression: bool,

    /// Exit non-zero if query errors occurred
    #[arg(long, default_value_t = false)]
    pub fail_on_error: bool,
}

#[derive(clap::Args)]
pub struct InspectArgs {
    /// Path to workload profile (.wkl)
    pub path: PathBuf,

    /// Show workload classification breakdown
    #[arg(long, default_value_t = false)]
    pub classify: bool,
}

#[derive(clap::Args)]
pub struct ProxyArgs {
    /// Address to listen on (e.g., 0.0.0.0:5433)
    #[arg(long, default_value = "0.0.0.0:5433")]
    pub listen: String,

    /// Target PostgreSQL address (e.g., localhost:5432)
    #[arg(long)]
    pub target: String,

    /// Output workload profile path (.wkl). Not required in persistent mode.
    #[arg(short, long)]
    pub output: Option<PathBuf>,

    /// Maximum server connections in the pool
    #[arg(long, default_value_t = 100)]
    pub pool_size: usize,

    /// Timeout waiting for a pool connection (seconds)
    #[arg(long, default_value_t = 30)]
    pub pool_timeout: u64,

    /// Mask string and numeric literals in captured SQL (PII protection)
    #[arg(long, default_value_t = false)]
    pub mask_values: bool,

    /// Disable workload capture (proxy-only mode)
    #[arg(long, default_value_t = false)]
    pub no_capture: bool,

    /// Capture duration (e.g., 60s, 5m). If not set, runs until Ctrl+C.
    #[arg(long)]
    pub duration: Option<String>,

    /// Run proxy in persistent mode (stays running, capture toggled separately)
    #[arg(long, default_value_t = false)]
    pub persistent: bool,

    /// Control port for persistent mode (default: 9091)
    #[arg(long, default_value_t = 9091)]
    pub control_port: u16,
}

#[derive(clap::Args)]
pub struct RunArgs {
    /// Path to pipeline config file (.toml)
    #[arg(long, default_value = ".pg-retest.toml")]
    pub config: PathBuf,
}

#[derive(clap::Args)]
pub struct ABArgs {
    /// Path to workload profile (.wkl)
    #[arg(long)]
    pub workload: PathBuf,

    /// Variant definitions: "label=connection_string" (specify 2+ times)
    #[arg(long = "variant", required = true, num_args = 2..)]
    pub variants: Vec<String>,

    /// Replay only SELECT queries
    #[arg(long, default_value_t = false)]
    pub read_only: bool,

    /// Speed multiplier
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,

    /// Output JSON report path
    #[arg(long)]
    pub json: Option<PathBuf>,

    /// Regression threshold percentage
    #[arg(long, default_value_t = 20.0)]
    pub threshold: f64,
}

#[derive(clap::Args)]
pub struct WebArgs {
    /// Port to listen on
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Data directory for SQLite database and workload files
    #[arg(long, default_value = "./data")]
    pub data_dir: std::path::PathBuf,
}

#[derive(clap::Args)]
pub struct TransformArgs {
    #[command(subcommand)]
    pub action: TransformAction,
}

#[derive(Subcommand)]
pub enum TransformAction {
    /// Analyze a workload and show query groups (no AI needed)
    Analyze {
        /// Path to source workload profile (.wkl)
        #[arg(long)]
        workload: PathBuf,

        /// Output analysis as JSON
        #[arg(long)]
        json: bool,
    },

    /// Generate a transform plan using AI
    Plan {
        /// Path to source workload profile (.wkl)
        #[arg(long)]
        workload: PathBuf,

        /// Natural language description of the scenario
        #[arg(long)]
        prompt: String,

        /// LLM provider: claude, openai, gemini, bedrock, ollama
        #[arg(long, default_value = "claude")]
        provider: String,

        /// API key (or set ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY env var)
        #[arg(long)]
        api_key: Option<String>,

        /// Override API URL
        #[arg(long)]
        api_url: Option<String>,

        /// Override model name
        #[arg(long)]
        model: Option<String>,

        /// Output plan file path
        #[arg(short, long, default_value = "transform-plan.toml")]
        output: PathBuf,

        /// Show what AI would see without calling API
        #[arg(long)]
        dry_run: bool,
    },

    /// Apply a transform plan to produce a new workload
    Apply {
        /// Path to source workload profile (.wkl)
        #[arg(long)]
        workload: PathBuf,

        /// Path to transform plan (.toml)
        #[arg(long)]
        plan: PathBuf,

        /// Output workload profile (.wkl)
        #[arg(short, long, default_value = "transformed.wkl")]
        output: PathBuf,

        /// RNG seed for reproducible injection
        #[arg(long)]
        seed: Option<u64>,
    },
}

#[derive(clap::Args)]
pub struct TuneArgs {
    /// Path to workload profile (.wkl)
    #[arg(long)]
    pub workload: PathBuf,

    /// Target PostgreSQL connection string
    #[arg(long)]
    pub target: String,

    /// LLM provider: claude, openai, gemini, bedrock, ollama
    #[arg(long, default_value = "claude")]
    pub provider: String,

    /// API key (or set ANTHROPIC_API_KEY / OPENAI_API_KEY / GEMINI_API_KEY env var)
    #[arg(long)]
    pub api_key: Option<String>,

    /// Override API URL
    #[arg(long)]
    pub api_url: Option<String>,

    /// Override model name
    #[arg(long)]
    pub model: Option<String>,

    /// Maximum tuning iterations
    #[arg(long, default_value_t = 3)]
    pub max_iterations: u32,

    /// Natural language hint for the LLM
    #[arg(long)]
    pub hint: Option<String>,

    /// Apply recommendations (default is dry-run)
    #[arg(long, default_value_t = false)]
    pub apply: bool,

    /// Allow targeting production-looking hostnames
    #[arg(long, default_value_t = false)]
    pub force: bool,

    /// Output JSON report path
    #[arg(long)]
    pub json: Option<PathBuf>,

    /// Replay speed multiplier
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,

    /// Replay only SELECT queries
    #[arg(long, default_value_t = false)]
    pub read_only: bool,
}

#[derive(clap::Args)]
pub struct ProxyCtlArgs {
    /// Address of the running proxy's control endpoint (host:port)
    #[arg(long, default_value = "localhost:9091")]
    pub proxy: String,

    #[command(subcommand)]
    pub action: ProxyCtlAction,
}

#[derive(Subcommand)]
pub enum ProxyCtlAction {
    /// Show proxy status
    Status,
    /// Start capturing queries
    StartCapture,
    /// Stop capturing and produce a .wkl file
    StopCapture {
        /// Output path for the workload file
        #[arg(short, long)]
        output: Option<PathBuf>,
    },
    /// Recover orphaned capture data from a crash
    Recover,
    /// Discard orphaned capture data
    Discard,
}
