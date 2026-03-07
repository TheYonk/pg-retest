# CI/CD Pipeline Guide

## Overview

The pg-retest pipeline automates the full cycle of database workload testing: capture a SQL workload, provision a target database, replay the workload, compare performance, evaluate pass/fail thresholds, and produce reports. It is designed to run as a CI/CD step so that configuration changes, PostgreSQL upgrades, and infrastructure migrations can be validated automatically before reaching production.

The pipeline executes six stages in sequence:

```
Capture --> Provision --> Replay --> Compare --> Threshold --> Report
```

Each stage has a dedicated exit code (see Exit Codes below) so that CI systems can distinguish the failure type.


## Running the Pipeline

```bash
pg-retest run --config .pg-retest.toml
```

The `run` subcommand reads a TOML configuration file and executes every stage end-to-end. If `--config` is omitted, it defaults to `.pg-retest.toml` in the current directory.

Enable verbose logging for troubleshooting:

```bash
RUST_LOG=debug pg-retest -v run --config .pg-retest.toml
```


## TOML Configuration Example

Below is a fully annotated configuration showing every section. Only `[capture]` (with a workload source) and `[replay]` (with a target) are strictly required.

```toml
# ── Capture ────────────────────────────────────────────────
[capture]
# Path to an existing workload profile. If set, the capture
# stage is skipped and this file is used directly.
# workload = "workloads/baseline.wkl"

# Path to a PG CSV log file to capture from.
source_log = "pg_log.csv"

# Source type: "pg-csv" (default), "mysql-slow", or "rds".
source_type = "pg-csv"

# Metadata fields recorded in the workload profile.
source_host = "prod-db-01"
pg_version  = "16.2"

# Replace string and numeric literals with $S / $N for PII protection.
mask_values = true

# RDS-specific fields (only used when source_type = "rds").
# rds_instance = "mydb-instance"
# rds_region   = "us-east-1"        # default
# rds_log_file = "error/postgresql.log.2024-03-08-10"

# ── Provision ──────────────────────────────────────────────
[provision]
# Backend type. Currently only "docker" is supported.
backend = "docker"

# Docker image to use (default: "postgres:16").
image = "postgres:16.2"

# SQL file to restore into the container after startup.
restore_from = "backup.sql"

# Port to expose on the host. 0 or omitted = random port.
# port = 5442

# Skip Docker provisioning entirely by supplying a connection string.
# connection_string = "host=localhost port=5432 user=app dbname=app"

# ── Replay ─────────────────────────────────────────────────
[replay]
# Replay speed multiplier: 2.0 = twice as fast, 0.5 = half speed.
speed = 1.0

# When true, only SELECT queries are replayed (DML is stripped).
read_only = false

# Uniform scale factor: duplicate all sessions N times.
scale = 1

# Delay (ms) between each scaled copy of a session.
stagger_ms = 0

# Per-category scaling (mutually exclusive with uniform `scale`).
# If any of these are set, they take priority. Unspecified classes default to 1.
# scale_analytical    = 2
# scale_transactional = 4
# scale_mixed         = 1
# scale_bulk          = 0

# Direct target connection string. If set, the [provision] section is skipped.
# target = "host=localhost port=5432 user=app dbname=app"

# ── Thresholds ─────────────────────────────────────────────
[thresholds]
# Maximum acceptable P95 latency (ms). Fails if replay P95 exceeds this.
p95_max_ms = 50.0

# Maximum acceptable P99 latency (ms).
p99_max_ms = 200.0

# Maximum error rate as a percentage of total queries.
error_rate_max_pct = 1.0

# Maximum number of regressed queries allowed.
regression_max_count = 5

# A query is flagged as "regressed" if its latency increased by this
# percentage compared to the source workload. Default: 20.0.
regression_threshold_pct = 20.0

# ── Output ─────────────────────────────────────────────────
[output]
# Write a JSON comparison report.
json_report = "report.json"

# Write a JUnit XML report (for CI test result integration).
junit_xml = "results.xml"

# ── A/B Variants (optional) ───────────────────────────────
# When two or more [[variants]] are defined, the pipeline switches
# to A/B mode. Normal provisioning is bypassed; each variant is
# replayed sequentially against its own target.
#
# [[variants]]
# label  = "pg16-default"
# target = "host=db1 port=5432 dbname=app user=app"
#
# [[variants]]
# label  = "pg16-tuned"
# target = "host=db2 port=5432 dbname=app user=app"
```


