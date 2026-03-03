# pg-retest Milestone 1 Design — Capture & Replay

**Date:** 2026-03-03
**Status:** Approved
**Scope:** Capture PG workload from CSV logs, replay against a target PG instance, produce comparison report.

## Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Language | Rust | High-performance concurrent replay, low capture overhead, strong type safety |
| Capture method (M1) | PG CSV log parsing | Broadest compatibility (cloud + on-prem), no app changes, minimal PG impact |
| Capture architecture | Pluggable backends via trait | Log parsing first, proxy + pg_stat_statements later |
| Profile format | MessagePack (`.wkl`) | ~60% smaller than JSON, ~3-5x faster serde, `inspect` command for debugging |
| CLI structure | Single binary + clap subcommands | `capture`, `replay`, `compare`, `inspect` |
| Replay concurrency | Tokio async, 1 connection per session | Scales to 1000s of sessions on few OS threads |
| Report output | Terminal table + JSON file | Human-readable + machine-parseable for future CI/CD |

## Architecture

```
pg-retest capture  →  [Capture Agent]  →  workload.wkl (MessagePack)
pg-retest replay   →  [Replay Engine]  →  results.wkl  (MessagePack)
pg-retest compare  →  [Reporter]       →  terminal table + report.json
pg-retest inspect  →  [Inspector]      →  pretty-printed JSON to stdout
```

### Capture Agent

PG CSV log parser for M1. Each backend implements:

```rust
trait CaptureSource {
    async fn capture(&self, config: &CaptureConfig) -> Result<WorkloadProfile>;
}
```

The CSV log parser:
- Reads PG CSV log files (`log_destination = 'csvlog'`)
- Extracts: session ID, user, database, query text, timestamp, duration
- Groups queries by session, computes relative timing offsets from session start
- Outputs `.wkl` file

### Workload Profile Format

```rust
WorkloadProfile {
    version: u8,
    captured_at: DateTime<Utc>,
    source_host: String,
    pg_version: String,
    capture_method: String,   // "csv_log" | "proxy" | "pg_stat"
    sessions: Vec<Session>,
    metadata: Metadata,
}

Session {
    id: u64,
    user: String,
    database: String,
    queries: Vec<Query>,
}

Query {
    sql: String,
    start_offset_us: u64,    // microseconds from session start
    duration_us: u64,
    kind: QueryKind,          // Select, Insert, Update, Delete, DDL, Other
}

Metadata {
    total_queries: u64,
    total_sessions: u64,
    capture_duration_us: u64,
}
```

### Replay Engine

- Tokio async runtime, one task per captured session
- Each task: open `tokio-postgres` connection → replay queries in order
- Timing: `sleep_until(replay_start + query.start_offset)` preserves inter-query gaps
- Modes:
  - `--read-write` (default): all queries
  - `--read-only`: strip INSERT/UPDATE/DELETE/DDL
- Speed multiplier: `--speed 2.0` compresses timing gaps
- Records per-query results into a results `.wkl` file

### Reporter

- Reads source `.wkl` + replay results `.wkl`
- Metrics: total queries, avg/p50/p95/p99 latency, throughput (qps), error count
- Per-query regression detection (configurable threshold)
- Output: terminal table (`tabled` crate) + JSON file

## Dependencies

| Crate | Purpose |
|-------|---------|
| `clap` (derive) | CLI argument parsing |
| `tokio` | Async runtime |
| `tokio-postgres` | Async PG client |
| `rmp-serde` | MessagePack serde |
| `serde` / `serde_json` | Serialization + JSON export |
| `chrono` | Timestamps |
| `csv` | PG CSV log parsing |
| `tabled` | Terminal tables |
| `tracing` / `tracing-subscriber` | Structured logging |
| `anyhow` / `thiserror` | Error handling |

## Project Structure

```
pg-retest/
├── Cargo.toml
├── src/
│   ├── main.rs              # clap CLI, subcommand dispatch
│   ├── cli.rs               # CLI arg structs
│   ├── capture/
│   │   ├── mod.rs            # CaptureSource trait
│   │   └── csv_log.rs        # PG CSV log parser
│   ├── profile/
│   │   ├── mod.rs            # WorkloadProfile, Session, Query types
│   │   └── io.rs             # Read/write .wkl files
│   ├── replay/
│   │   ├── mod.rs            # Replay orchestrator
│   │   └── session.rs        # Per-session async replay
│   ├── compare/
│   │   ├── mod.rs            # Comparison logic
│   │   └── report.rs         # Terminal table + JSON output
│   └── inspect/
│       └── mod.rs            # Dump .wkl as JSON
└── tests/
    ├── capture_csv_test.rs
    ├── profile_io_test.rs
    └── replay_test.rs
```

## PostgreSQL Logging Setup

See the user-facing guide in the project README for how to configure PG logging for capture.

Key points:
- Most settings are reload-only (no restart): `log_statement`, `log_duration`, `log_destination`
- `logging_collector = on` requires restart if not already enabled
- The tool should check and warn if logging is misconfigured
