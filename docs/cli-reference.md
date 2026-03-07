# CLI Reference

pg-retest is a single binary with 8 subcommands for capturing, replaying, and comparing PostgreSQL workloads.

```
pg-retest [OPTIONS] <COMMAND>
```

## Global Flags

| Flag             | Short | Description              |
|------------------|-------|--------------------------|
| `--verbose`      | `-v`  | Enable verbose logging   |
| `--version`      |       | Print version and exit   |
| `--help`         | `-h`  | Print help and exit      |

The `-v` flag is global and can appear before or after the subcommand. It enables `tracing` output at the debug level for all modules.


## Subcommands

---

### `pg-retest capture`

Capture workload from PostgreSQL CSV logs, MySQL slow query logs, or AWS RDS/Aurora logs and produce a `.wkl` workload profile.

```
pg-retest capture [OPTIONS]
```

| Flag               | Default       | Description                                                      |
|--------------------|---------------|------------------------------------------------------------------|
| `--source-log`     | *(none)*      | Path to source log file (required for `pg-csv` and `mysql-slow`) |
| `--source-type`    | `pg-csv`      | Source log type: `pg-csv`, `mysql-slow`, or `rds`                |
| `-o`, `--output`   | `workload.wkl`| Output workload profile path                                    |
| `--source-host`    | `unknown`     | Source host identifier stored in profile metadata                |
| `--pg-version`     | `unknown`     | PostgreSQL version stored in profile metadata                    |
| `--mask-values`    | `false`       | Mask string and numeric literals in SQL for PII protection       |
| `--rds-instance`   | *(none)*      | RDS instance identifier (required for `--source-type rds`)       |
| `--rds-region`     | `us-east-1`   | AWS region for the RDS instance                                  |
| `--rds-log-file`   | *(none)*      | Specific RDS log file to download (omit to use the latest)       |

**Examples:**

```bash
# Capture from a PostgreSQL CSV log
pg-retest capture --source-log /var/log/postgresql/postgresql.csv -o workload.wkl

# Capture from a MySQL slow query log with PII masking
pg-retest capture --source-type mysql-slow --source-log /var/log/mysql/slow.log \
    --mask-values -o mysql-workload.wkl

# Capture from AWS RDS/Aurora
pg-retest capture --source-type rds --rds-instance my-db-instance \
    --rds-region us-west-2 -o rds-workload.wkl

# Capture a specific RDS log file
pg-retest capture --source-type rds --rds-instance my-db-instance \
    --rds-log-file error/postgresql.log.2024-03-08-10 -o rds-workload.wkl
```

**Notes:**
- MySQL slow log capture (`--source-type mysql-slow`) automatically applies the SQL transform pipeline to convert MySQL syntax to PostgreSQL-compatible SQL. MySQL-specific commands (`SHOW`, `SET NAMES`, `USE`) are skipped.
- RDS capture (`--source-type rds`) requires the `aws` CLI to be installed and configured with appropriate IAM permissions. Large log files (>1MB) are downloaded in paginated chunks.
- PII masking (`--mask-values`) replaces string literals with `$S` and numeric literals with `$N` using a hand-written character-level state machine that handles SQL edge cases (escaped quotes, dollar-quoting).

---

### `pg-retest replay`

Replay a captured workload against a target PostgreSQL instance.

```
pg-retest replay [OPTIONS] --workload <PATH> --target <CONNSTRING>
```

| Flag                     | Default        | Description                                                     |
|--------------------------|----------------|-----------------------------------------------------------------|
| `--workload`             | *(required)*   | Path to the workload profile (`.wkl`)                           |
| `--target`               | *(required)*   | Target PostgreSQL connection string                             |
| `-o`, `--output`         | `results.wkl`  | Output results profile path                                    |
| `--read-only`            | `false`        | Replay only SELECT queries (strip all DML)                     |
| `--speed`                | `1.0`          | Speed multiplier (e.g., `2.0` = 2x faster, `0.5` = half speed)|
| `--scale`                | `1`            | Uniform scale factor: duplicate all sessions N times            |
| `--stagger-ms`           | `0`            | Stagger interval in milliseconds between scaled copies          |
| `--scale-analytical`     | *(none)*       | Scale factor for Analytical sessions only                       |
| `--scale-transactional`  | *(none)*       | Scale factor for Transactional sessions only                    |
| `--scale-mixed`          | *(none)*       | Scale factor for Mixed sessions only                            |
| `--scale-bulk`           | *(none)*       | Scale factor for Bulk sessions only                             |

**Examples:**

```bash
# Basic replay
pg-retest replay --workload workload.wkl \
    --target "host=localhost port=5432 dbname=mydb user=myuser password=mypass"

# Read-only replay at 2x speed
pg-retest replay --workload workload.wkl --target "..." --read-only --speed 2.0

# Scale all sessions 4x with 100ms stagger
pg-retest replay --workload workload.wkl --target "..." --scale 4 --stagger-ms 100

# Per-category scaling: 2x analytical, 8x transactional
pg-retest replay --workload workload.wkl --target "..." \
    --scale-analytical 2 --scale-transactional 8 --stagger-ms 50
```