## Pipeline Stages

### 1. Capture

The pipeline needs a workload profile (`.wkl` file) to replay. There are two paths:

- **Pre-existing workload**: Set `[capture].workload` to the path of an existing `.wkl` file. The capture stage is skipped entirely.
- **Capture from log**: Set `[capture].source_log` (and optionally `source_type`) to parse a log file and build a workload profile on the fly. Supported source types:
  - `pg-csv` -- PostgreSQL CSV log format (default).
  - `mysql-slow` -- MySQL slow query log. SQL transforms (backticks, LIMIT, IFNULL, IF, UNIX_TIMESTAMP) are applied automatically.
  - `rds` -- AWS RDS/Aurora. Downloads log files via the `aws` CLI, then parses them as PG CSV.

If `mask_values = true`, all string and numeric literals in captured SQL are replaced with `$S` and `$N` respectively before the workload enters the replay stage.

### 2. Provision

The provision stage creates a target PostgreSQL instance to replay against. Three options:

| Method | How to configure |
|--------|-----------------|
| Docker container | Set `[provision].backend = "docker"` with an optional `image` and `restore_from`. |
| Pre-existing server | Set `[provision].connection_string` to skip Docker and connect directly. |
| Direct target | Set `[replay].target` to bypass the `[provision]` section entirely. |

When Docker provisioning is used, the pipeline:

1. Runs `docker run` with the specified image, creating a container named `pg-retest-<pid>`.
2. Waits up to 30 seconds for PostgreSQL to become ready (polling with `pg_isready`).
3. If `restore_from` is set, copies the SQL file into the container and runs it via `psql`.
4. Builds a connection string using the mapped port.
5. After the pipeline completes (regardless of success or failure), runs `docker rm -f` to tear down the container.

### 3. Replay

The replay engine reads the workload profile and executes every session concurrently against the target, preserving the original connection parallelism and inter-query timing.

Key replay options:

- **speed**: Multiplier applied to inter-query delays. `2.0` replays at double speed; `0.5` at half speed.
- **read_only**: When true, only SELECT queries are executed. All INSERT, UPDATE, DELETE, and other DML statements are skipped.
- **scale / stagger_ms**: Duplicate every session `scale` times, with `stagger_ms` milliseconds between each copy. Useful for load testing.
- **Per-category scaling**: If any of `scale_analytical`, `scale_transactional`, `scale_mixed`, or `scale_bulk` are set, sessions are classified first, then each category is scaled independently. Unspecified categories default to 1x. This mode is mutually exclusive with uniform `scale`.

A safety warning is printed when scaling write workloads, because duplicated DML executes multiple times and changes data state.

### 4. Compare

After replay completes, the pipeline compares source workload timings against replay timings. It produces:

- Per-query latency comparison (source vs. replay).
- Total queries replayed and total errors.
- A list of regressed queries (those exceeding the regression threshold percentage).
- A terminal summary printed to stdout.

### 5. Threshold Evaluation

If a `[thresholds]` section is present, each configured threshold is evaluated against the comparison results:

| Threshold | What it checks |
|-----------|---------------|
| `p95_max_ms` | Replay P95 latency must be at or below this value (milliseconds). |
| `p99_max_ms` | Replay P99 latency must be at or below this value (milliseconds). |
| `error_rate_max_pct` | Percentage of failed queries must be at or below this value. |
| `regression_max_count` | Number of regressed queries must be at or below this count. |
| `regression_threshold_pct` | Defines what counts as a "regression" -- a query whose latency increased by at least this percentage. Default: 20%. |

Each threshold is reported as PASS or FAIL. If any threshold fails, the pipeline exits with code 1.

If no `[thresholds]` section is present, the pipeline always reports PASS (exit code 0), assuming replay itself succeeded.

### 6. Report

The final stage writes output files if configured:

