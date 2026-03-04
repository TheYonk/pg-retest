# pg-retest

Capture, replay, and compare PostgreSQL workloads.

pg-retest captures SQL workload from PostgreSQL server logs, replays it against a target database, and produces a side-by-side performance comparison report. Use it to validate configuration changes, server migrations, and capacity planning.

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
```

## PostgreSQL Logging Setup

pg-retest captures workload by parsing PostgreSQL CSV logs. You need to configure your PostgreSQL server to produce these logs.

### Check Current Settings

Connect to your PostgreSQL server and check if logging is already configured:

```sql
SHOW logging_collector;              -- Must be 'on'
SHOW log_destination;                -- Must include 'csvlog'
SHOW log_min_duration_statement;     -- Check current value
```

### Configure Logging

Add or modify these settings in `postgresql.conf`:

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

### Which settings require a restart?

| Setting | Restart required? | Notes |
|---------|:-:|-------|
| `log_statement = 'all'` | No | `ALTER SYSTEM` + `SELECT pg_reload_conf()` |
| `log_duration = on` | No | Reload only |
| `log_destination = 'csvlog'` | No | Reload only |
| `log_min_duration_statement = 0` | No | Reload only |
| `logging_collector = on` | **Yes** | Only if not already enabled (usually is in production) |

### Apply Changes

If `logging_collector` was already `on`:

```bash
# No restart needed — just reload config
pg_ctl reload -D /path/to/data
# OR from SQL:
# SELECT pg_reload_conf();
```

If `logging_collector` was `off` (requires restart):

```bash
pg_ctl restart -D /path/to/data
```

### Verify Logging

After applying changes, run a few queries and check that CSV logs appear:

```bash
ls -la /path/to/data/log/*.csv
# You should see files like: postgresql-2024-03-08.csv
```

### Log File Location

- **Default:** `$PGDATA/log/` directory
- **Custom:** Set `log_directory` in postgresql.conf
- **RDS/Aurora:** Download logs via AWS Console, CLI, or RDS API
- **Cloud SQL:** Access via Google Cloud Console or gcloud CLI
- **Azure:** Access via Azure Portal or az CLI

### Performance Impact

Logging with `log_min_duration_statement = 0` has minimal overhead on most workloads (typically <2% throughput impact). For extremely high-throughput systems (>50k queries/sec), consider:

- Setting `log_min_duration_statement = 1` to skip sub-millisecond queries
- Capturing during off-peak windows
- Using `log_statement = 'none'` and relying on `auto_explain` instead

## Replay Modes

### Read-Write (default)

Replays all captured queries including INSERT, UPDATE, DELETE. **Important:** use a backup or snapshot of your database — DML will modify data.

```bash
pg-retest replay --workload workload.wkl --target "host=localhost dbname=mydb user=postgres"
```

### Read-Only

Strips all DML (INSERT, UPDATE, DELETE) and DDL (CREATE, ALTER, DROP), replaying only SELECT queries. Safe to run against production data.

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

## Comparison Report

The compare command produces a terminal summary and optional JSON report:

```bash
pg-retest compare --source workload.wkl --replay results.wkl --json report.json --threshold 20
```

- `--threshold`: Flag queries that are slower by this percentage (default: 20%)

### Report Metrics

| Metric | Description |
|--------|-------------|
| Total queries | Count of queries in source vs. replay |
| Avg/P50/P95/P99 latency | Latency percentiles (microseconds) |
| Errors | Queries that failed during replay |
| Regressions | Individual queries exceeding the threshold |

## Workload Profile Format

Profiles are stored as MessagePack binary files (`.wkl`). Use `inspect` to view as JSON:

```bash
pg-retest inspect workload.wkl | jq .
```

## Building

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run tests
cargo test

# Run a single test file
cargo test --test profile_io_test

# Run a single test function
cargo test --test compare_test test_comparison_regressions

# Run with verbose logging
RUST_LOG=debug pg-retest capture --source-log ...
```