**Notes:**
- Per-category scaling flags (`--scale-analytical`, etc.) are mutually exclusive with uniform `--scale`. If any per-category flag is set, it takes priority; unspecified categories default to 1x.
- Scaling write workloads (`--scale N` with DML) prints a safety warning because scaled writes execute multiple times and change data state. For accurate results with DML, restore from a point-in-time backup before each replay.
- Each session in the workload profile is replayed as a separate async Tokio task, preserving the original connection-level parallelism.
- Transaction-aware replay: failed statements within a transaction trigger an automatic rollback.

---

### `pg-retest compare`

Compare a source workload profile with replay results to produce a performance comparison report.

```
pg-retest compare [OPTIONS] --source <PATH> --replay <PATH>
```

| Flag                   | Default  | Description                                                    |
|------------------------|----------|----------------------------------------------------------------|
| `--source`             | *(required)* | Path to the source workload profile (`.wkl`)               |
| `--replay`             | *(required)* | Path to the replay results profile (`.wkl`)                |
| `--json`               | *(none)*     | Output JSON report to this path                            |
| `--threshold`          | `20.0`       | Regression threshold percentage (flag queries slower by this %) |
| `--fail-on-regression` | `false`      | Exit non-zero if regressions are detected                  |
| `--fail-on-error`      | `false`      | Exit non-zero if query errors occurred during replay       |

**Examples:**

```bash
# Basic comparison with terminal output
pg-retest compare --source workload.wkl --replay results.wkl

# JSON report with 10% regression threshold
pg-retest compare --source workload.wkl --replay results.wkl \
    --json report.json --threshold 10.0

# CI mode: fail on any regression or error
pg-retest compare --source workload.wkl --replay results.wkl \
    --fail-on-regression --fail-on-error --threshold 15.0
```

**Notes:**
- The report includes per-query latency comparison (source vs. replay), regression flags for queries exceeding the threshold, total throughput, and error counts.
- Terminal output shows a formatted table with color-coded regression indicators.
- When `--fail-on-regression` is set, the process exits with code 1 if any query exceeds the regression threshold.

---

### `pg-retest inspect`

Inspect a workload profile file and display its contents as JSON.

```
pg-retest inspect [OPTIONS] <PATH>
```

| Flag         | Default | Description                                       |
|--------------|---------|---------------------------------------------------|
| `<PATH>`     | *(required, positional)* | Path to the workload profile (`.wkl`) |
| `--classify` | `false` | Show workload classification breakdown             |

**Examples:**

```bash
# Inspect a workload profile
pg-retest inspect workload.wkl

# Inspect with classification breakdown
pg-retest inspect --classify workload.wkl
```

**Notes:**
- `.wkl` files are MessagePack binary (v2 format). This command decodes them and displays the contents as human-readable JSON.
- With `--classify`, each session is classified as Analytical, Transactional, Mixed, or Bulk based on read/write ratio, latency, and transaction count.

---

### `pg-retest proxy`

Run a PG wire protocol capture proxy between clients and a PostgreSQL server. The proxy records all SQL traffic as a workload profile.

```
pg-retest proxy [OPTIONS] --target <ADDRESS>
```

| Flag              | Default          | Description                                                     |
|-------------------|------------------|-----------------------------------------------------------------|
| `--listen`        | `0.0.0.0:5433`   | Address and port to listen on                                  |
| `--target`        | *(required)*     | Target PostgreSQL address (e.g., `localhost:5432`)              |
| `-o`, `--output`  | `workload.wkl`   | Output workload profile path                                   |
| `--pool-size`     | `100`            | Maximum server connections in the pool                          |
| `--pool-timeout`  | `30`             | Timeout in seconds waiting for a pool connection                |
| `--mask-values`   | `false`          | Mask string and numeric literals in captured SQL                |
| `--no-capture`    | `false`          | Run in proxy-only mode (no workload capture)                   |
| `--duration`      | *(none)*         | Capture duration (e.g., `60s`, `5m`). Runs until Ctrl+C if not set |

**Examples:**

```bash
# Basic proxy capture
pg-retest proxy --target localhost:5432 -o workload.wkl

# Proxy with PII masking and 5-minute capture window
pg-retest proxy --target localhost:5432 --mask-values --duration 5m

# Proxy-only mode (no capture), custom pool settings
pg-retest proxy --target db.example.com:5432 --no-capture \
    --pool-size 200 --pool-timeout 60

# Listen on a specific port
pg-retest proxy --listen 0.0.0.0:15432 --target localhost:5432
```

**Notes:**
- The proxy uses session-mode connection pooling with buffered I/O for minimal overhead.
- Handles SCRAM-SHA-256 authentication by relaying auth messages between client and server.
- Supports both SIGINT (Ctrl+C) and SIGTERM for graceful shutdown (Docker/Kubernetes compatible).
- Point your application's connection string at the proxy listen address instead of the real database to capture traffic transparently.

---

### `pg-retest run`

Run a full CI/CD pipeline defined in a TOML configuration file. The pipeline executes stages in order: capture, provision, replay, compare, threshold evaluation, and report generation.

```
pg-retest run [OPTIONS]
```