- **JSON report** (`[output].json_report`): A machine-readable comparison report with per-query latency data, regressions, and summary statistics.
- **JUnit XML** (`[output].junit_xml`): A test result file compatible with CI systems. Each threshold check becomes a `<testcase>`. Failed thresholds include a `<failure>` element with a descriptive message.


## Docker Provisioning

The `[provision]` section controls how the target database is created. When `backend = "docker"`, the pipeline manages the full lifecycle:

```toml
[provision]
backend      = "docker"
image        = "postgres:16.2"   # Docker image (default: postgres:16)
restore_from = "backup.sql"      # SQL file to restore after startup
port         = 5442              # Host port (0 or omitted = random)
```

**Lifecycle:**

1. **Start**: `docker run -d` creates a detached container with the user `pgretest`, password `pgretest`, database `pgretest`.
2. **Health check**: The pipeline polls `pg_isready` inside the container every second, up to 30 attempts.
3. **Restore**: If `restore_from` is set, the file is copied into the container via `docker cp` and executed with `psql -f`.
4. **Replay**: The pipeline connects using the auto-generated connection string.
5. **Teardown**: After the pipeline finishes, `docker rm -f` removes the container.

To skip Docker and use an existing database, set either `[provision].connection_string` or `[replay].target`.


## Threshold Evaluation

Thresholds turn the pipeline into a pass/fail gate. Configure them in the `[thresholds]` section:

```toml
[thresholds]
p95_max_ms             = 50.0    # P95 replay latency must stay under 50ms
p99_max_ms             = 200.0   # P99 replay latency must stay under 200ms
error_rate_max_pct     = 1.0     # No more than 1% of queries may fail
regression_max_count   = 5       # No more than 5 queries may regress
regression_threshold_pct = 20.0  # A query is "regressed" if >20% slower
```

The pipeline prints a summary after evaluation:

```
  Threshold Checks:
    [PASS] p95_latency: 12.30 (limit: 50.00)
    [PASS] p99_latency: 45.20 (limit: 200.00)
    [PASS] error_rate: 0.10 (limit: 1.00)
    [FAIL] regression_count: 8.00 (limit: 5.00)
  Threshold violations detected.
```

Any FAIL causes exit code 1.


## JUnit XML Output

Enable JUnit XML output in the `[output]` section:

```toml
[output]
junit_xml = "results.xml"
```

The generated file follows the standard JUnit XML schema. Each threshold check becomes a test case. Failed thresholds include `<failure>` elements:

```xml
<?xml version="1.0" encoding="UTF-8"?>
<testsuites tests="4" failures="1" time="32.456">
  <testsuite name="pg-retest" tests="4" failures="1">
    <testcase name="p95_latency" time="0.012"/>
    <testcase name="p99_latency" time="0.045"/>
    <testcase name="error_rate" time="0.000"/>
    <testcase name="regression_count" time="0.008">
      <failure message="8 regressions found, max allowed: 5"/>
    </testcase>
  </testsuite>
</testsuites>
```

### GitHub Actions

```yaml
name: Database Regression Test
on: [push]

jobs:
  pg-retest:
    runs-on: ubuntu-latest
    services:
      postgres:
        image: postgres:16
        env:
          POSTGRES_USER: pgretest
          POSTGRES_PASSWORD: pgretest
          POSTGRES_DB: pgretest
        ports:
          - 5432:5432
        options: >-
          --health-cmd "pg_isready -U pgretest"
          --health-interval 10s
          --health-timeout 5s
          --health-retries 5

    steps:
      - uses: actions/checkout@v4

      - name: Install pg-retest
        run: cargo install --path .

      - name: Restore test database
        run: psql -h localhost -U pgretest -d pgretest -f backup.sql
        env:
          PGPASSWORD: pgretest

      - name: Run pipeline
        run: pg-retest run --config .pg-retest.toml

      - name: Upload test results
        if: always()
        uses: actions/upload-artifact@v4
        with:
          name: pg-retest-results
          path: |
            results.xml
            report.json

      - name: Publish JUnit results
        if: always()
        uses: mikepenz/action-junit-report@v4
        with:
          report_paths: results.xml
```

For this workflow, use a config with `[replay].target` instead of `[provision]`:

