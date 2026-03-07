# pg-retest

Capture, replay, and compare PostgreSQL workloads.

pg-retest captures SQL workload from multiple sources (PostgreSQL logs, wire protocol proxy, MySQL slow logs, AWS RDS/Aurora), replays it against a target database with full connection parallelism and transaction awareness, and produces performance comparison reports. Use it to validate configuration changes, server migrations, capacity planning, and cross-database migrations to PostgreSQL.

## Features

- **Multiple capture methods** -- PG CSV logs, wire protocol proxy, MySQL slow query logs, AWS RDS/Aurora
- **Transaction-aware replay** -- tracks BEGIN/COMMIT/ROLLBACK boundaries, auto-rollback on failure
- **Read-only mode** -- strip DML for safe replay against production replicas
- **Speed control** -- compress or stretch timing between queries
- **Scaled benchmark** -- duplicate sessions N times with staggered offsets for load testing
- **Per-category scaling** -- scale Analytical, Transactional, Mixed, and Bulk workloads independently
- **Workload classification** -- categorize sessions as Analytical, Transactional, Mixed, or Bulk
- **PII masking** -- strip string and numeric literals from captured SQL
- **Comparison reports** -- per-query latency regression detection with threshold evaluation and exit codes
- **A/B variant testing** -- compare replay performance across different database targets
- **CI/CD pipeline** -- TOML-driven automation with Docker provisioning, JUnit XML output, and pass/fail thresholds
- **Cross-database capture** -- capture from MySQL and transform SQL to PostgreSQL-compatible syntax
- **Web dashboard** -- browser-based UI for managing workloads, running replays, and viewing reports
- **Capacity planning** -- throughput QPS, latency percentiles, error rates at scale

## Quick Start

```bash
# Build
cargo build --release

# 1. Capture workload from PG CSV logs
pg-retest capture --source-log /path/to/postgresql.csv --output workload.wkl

# 2. Replay against target database
pg-retest replay --workload workload.wkl --target "host=localhost dbname=mydb user=postgres"

# 3. Compare results
pg-retest compare --source workload.wkl --replay results.wkl --json report.json

# Inspect a workload profile
pg-retest inspect workload.wkl

# Launch the web dashboard
pg-retest web --port 8080
```

## Web Dashboard

pg-retest includes a browser-based dashboard for managing the full capture/replay/compare workflow without the command line.

```bash
pg-retest web --port 8080 --data-dir ./data
```

Open `http://localhost:8080` in your browser. The dashboard includes 9 pages:

- **Dashboard** -- overview of workloads, recent runs, and status
- **Workloads** -- upload, import, inspect, classify, and delete workload profiles
- **Proxy** -- start/stop the capture proxy with live traffic view via WebSocket
- **Replay** -- configure and launch replays with real-time progress updates
- **A/B Testing** -- run A/B comparisons across database variants
- **Compare** -- view comparison reports with charts
- **Pipeline** -- configure and run CI/CD pipelines
- **History** -- browse historical runs with filtering and trends
- **Help** -- reference documentation

The dashboard uses WebSocket for real-time updates (proxy traffic, replay progress, pipeline status). Workload profiles (`.wkl` files) are stored on disk; metadata is tracked in an embedded SQLite database. The frontend uses Alpine.js, Chart.js, and Tailwind CSS loaded via CDN -- no frontend build step required.

## Capture Methods

pg-retest supports four capture backends. All produce the same `.wkl` workload profile format, which can be replayed interchangeably.

### PostgreSQL CSV Log Capture

The default capture method. Parses PostgreSQL CSV log files to extract per-connection SQL streams with timing metadata.

```bash
pg-retest capture \
  --source-log /path/to/postgresql.csv \
  --output workload.wkl \
  --source-host prod-db-01 \
  --pg-version 16.2 \
  --mask-values
```

#### PostgreSQL Logging Setup

pg-retest captures workload by parsing PostgreSQL CSV logs. You need to configure your PostgreSQL server to produce these logs.

**Check current settings:**

```sql
SHOW logging_collector;              -- Must be 'on'
SHOW log_destination;                -- Must include 'csvlog'
SHOW log_min_duration_statement;     -- Check current value
```

**Configure logging** in `postgresql.conf`:

