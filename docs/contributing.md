# Contributing Guide

This document covers how to set up a development environment, run tests, and extend pg-retest with new capture backends, SQL transforms, and web endpoints.

## Development Setup

### Prerequisites

- **Rust toolchain**: Install via [rustup](https://rustup.rs/). The project uses the 2021 edition.
- **Docker**: Required for integration tests that start PostgreSQL containers.
- **PostgreSQL client libraries**: `tokio-postgres` links against system libpq for some features, though most tests use Docker.
- **AWS CLI** (optional): Only needed for RDS capture development (`--source-type rds`).

### Test Database

Integration tests expect a PostgreSQL instance on port 5441:

```
Host:     localhost
Port:     5441
User:     sales_demo_app
Password: salesdemo123
```

Start it with Docker:

```bash
docker run -d --name pg-retest-test \
  -p 5441:5432 \
  -e POSTGRES_USER=sales_demo_app \
  -e POSTGRES_PASSWORD=salesdemo123 \
  -e POSTGRES_DB=sales_demo \
  postgres:16
```

The `tests/fixtures/bench_setup.sql` file contains schema setup for tests that need tables.

## Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release
```

## Testing

### Run All Tests

```bash
cargo test
```

This runs both unit tests (inline `#[cfg(test)] mod tests` blocks in source files) and integration tests (files in the `tests/` directory).

### Run a Single Test File

```bash
cargo test --test profile_io_test
cargo test --test capture_csv_test
cargo test --test web_test
```

### Run a Single Test Function

```bash
cargo test --test profile_io_test test_profile_roundtrip_messagepack
```

### Run Unit Tests for a Specific Module

```bash
cargo test --lib capture::csv_log
cargo test --lib transform
cargo test --lib config
```

### Test Files

Integration tests live in `tests/` and import from the library crate:

```
tests/
  ab_test.rs                  — A/B variant comparison tests
  capture_csv_test.rs         — PG CSV log capture tests
  classify_test.rs            — Workload classification tests
  compare_test.rs             — Comparison report tests
  junit_test.rs               — JUnit XML output tests
  masking_test.rs             — PII masking tests
  mysql_integration_test.rs   — MySQL end-to-end tests
  mysql_slow_test.rs          — MySQL slow log parser tests
  mysql_transform_test.rs     — MySQL-to-PG transform tests
  per_category_scaling_test.rs — Per-category scaling tests
  pipeline_test.rs            — Pipeline orchestrator tests
  profile_io_test.rs          — Profile serialization roundtrip tests
  rds_capture_test.rs         — RDS capture tests
  replay_test.rs              — Replay engine tests (requires PG on 5441)
  scaling_test.rs             — Session scaling tests
  threshold_test.rs           — Threshold evaluation tests
  web_test.rs                 — Web dashboard handler tests
```

### Test Fixtures

Test fixture files live in `tests/fixtures/`:

```
tests/fixtures/
  bench_setup.sql         — Schema setup SQL for replay tests
  sample_config.toml      — Sample pipeline config for config parsing tests
  sample_mysql_slow.log   — Sample MySQL slow query log for parser tests
  sample_pg_txn.csv       — PG CSV log with transactions for capture tests
  sample_pg.csv           — Basic PG CSV log for capture tests
```

## Linting

```bash
# Check for common mistakes and style issues
cargo clippy

# The project maintains zero clippy warnings. Fix any warnings before submitting.
```

## Formatting

```bash
# Format all source files
cargo fmt

# Always run this after writing code. The formatter's output may differ from
# hand-written style, and CI will reject unformatted code.
```

## Verbose Logging

When debugging, enable detailed logging:

```bash
RUST_LOG=debug cargo run -- -v <subcommand>
```

## Adding a Capture Backend

To add support for capturing workloads from a new source (e.g., Oracle AWR, SQL Server trace), follow these steps:

### Step 1: Create the Source Module

Create `src/capture/new_source.rs`. Implement a struct that parses the source format:

```rust
use anyhow::Result;
use chrono::Utc;

use crate::profile::{
    assign_transaction_ids, Metadata, Query, QueryKind, Session, WorkloadProfile,
};

pub struct NewSourceCapture;

impl NewSourceCapture {
    pub fn capture_from_file(
        &self,
        path: &str,
        source_host: &str,
    ) -> Result<WorkloadProfile> {
        // 1. Parse the source file into per-session query lists
        let mut sessions: Vec<Session> = Vec::new();

        // ... parsing logic ...

        // 2. For each session, assign transaction IDs
        let mut next_txn_id: u64 = 1;
        for session in &mut sessions {
            assign_transaction_ids(&mut session.queries, &mut next_txn_id);
        }

        // 3. Compute metadata
        let total_queries: u64 = sessions.iter()
            .map(|s| s.queries.len() as u64)
            .sum();

        // 4. Build the profile
        Ok(WorkloadProfile {
            version: 2,
            captured_at: Utc::now(),
            source_host: source_host.to_string(),
            pg_version: "unknown".to_string(),
            capture_method: "new_source".to_string(),
            metadata: Metadata {
                total_queries,
                total_sessions: sessions.len() as u64,
                capture_duration_us: 0, // compute from timing data
            },
            sessions,
        })
    }
}
```

Key requirements for each `Query`:

- `sql`: The SQL text.
- `start_offset_us`: Microseconds from the first query in the session. Used by the replay engine for timing.
- `duration_us`: How long the query took on the source. Used for regression detection.
- `kind`: Use `QueryKind::from_sql(&sql)` to classify automatically.
- `transaction_id`: Populated by `assign_transaction_ids()`.

### Step 2: Register the Module

Add to `src/capture/mod.rs`:

```rust
pub mod new_source;
```

### Step 3: Add CLI Support

Add a new match arm in `cmd_capture()` in `src/main.rs`:

```rust
"new-source" => {
    use pg_retest::capture::new_source::NewSourceCapture;
    let source_log = args.source_log.as_deref()
        .ok_or_else(|| anyhow::anyhow!("--source-log required for new-source"))?;
    let capture = NewSourceCapture;
    capture.capture_from_file(source_log, &args.source_host)?
}
```

### Step 4: Add Pipeline Config Support

If the new source should be usable in pipeline configs, update the config validation in `src/config/mod.rs` to recognize the new `source_type` value.

### Step 5: Write Tests

Create `tests/new_source_test.rs` with at least:

- A test that parses a sample log file (add a fixture to `tests/fixtures/`).
- A test that verifies the produced profile has correct session/query counts.
- A test that verifies transaction IDs are assigned correctly.
- A test that verifies the `capture_method` field is set correctly.

## Adding a SQL Transform

To add a new SQL transform rule (e.g., for Oracle-to-PG conversion):

### Step 1: Implement the SqlTransformer Trait

Create `src/transform/oracle_to_pg.rs` (or add to an existing file):

```rust
use regex::Regex;

use super::{SqlTransformer, TransformResult};

pub struct NvlToCoalesce {
    re: Regex,
}

impl NvlToCoalesce {
    pub fn new() -> Self {
        Self {
            re: Regex::new(r"(?i)\bNVL\s*\(").unwrap(),
        }
    }
}

impl SqlTransformer for NvlToCoalesce {
    fn transform(&self, sql: &str) -> TransformResult {
        if self.re.is_match(sql) {
            let result = self.re.replace_all(sql, "COALESCE(").to_string();
            TransformResult::Transformed(result)
        } else {
            TransformResult::Unchanged
        }
    }

    fn name(&self) -> &str {
        "nvl_to_coalesce"
    }
}
```

### Step 2: Register the Module

Add to `src/transform/mod.rs`:

```rust
pub mod oracle_to_pg;
```

### Step 3: Use in a Pipeline

In your capture backend, build a `TransformPipeline` with the new transformer:

```rust
use crate::transform::{TransformPipeline, TransformResult};
use crate::transform::oracle_to_pg::NvlToCoalesce;

let pipeline = TransformPipeline::new(vec![
    Box::new(NvlToCoalesce::new()),
    // ... more transformers ...
]);

// For each captured query:
match pipeline.apply(&sql) {
    TransformResult::Transformed(new_sql) => { /* use new_sql */ }
    TransformResult::Skipped { reason } => { /* skip this query */ }
    TransformResult::Unchanged => { /* use original sql */ }
}
```

### Step 4: Write Tests

Write unit tests in the transform module itself (`#[cfg(test)] mod tests`) and integration tests in `tests/` that verify the transform pipeline end-to-end.

## Adding a Web Endpoint

### Step 1: Create the Handler

Add a handler function to an existing file in `src/web/handlers/` or create a new file. Handlers receive `AppState` via Axum's `State` extractor:

```rust
use axum::{extract::State, Json};
use serde::Serialize;

use crate::web::state::AppState;

#[derive(Serialize)]
pub struct MyResponse {
    pub status: String,
}

pub async fn my_handler(
    State(state): State<AppState>,
) -> Json<MyResponse> {
    // Access SQLite: let db = state.db.lock().await;
    // Access data dir: &state.data_dir
    // Broadcast WebSocket: state.broadcast(WsMessage::...);
    Json(MyResponse { status: "ok".to_string() })
}
```

If you create a new handler file, register it in `src/web/handlers/mod.rs`:

```rust
pub mod my_module;
```

### Step 2: Add the Route

In `src/web/routes.rs`, add the route to the `build_router` function:

```rust
.route("/my-endpoint", get(handlers::my_module::my_handler))
```

All API routes are nested under `/api/v1/`.

### Step 3: Add Database Functions (if needed)

If the endpoint needs to persist or query data, add functions to `src/web/db.rs`. Follow the existing pattern of accepting `&Connection` and using rusqlite queries.

### Step 4: Write Tests

Add tests to `tests/web_test.rs`. Web tests use in-memory SQLite (`:memory:`) and construct `AppState` directly, avoiding the need for a running HTTP server.

## Coding Conventions

### Module Organization

- All `pub mod` declarations go in `src/lib.rs`, never in `src/main.rs`.
- `src/main.rs` contains only CLI dispatch logic (Clap parsing and calling into library functions).
- New modules with submodules should use the `mod.rs` convention (e.g., `src/capture/mod.rs`).

### Test Organization

- **Integration tests** go in `tests/` as separate files. They import via `use pg_retest::...`.
- **Unit tests** go in `#[cfg(test)] mod tests` blocks at the bottom of the source file they test.
- Integration tests are preferred for anything that exercises multiple modules or tests end-to-end behavior.
- Unit tests are preferred for testing internal functions, edge cases, and parsing logic.

### Error Handling

- Use `anyhow::Result` for functions that can fail.
- Use `.context()` or `.with_context()` to add descriptive error messages.
- Avoid `.unwrap()` in non-test code.

### Serialization

- Use `serde` derives (`Serialize`, `Deserialize`) for all data types that cross boundaries (disk, network, API).
- MessagePack for workload profiles (`.wkl` files).
- JSON for API responses and reports.
- TOML for pipeline configuration.

### Async Code

- Use `tokio::spawn` for concurrent tasks.
- Use `CancellationToken` for graceful shutdown of background operations.
- Use `broadcast::channel` for fan-out event distribution (WebSocket).
- Use `mpsc::unbounded_channel` for producer-consumer patterns (capture events).

### Naming

- Capture backends: `XxxCapture` (e.g., `CsvLogCapture`, `MysqlSlowLogCapture`).
- Transform rules: Descriptive names (e.g., `BacktickToDoubleQuote`, `IfnullToCoalesce`).
- Web handlers: verb_noun pattern (e.g., `list_workloads`, `start_replay`, `compute_compare`).
- Test functions: `test_` prefix with descriptive name (e.g., `test_profile_roundtrip_messagepack`).

### Docker Test Database

Tests that need a live PostgreSQL connection use port 5441 with the credentials listed in the Development Setup section above. Tests that do not need a live database should not require one.

## Gotchas

This is a collected list of non-obvious behaviors that can trip up contributors.

### PG CSV Log Timestamps

PostgreSQL CSV log timestamps use the format `2024-03-08 10:00:00.100 UTC`, which is not RFC 3339. The parser in `capture::csv_log` has a fallback path that parses via `NaiveDateTime` and assumes UTC. If you add a new capture backend for PG logs, be aware of this.

### ROLLBACK TO SAVEPOINT

`ROLLBACK TO SAVEPOINT` must be classified as `QueryKind::Other`, not `QueryKind::Rollback`. Unlike a plain `ROLLBACK`, it does not end the enclosing transaction. The `QueryKind::from_sql()` function handles this by checking for `ROLLBACK TO` before checking for `ROLLBACK`.

### Profile v1/v2 Compatibility

The `transaction_id` field on `Query` was added in v2. It uses `#[serde(default)]`, which means v1 files (without the field) deserialize correctly -- `transaction_id` defaults to `None`. Do not remove this `#[serde(default)]` annotation.

### QueryKind::Begin, Commit, Rollback

These variants were added after the initial release. Older tests that asserted `BEGIN` maps to `QueryKind::Other` were updated to expect `QueryKind::Begin`. If you encounter a test failure where a transaction control statement maps to `Other`, check whether it needs updating.

### PII Masking Implementation

The masking in `capture::masking` uses a hand-written character-level state machine, not regex. This is intentional -- regex cannot correctly handle all SQL edge cases (escaped quotes, dollar-quoting, identifiers containing numbers). If you need to modify the masking logic, trace through the state machine carefully.

### MySQL-Specific Command Skipping

The MySQL slow log capture skips commands that have no PG equivalent: `SHOW`, `SET NAMES`, `USE`, etc. These are filtered by the transform pipeline returning `TransformResult::Skipped`. If you add new MySQL transform rules, consider which commands should be skipped vs. transformed.

### SQL Transform Limitations

The regex-based transforms have known edge cases:

- Backtick replacement can fire inside string literals (e.g., `SELECT 'foo\`bar'`).
- Only one LIMIT rewrite is applied per query (nested subqueries with LIMIT may not all be rewritten).
- These limitations are documented in `TransformReport` output.

### Per-Category vs. Uniform Scaling

Per-category scaling flags (`--scale-analytical`, `--scale-transactional`, etc.) and uniform scaling (`--scale N`) are mutually exclusive. If any per-category flag is set, uniform scaling is ignored. Unspecified categories default to 1x.

### A/B Variant Format

CLI: `--variant "label=connection_string"` (minimum 2 variants required).
Pipeline TOML: `[[variants]]` array with `label` and `target` fields.
When variants are present in a pipeline config, the pipeline bypasses normal provisioning and runs sequential replay against each variant target.

### RDS Capture

Requires the `aws` CLI to be installed and configured with appropriate credentials. Uses `--marker` for pagination (not `--starting-token`, which is for a different AWS CLI pattern). Files larger than 1MB are downloaded in chunks via `download-db-log-file-portion`.

### WorkloadClass Hash

`WorkloadClass` derives `Hash` because it is used as a `HashMap` key in per-category scaling (`scale_sessions_by_class`). If you add new variants to this enum, `Hash` will be derived automatically, but be aware of this usage.

### Web Dashboard Static Files

Frontend files in `src/web/static/` are embedded at compile time via `rust-embed`. Any changes to HTML, JS, or CSS files require recompilation (`cargo build`). There is no hot reload.

### Web Dashboard SQLite

SQLite stores metadata only. The `.wkl` files on disk in `data_dir/workloads/` are the source of truth. Deleting the SQLite database loses metadata (run history, threshold results) but not workload data.

### Proxy State

The proxy module uses `OnceLock<Arc<RwLock<ProxyState>>>` for module-level proxy state tracking. This is a static variable pattern -- be careful with initialization and locking order.

### SCRAM-SHA-256 Authentication

The proxy's auth relay has a subtle bug-prone area: after forwarding the SASLFinal (type 12) message from the server, the relay must NOT read from the client. The next message comes from the server (AuthenticationOk or ReadyForQuery), not the client. Getting this wrong causes a deadlock.

### Exit Codes

The pipeline uses staged exit codes: 0=pass, 1=threshold violation, 2=config error, 3=capture error, 4=provision error, 5=replay error. The `compare` subcommand uses different codes: 0=pass, 1=regressions, 2=errors. These are distinct systems.

### Buffered I/O in Proxy

The proxy uses buffered I/O for performance. The flush strategy matters: server writes are flushed immediately (because the server sends multi-message responses), but client writes are flushed only on `ReadyForQuery` boundaries (to batch response messages). Changing the flush points can either hurt performance or cause clients to hang waiting for data.