```toml
[capture]
workload = "workloads/baseline.wkl"

[replay]
target = "host=localhost port=5432 user=pgretest password=pgretest dbname=pgretest"

[thresholds]
p95_max_ms           = 50.0
error_rate_max_pct   = 1.0
regression_max_count = 5

[output]
junit_xml   = "results.xml"
json_report = "report.json"
```

### GitLab CI

```yaml
pg-retest:
  stage: test
  image: rust:latest
  services:
    - name: postgres:16
      alias: postgres
      variables:
        POSTGRES_USER: pgretest
        POSTGRES_PASSWORD: pgretest
        POSTGRES_DB: pgretest

  before_script:
    - cargo install --path .
    - PGPASSWORD=pgretest psql -h postgres -U pgretest -d pgretest -f backup.sql

  script:
    - pg-retest run --config .pg-retest.toml

  artifacts:
    when: always
    reports:
      junit: results.xml
    paths:
      - report.json
```

For GitLab, the target host is `postgres` (the service alias):

```toml
[capture]
workload = "workloads/baseline.wkl"

[replay]
target = "host=postgres port=5432 user=pgretest password=pgretest dbname=pgretest"

[thresholds]
p95_max_ms           = 50.0
error_rate_max_pct   = 1.0
regression_max_count = 5

[output]
junit_xml   = "results.xml"
json_report = "report.json"
```

### Jenkins

Jenkins supports JUnit natively via the `junit` post step:

```groovy
pipeline {
    agent any
    stages {
        stage('Database Regression Test') {
            steps {
                sh 'pg-retest run --config .pg-retest.toml'
            }
        }
    }
    post {
        always {
            junit 'results.xml'
            archiveArtifacts artifacts: 'report.json', allowEmptyArchive: true
        }
    }
}
```


## A/B Variant Mode

A/B variant mode compares replay performance across two or more database targets. This is useful for testing configuration changes (e.g., default settings vs. tuned settings) or comparing PostgreSQL versions.

### Configuration

Define two or more `[[variants]]` in the TOML config:

```toml
[capture]
workload = "workloads/baseline.wkl"

[[variants]]
label  = "pg16-default"
target = "host=db1 port=5432 dbname=app user=app password=secret"

[[variants]]
label  = "pg16-tuned"
target = "host=db2 port=5432 dbname=app user=app password=secret"

[replay]
speed     = 1.0
read_only = true

[output]
json_report = "ab-report.json"
```

### How it works

When two or more `[[variants]]` are present:

1. The pipeline loads (or captures) the workload profile as usual.
2. Normal provisioning is **bypassed entirely**. The `[provision]` section is ignored.
3. The workload is replayed **sequentially** against each variant's `target`.
4. After all replays complete, an A/B comparison report is produced:
   - Per-query regression detection using positional matching.
   - Winner determination based on average latency.
   - Terminal and optional JSON output.

### CLI alternative

A/B testing is also available as a standalone command without a pipeline config:

```bash
pg-retest ab \
  --workload workloads/baseline.wkl \
  --variant "pg16-default=host=db1 dbname=app" \
  --variant "pg16-tuned=host=db2 dbname=app" \
  --read-only \
  --threshold 20.0 \
  --json ab-report.json
```

Two or more `--variant` flags are required. Each takes the form `label=connection_string`.


## Exit Codes

The pipeline uses structured exit codes so CI systems can react to different failure modes:

| Code | Meaning | Description |
|------|---------|-------------|
| 0 | Pass | All stages completed and all thresholds passed (or no thresholds configured). |
| 1 | Threshold violation | Replay succeeded but one or more thresholds failed. |
| 2 | Config error | The TOML config file could not be parsed or validated. |
| 3 | Capture error | The capture stage failed (log file not found, parse error, RDS download error). |
| 4 | Provision error | Docker provisioning failed (Docker unavailable, image pull error, backup restore error). |
| 5 | Replay error | The replay stage failed (connection refused, authentication error, query execution error). |

In CI, you can use these codes to distinguish between "the database is slower" (code 1, a meaningful test result) and "the test infrastructure broke" (codes 2-5, an operational failure).