```ini
# Required: enables the log file collector process
# NOTE: changing this requires a PostgreSQL RESTART if not already 'on'
logging_collector = on

# Required: enable CSV log output
# Change takes effect after: pg_ctl reload (no restart needed)
log_destination = 'csvlog'

# Recommended: log all statements with their duration in one line
# Change takes effect after: pg_ctl reload (no restart needed)
log_min_duration_statement = 0    # logs every statement with duration

# Alternative: log statements and duration separately
# log_statement = 'all'           # logs all SQL statements
# log_duration = on               # logs duration of each statement

# Optional: useful log file naming
log_filename = 'postgresql-%Y-%m-%d.log'
log_rotation_age = 1d
```

**Which settings require a restart?**

| Setting | Restart required? | Notes |
|---------|:-:|-------|
| `log_statement = 'all'` | No | `ALTER SYSTEM` + `SELECT pg_reload_conf()` |
| `log_duration = on` | No | Reload only |
| `log_destination = 'csvlog'` | No | Reload only |
| `log_min_duration_statement = 0` | No | Reload only |
| `logging_collector = on` | **Yes** | Only if not already enabled (usually is in production) |

**Apply changes:**

If `logging_collector` was already `on`:

```bash
# No restart needed -- just reload config
pg_ctl reload -D /path/to/data
# OR from SQL:
# SELECT pg_reload_conf();
```

If `logging_collector` was `off` (requires restart):

```bash
pg_ctl restart -D /path/to/data
```

**Verify logging:**

```bash
ls -la /path/to/data/log/*.csv
# You should see files like: postgresql-2024-03-08.csv
```

**Log file locations:**

- **Default:** `$PGDATA/log/` directory
- **Custom:** Set `log_directory` in postgresql.conf
- **RDS/Aurora:** Use the `--source-type rds` capture method (see below)

**Performance impact:**

Logging with `log_min_duration_statement = 0` has minimal overhead on most workloads (typically <2% throughput impact). For extremely high-throughput systems (>50k queries/sec), consider setting `log_min_duration_statement = 1` to skip sub-millisecond queries, or capturing during off-peak windows.

### Proxy Capture

A PostgreSQL wire protocol proxy that sits between your application and database, capturing all SQL traffic with zero application changes.

```bash
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --output workload.wkl \
  --pool-size 100 \
  --pool-timeout 30 \
  --mask-values
```

Point your application at the proxy (port 5433) instead of the database directly. The proxy transparently relays all traffic while capturing queries. It handles SCRAM-SHA-256 authentication, session-mode connection pooling, and buffered I/O for near-zero overhead.

Options:
- `--no-capture` -- run as a proxy without capturing (useful for testing connectivity)
- `--duration 5m` -- capture for a fixed duration, then shut down (supports `s`, `m` suffixes)
- `--mask-values` -- strip PII from captured SQL literals

Stop with Ctrl+C (SIGINT) or SIGTERM -- the proxy writes the `.wkl` file on graceful shutdown.

### MySQL Slow Query Log Capture

Capture workload from MySQL slow query logs and automatically transform SQL syntax to PostgreSQL-compatible format.

```bash
pg-retest capture \
  --source-type mysql-slow \
  --source-log /path/to/mysql-slow.log \
  --output workload.wkl
```

The transform pipeline automatically converts:
- Backtick-quoted identifiers to double-quoted identifiers
- `LIMIT offset, count` to `LIMIT count OFFSET offset`
- `IFNULL()` to `COALESCE()`
- `IF(cond, a, b)` to `CASE WHEN cond THEN a ELSE b END`
- `UNIX_TIMESTAMP(col)` to `EXTRACT(EPOCH FROM col)`

MySQL-specific commands (`SHOW`, `SET NAMES`, `USE`, etc.) are automatically filtered out. The transform covers approximately 80-90% of real MySQL queries.

### AWS RDS/Aurora Capture

Capture directly from RDS or Aurora instances via the AWS CLI.

```bash
pg-retest capture \
  --source-type rds \
  --rds-instance mydb-instance \
  --rds-region us-west-2 \
  --output workload.wkl
```

Requires the `aws` CLI to be installed and configured with appropriate IAM permissions. If `--rds-log-file` is omitted, the most recent log file is used. Large log files (>1MB) are downloaded in paginated chunks.

## Replay

The replay engine reads a workload profile and replays it against a target PostgreSQL instance, preserving connection parallelism and inter-query timing.

```bash
pg-retest replay --workload workload.wkl --target "host=localhost dbname=mydb user=postgres"
```

### Read-Write Mode (default)

Replays all captured queries including INSERT, UPDATE, DELETE. Use a backup or snapshot of your database -- DML will modify data.

### Read-Only Mode

Strips all DML (INSERT, UPDATE, DELETE), DDL (CREATE, ALTER, DROP), and transaction control (BEGIN, COMMIT, ROLLBACK), replaying only SELECT queries. Safe to run against production data.