| Flag       | Default            | Description                              |
|------------|--------------------|------------------------------------------|
| `--config` | `.pg-retest.toml`  | Path to the pipeline config file (TOML)  |

**Examples:**

```bash
# Run with default config path
pg-retest run

# Run with a specific config file
pg-retest run --config ci/performance-test.toml
```

**Notes:**
- The TOML config defines all pipeline parameters: capture source, Docker provisioner settings, replay options, comparison thresholds, and report output.
- When `[[variants]]` sections are present in the config, the pipeline runs in A/B mode: it bypasses normal provisioning and replays the workload against each variant target sequentially.
- Exit codes are staged: 0 = pass, 1 = threshold violation, 2 = config error, 3 = capture error, 4 = provision error, 5 = replay error.
- Supports JUnit XML output for integration with CI test result viewers.
- Docker provisioner manages container lifecycle via CLI subprocess (start, restore backup, teardown).

---

### `pg-retest ab`

Compare replay performance across two or more database targets (A/B variant testing).

```
pg-retest ab [OPTIONS] --workload <PATH> --variant <LABEL=CONNSTRING> --variant <LABEL=CONNSTRING>
```

| Flag            | Default      | Description                                                   |
|-----------------|--------------|---------------------------------------------------------------|
| `--workload`    | *(required)* | Path to the workload profile (`.wkl`)                         |
| `--variant`     | *(required, 2+ times)* | Variant definition in `label=connection_string` format |
| `--read-only`   | `false`      | Replay only SELECT queries                                    |
| `--speed`       | `1.0`        | Speed multiplier for all variants                             |
| `--json`        | *(none)*     | Output JSON report to this path                               |
| `--threshold`   | `20.0`       | Regression threshold percentage for winner determination      |

**Examples:**

```bash
# Compare two PostgreSQL configurations
pg-retest ab --workload workload.wkl \
    --variant "pg15-default=host=server1 dbname=testdb user=app password=secret" \
    --variant "pg16-tuned=host=server2 dbname=testdb user=app password=secret"

# Read-only A/B test with JSON output
pg-retest ab --workload workload.wkl --read-only --json ab-report.json \
    --variant "baseline=host=localhost port=5432 dbname=test user=test" \
    --variant "optimized=host=localhost port=5433 dbname=test user=test"

# Three-way comparison with tight threshold
pg-retest ab --workload workload.wkl --threshold 5.0 \
    --variant "v1=..." --variant "v2=..." --variant "v3=..."
```

**Notes:**
- The workload is replayed sequentially against each variant. Each variant gets a fresh replay run.
- Per-query regression detection uses positional matching between variants.
- Winner is determined by average latency across all queries.
- You must specify at least 2 variants.

---

### `pg-retest web`

Launch the web dashboard -- a browser-based interface for all pg-retest operations.

```
pg-retest web [OPTIONS]
```

| Flag         | Default  | Description                                        |
|--------------|----------|----------------------------------------------------|
| `--port`     | `8080`   | HTTP port to listen on                             |
| `--data-dir` | `./data` | Data directory for SQLite database and workload files |

**Examples:**

```bash
# Start with defaults
pg-retest web

# Custom port and data directory
pg-retest web --port 3000 --data-dir /var/lib/pg-retest

# With verbose logging
pg-retest -v web --port 8080
```

**Notes:**
- The dashboard is a single-page application served from embedded static files. No external web server or build step is required.
- SQLite database is created automatically in the data directory.
- Workload files are stored in `{data-dir}/workloads/`, replay results in `{data-dir}/results/`.
- See the [Web Dashboard Guide](web-dashboard.md) for full documentation.


## Exit Codes

pg-retest uses structured exit codes, primarily relevant for the `run` (pipeline) and `compare` subcommands:

| Code | Meaning              | Description                                              |
|------|----------------------|----------------------------------------------------------|
| 0    | Pass                 | All operations completed successfully; thresholds met    |
| 1    | Threshold violation  | Replay completed but performance thresholds were exceeded|
| 2    | Config error         | Invalid configuration file or arguments                  |
| 3    | Capture error        | Workload capture failed                                  |
| 4    | Provision error      | Database provisioning (Docker container) failed          |
| 5    | Replay error         | Workload replay failed                                   |

For the `compare` subcommand, exit code 1 is returned when `--fail-on-regression` or `--fail-on-error` flags are set and the corresponding condition is detected.


## Environment Variables

| Variable    | Description                                                                              |
|-------------|------------------------------------------------------------------------------------------|
| `RUST_LOG`  | Controls log verbosity via the `tracing` crate. Example values: `debug`, `info`, `warn`, `pg_retest=debug`, `pg_retest::replay=trace`. Combine with `-v` for full diagnostic output. |

**Example:**

```bash
# Debug logging for all modules
RUST_LOG=debug pg-retest -v replay --workload workload.wkl --target "..."

# Trace-level logging for the replay module only
RUST_LOG=pg_retest::replay=trace pg-retest replay --workload workload.wkl --target "..."

# Info-level logging (default with -v)
RUST_LOG=info pg-retest -v capture --source-log server.csv
```