```bash
pg-retest replay --workload workload.wkl --target "..." --read-only
```

### Speed Control

Compress or stretch timing gaps between queries:

```bash
# 2x faster (halves wait times between queries)
pg-retest replay --workload workload.wkl --target "..." --speed 2.0

# Half speed (doubles wait times)
pg-retest replay --workload workload.wkl --target "..." --speed 0.5
```

### Scaled Benchmark

Duplicate sessions N times for load testing:

```bash
# 4x the original sessions, staggered 500ms apart
pg-retest replay --workload workload.wkl --target "..." --scale 4 --stagger-ms 500
```

This produces a capacity planning report with throughput (queries/sec), latency percentiles, and error rates.

Note: scaling write workloads executes DML multiple times and changes data state. A safety warning is printed when scaling with DML queries present.

### Per-Category Scaling

Scale different workload categories independently for targeted capacity testing:

```bash
pg-retest replay \
  --workload workload.wkl \
  --target "..." \
  --scale-analytical 2 \
  --scale-transactional 4 \
  --scale-mixed 1 \
  --scale-bulk 0 \
  --stagger-ms 500
```

Per-category scaling is mutually exclusive with uniform `--scale N`. If any category flag is set, per-category mode is used and unspecified categories default to 1x.

### Transaction-Aware Replay

pg-retest tracks transaction boundaries (BEGIN/COMMIT/ROLLBACK) during capture and provides transaction-aware replay:

- Queries within a transaction share a `transaction_id`
- If a query inside a transaction fails, the replay engine automatically issues a ROLLBACK and skips remaining queries in that transaction
- COMMIT for a failed transaction is converted to a no-op

## Compare

The compare command produces a terminal summary and optional JSON report:

```bash
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --json report.json \
  --threshold 20 \
  --fail-on-regression \
  --fail-on-error
```

### Report Metrics

| Metric | Description |
|--------|-------------|
| Total queries | Count of queries in source vs. replay |
| Avg/P50/P95/P99 latency | Latency percentiles (microseconds) |
| Errors | Queries that failed during replay |
| Regressions | Individual queries exceeding the threshold |

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | PASS -- all checks passed |
| 1 | FAIL -- regressions detected (with `--fail-on-regression`) |
| 2 | FAIL -- query errors detected (with `--fail-on-error`) |

When both flags are set, errors take priority over regressions.

### Threshold Evaluation

The CI/CD pipeline supports detailed threshold checks:

| Threshold | Description |
|-----------|-------------|
| `p95_max_ms` | Maximum allowed P95 latency in milliseconds |
| `p99_max_ms` | Maximum allowed P99 latency in milliseconds |
| `error_rate_max_pct` | Maximum allowed error rate as a percentage |
| `regression_max_count` | Maximum number of individual query regressions allowed |

## CI/CD Pipeline

Automate the full capture-provision-replay-compare cycle with a single TOML config file:

```bash
pg-retest run --config .pg-retest.toml
```

### Pipeline Config Example

```toml
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
```

### Pipeline Sections

- **[capture]** -- specify a `workload` path (skip capture) or `source_log` to capture from. Supports `source_type` values: `pg-csv`, `mysql-slow`, `rds`.
- **[provision]** -- Docker provisioning (`backend = "docker"`) with optional `restore_from` for database backup restore. Or use `connection_string` to point at a pre-existing target.
- **[replay]** -- speed, read-only mode, scaling options. If no `[provision]` section, specify `target` here.
- **[thresholds]** -- pass/fail criteria for CI integration.
- **[output]** -- `json_report` for detailed JSON output, `junit_xml` for CI test result integration.

### Pipeline Exit Codes

| Code | Meaning |
|------|---------|
| 0 | PASS |
| 1 | Threshold violation |
| 2 | Config error |
| 3 | Capture error |
| 4 | Provision error |
| 5 | Replay error |

### Minimal Pipeline Config

If you already have a captured workload and a running target database:

```toml
[capture]
workload = "existing.wkl"

[replay]
target = "host=localhost dbname=test user=postgres"
```

### MySQL Pipeline Config

```toml
[capture]
source_log = "mysql_slow.log"
source_type = "mysql-slow"
source_host = "mysql-prod"

[replay]
target = "host=localhost dbname=test user=postgres"
read_only = true
```

### Per-Category Scaling Pipeline Config

```toml
[capture]
workload = "workload.wkl"

[replay]
target = "host=localhost dbname=test user=postgres"
scale_analytical = 2
scale_transactional = 4
scale_mixed = 1
scale_bulk = 0
stagger_ms = 500
```

## A/B Variant Testing

Compare replay performance across two or more database targets -- useful for evaluating configuration changes, version upgrades, or hardware differences.

### CLI Usage

```bash
pg-retest ab \
  --workload workload.wkl \
  --variant "pg16-default=host=db1 dbname=app user=postgres" \
  --variant "pg16-tuned=host=db2 dbname=app user=postgres" \
  --read-only \
  --threshold 20 \
  --json ab_report.json
```

Each variant is defined as `label=connection_string`. At least two variants are required. The workload is replayed sequentially against each target, and results are compared with per-query regression detection and winner determination by average latency.

### A/B via Pipeline Config

```toml
[capture]
workload = "workload.wkl"

[[variants]]
label = "pg16-default"
target = "host=db1 dbname=app user=postgres"

[[variants]]
label = "pg16-tuned"
target = "host=db2 dbname=app user=postgres"

[replay]
speed = 1.0
read_only = true
```

When `[[variants]]` are present, the pipeline bypasses normal provisioning and runs sequential replay against each variant target.

## Workload Classification

Classify captured workloads to understand their characteristics:

```bash
pg-retest inspect workload.wkl --classify
```

| Class | Criteria |
|-------|---------|
| **Analytical** | >80% reads, avg latency >10ms (OLAP pattern) |
| **Transactional** | >20% writes, avg latency <5ms, >2 transactions (OLTP pattern) |
| **Bulk** | >80% writes, <=2 transactions (data loading) |
| **Mixed** | Everything else |

Classification outputs per-session breakdown with read/write percentages, average latency, and transaction count. Classification drives per-category scaling behavior.

## PII Masking

The `--mask-values` flag (available on `capture` and `proxy` commands) replaces string literals with `$S` and numeric literals with `$N`:

```
-- Original:
SELECT * FROM users WHERE email = 'alice@corp.com' AND id = 42

-- Masked:
SELECT * FROM users WHERE email = $S AND id = $N
```

Masking uses a hand-written character-level state machine (not regex) to correctly handle SQL edge cases: escaped quotes (`''`), dollar-quoted strings (`$$...$$`), and numbers inside identifiers (`table3`, `col1`).

## Workload Profile Format

Profiles are stored as MessagePack binary files (`.wkl`, v2 format). Use `inspect` to view as JSON:

```bash
pg-retest inspect workload.wkl | jq .
```

The profile contains:
- Metadata (source host, PG version, capture method, timestamp)
- Per-session query lists with SQL text, timing, and query kind classification
- Transaction IDs linking queries within the same transaction

v2 profiles include transaction IDs on queries. v1 profiles (without transaction support) are fully backward compatible.

The `capture_method` field distinguishes sources: `"csv_log"` for PG CSV logs, `"mysql_slow_log"` for MySQL, `"rds"` for AWS RDS/Aurora, and proxy capture.

## Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests (174 tests)
cargo test

# Run a single test file
cargo test --test profile_io_test

# Run a single test function
cargo test --test compare_test test_comparison_regressions

# Run with verbose logging
RUST_LOG=debug pg-retest capture --source-log ...

# Lint
cargo clippy

# Format
cargo fmt
```

## Subcommands

| Command | Description |
|---------|-------------|
| `capture` | Capture workload from PostgreSQL logs, MySQL logs, or RDS |
| `replay` | Replay a captured workload against a target database |
| `compare` | Compare source workload with replay results |
| `inspect` | Inspect a workload profile file (optionally with classification) |
| `proxy` | Run a capture proxy between clients and PostgreSQL |
| `run` | Run full CI/CD pipeline from TOML config |
| `ab` | Compare replay performance across different database targets |
| `web` | Launch the web dashboard |

## Documentation

Design documents and implementation plans are available in the `docs/` directory:

- `docs/plans/` -- Design and implementation plans for each milestone
- `docs/plans/2026-03-03-pg-retest-m1-design.md` -- M1 Capture & Replay design
- `docs/plans/2026-03-04-m3-cicd-design.md` -- M3 CI/CD integration design
- `docs/plans/2026-03-04-m4-mysql-capture-design.md` -- M4 MySQL capture design
- `docs/plans/2026-03-04-proxy-gateway-design.md` -- Proxy capture design
- `docs/plans/2026-03-05-gap-closure-design.md` -- Per-category scaling, A/B testing, RDS capture
- `docs/plans/2026-03-04-m5-ai-tuning-design.md` -- M5 AI-Assisted Tuning design (planned)
