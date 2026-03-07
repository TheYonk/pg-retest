# Configuration Reference

This document is the complete reference for pg-retest TOML pipeline configuration files. The configuration file is used with the `pg-retest run` command:

```bash
pg-retest run --config .pg-retest.toml
```

The file is parsed as standard TOML. All sections are documented below with their fields, types, default values, and behavior.


## Required vs Optional Sections

| Section | Required | Notes |
|---------|----------|-------|
| `[capture]` | Yes | Must provide either `workload` or `source_log` (or `rds_instance` with `source_type = "rds"`). |
| `[provision]` | No | Required only if `[replay].target` and `[[variants]]` are both absent. |
| `[replay]` | Yes | Must be present. Requires `target` unless `[provision]` or `[[variants]]` provides the connection. |
| `[thresholds]` | No | Omit to skip pass/fail evaluation (pipeline always exits 0 on successful replay). |
| `[output]` | No | Omit to skip file output (terminal report is always printed). |
| `[[variants]]` | No | When present with 2+ entries, enables A/B variant mode and bypasses `[provision]`. |


## `[capture]`

Controls how the workload profile is obtained. Either load an existing `.wkl` file or capture from a log source.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `workload` | `string` (path) | -- | Path to an existing `.wkl` workload profile file. When set, all other capture fields are ignored and the capture stage is skipped. |
| `source_log` | `string` (path) | -- | Path to a log file to parse. Required when `source_type` is `"pg-csv"` or `"mysql-slow"`. |
| `source_type` | `string` | `"pg-csv"` | Log format to parse. Accepted values: `"pg-csv"`, `"mysql-slow"`, `"rds"`. |
| `source_host` | `string` | `"unknown"` | Metadata field recording the name of the source host. Stored in the workload profile. |
| `pg_version` | `string` | `"unknown"` | Metadata field recording the PostgreSQL version. Stored in the workload profile. |
| `mask_values` | `bool` | `false` | When `true`, all string literals in captured SQL are replaced with `$S` and all numeric literals with `$N`. Use this for PII protection. |
| `rds_instance` | `string` | -- | AWS RDS instance identifier. Required when `source_type = "rds"`. |
| `rds_region` | `string` | `"us-east-1"` | AWS region where the RDS instance is located. |
| `rds_log_file` | `string` | -- | Specific RDS log file name to download. When omitted, the most recent log file is used. |

**Validation**: The config must specify at least one of: `workload`, `source_log`, or `rds_instance` (with `source_type = "rds"`). If none are present, the pipeline exits with code 2 (config error).

### Source type details

- **`pg-csv`**: Parses PostgreSQL CSV log format. Requires `source_log`.
- **`mysql-slow`**: Parses MySQL slow query log format. SQL transforms (backtick removal, LIMIT rewrite, IFNULL to COALESCE, IF to CASE WHEN, UNIX_TIMESTAMP to EXTRACT) are applied automatically. MySQL-specific commands (`SHOW`, `SET NAMES`, `USE`) are skipped. Requires `source_log`.
- **`rds`**: Downloads log files from AWS RDS/Aurora via the `aws` CLI (must be installed and configured). Uses paginated download for large files. Requires `rds_instance`.


## `[provision]`

Controls how the target database is created for replay. This section is optional; it can be replaced by setting `[replay].target` directly.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `backend` | `string` | -- | Provisioning backend. Currently only `"docker"` is supported. Required if this section is present and `connection_string` is not set. |
| `image` | `string` | `"postgres:16"` | Docker image to use for the container. |
| `restore_from` | `string` (path) | -- | Path to a SQL file to restore into the container after startup. The file is copied into the container via `docker cp` and executed with `psql`. |
| `port` | `integer` | random | Host port to expose. When set to `0` or omitted, Docker assigns a random available port. |
| `connection_string` | `string` | -- | Pre-existing PostgreSQL connection string. When set, Docker provisioning is skipped entirely and this connection is used directly. |

**Docker lifecycle**: When `backend = "docker"`, the pipeline creates a container named `pg-retest-<pid>` with user `pgretest`, password `pgretest`, database `pgretest`. After the pipeline completes, the container is removed with `docker rm -f`.

### Precedence

The pipeline determines the target connection string using this priority:

1. `[[variants]]` (if 2+ defined) -- A/B mode, each variant has its own target.
2. `[replay].target` -- direct connection string, no provisioning.
3. `[provision].connection_string` -- pre-existing server, no Docker.
4. `[provision].backend = "docker"` -- full Docker lifecycle.


## `[replay]`

Controls how the workload is replayed against the target database.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `target` | `string` | -- | PostgreSQL connection string (e.g., `"host=localhost port=5432 user=app dbname=app"`). When set, the `[provision]` section is skipped. |
| `speed` | `float` | `1.0` | Replay speed multiplier applied to inter-query delays. `2.0` replays at double speed, `0.5` at half speed. |
| `read_only` | `bool` | `false` | When `true`, only SELECT queries are replayed. All DML (INSERT, UPDATE, DELETE) is stripped. |
| `scale` | `integer` | `1` | Uniform scale factor. Duplicates every session N times for load testing. Has no effect when set to 1. |
| `stagger_ms` | `integer` | `0` | Delay in milliseconds between each scaled copy of a session. Applies to both uniform scaling and per-category scaling. |
| `scale_analytical` | `integer` | -- | Scale factor for sessions classified as Analytical. |
| `scale_transactional` | `integer` | -- | Scale factor for sessions classified as Transactional. |
| `scale_mixed` | `integer` | -- | Scale factor for sessions classified as Mixed. |
| `scale_bulk` | `integer` | -- | Scale factor for sessions classified as Bulk. |

### Scaling modes

There are two mutually exclusive scaling modes:

**Uniform scaling** (`scale`): Every session is duplicated the same number of times regardless of its workload classification.

```toml
[replay]
scale      = 4
stagger_ms = 500
```

**Per-category scaling** (`scale_analytical`, `scale_transactional`, `scale_mixed`, `scale_bulk`): Sessions are classified first, then each category is scaled independently. If any per-category field is set, per-category mode takes priority over uniform `scale`. Categories not explicitly set default to `1` (no scaling).

```toml
[replay]
scale_analytical    = 2
scale_transactional = 4
scale_mixed         = 1
scale_bulk          = 0    # 0 = exclude this category entirely
stagger_ms          = 500
```

Setting a category to `0` excludes all sessions of that class from replay.

**Safety warning**: Scaling write workloads causes DML to execute multiple times, changing data state. The pipeline prints a warning when this is detected.


## `[thresholds]`

Defines pass/fail criteria for the pipeline. If this section is omitted, the pipeline always exits 0 when replay succeeds. If present, any threshold violation causes exit code 1.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `p95_max_ms` | `float` | -- | Maximum acceptable P95 replay latency in milliseconds. The pipeline fails if the actual P95 exceeds this value. |
| `p99_max_ms` | `float` | -- | Maximum acceptable P99 replay latency in milliseconds. |
| `error_rate_max_pct` | `float` | -- | Maximum acceptable error rate as a percentage (0-100) of total queries replayed. |
| `regression_max_count` | `integer` | -- | Maximum number of individual queries allowed to regress. |
| `regression_threshold_pct` | `float` | `20.0` | Defines what constitutes a "regression". A query is flagged as regressed if its replay latency is more than this percentage higher than its source latency. This value is also used by the compare stage to identify regressions in the report. |

All threshold fields are optional. Only configured thresholds are evaluated. For example, if you only set `p95_max_ms` and `error_rate_max_pct`, only those two checks run.


## `[output]`

Controls what report files are written after the pipeline completes.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `json_report` | `string` (path) | -- | Path to write a JSON comparison report. Contains per-query latency data, regressions, and summary statistics. |
| `junit_xml` | `string` (path) | -- | Path to write a JUnit XML test report. Each threshold check becomes a `<testcase>` element. Compatible with GitHub Actions, GitLab CI, Jenkins, and other CI systems. |

If this section is omitted or both fields are absent, no files are written. The terminal report is always printed regardless.


## `[[variants]]`

Defines A/B variant targets for comparative testing. This is a TOML array of tables -- each entry defines one variant.

| Field | Type | Default | Description |
|-------|------|---------|-------------|
| `label` | `string` | -- | Human-readable label for this variant (e.g., `"pg16-default"`, `"pg16-tuned"`). Used in report output. Required. |
| `target` | `string` | -- | PostgreSQL connection string for this variant. Required. |

**Requirements**: At least 2 variants must be defined for A/B mode to activate. If only 1 variant is defined, the pipeline falls back to normal (non-A/B) mode.

**Behavior**: When A/B mode is active:

- The `[provision]` section is ignored entirely.
- The workload is replayed sequentially against each variant's target.
- An A/B comparison report is produced with per-query regression detection and winner determination based on average latency.
- JSON output is written if `[output].json_report` is configured.
- JUnit XML output is not produced in A/B mode.


## Minimal Configuration Example

The smallest valid configuration requires a workload source and a target:

```toml
[capture]
workload = "workloads/baseline.wkl"

[replay]
target = "host=localhost port=5432 user=app dbname=app"
```

This loads an existing workload file, replays it at 1x speed against the specified database, prints a terminal comparison report, and exits 0.


## Full Configuration Example

```toml
[capture]
source_log  = "pg_log.csv"
source_type = "pg-csv"
source_host = "prod-db-01"
pg_version  = "16.2"
mask_values = true

[provision]
backend      = "docker"
image        = "postgres:16.2"
restore_from = "backup.sql"
port         = 5442

[replay]
speed     = 1.0
read_only = false
scale     = 1

[thresholds]
p95_max_ms             = 50.0
p99_max_ms             = 200.0
error_rate_max_pct     = 1.0
regression_max_count   = 5
regression_threshold_pct = 20.0

[output]
json_report = "report.json"
junit_xml   = "results.xml"
```


## Per-Category Scaling Example

```toml
[capture]
workload = "workloads/baseline.wkl"

[replay]
target              = "host=localhost dbname=loadtest"
scale_analytical    = 2
scale_transactional = 8
scale_mixed         = 1
scale_bulk          = 0
stagger_ms          = 250

[thresholds]
p95_max_ms         = 100.0
error_rate_max_pct = 5.0

[output]
json_report = "capacity-report.json"
```


## A/B Variant Example

```toml
[capture]
workload = "workloads/baseline.wkl"

[[variants]]
label  = "pg16-default"
target = "host=db1 port=5432 user=app dbname=app"

[[variants]]
label  = "pg16-tuned"
target = "host=db2 port=5432 user=app dbname=app"

[replay]
speed     = 1.0
read_only = true

[thresholds]
regression_threshold_pct = 15.0

[output]
json_report = "ab-report.json"
```


## MySQL Capture Example

```toml
[capture]
source_log  = "mysql-slow.log"
source_type = "mysql-slow"
source_host = "mysql-prod-01"
mask_values = true

[replay]
target    = "host=localhost port=5432 user=app dbname=migrated_app"
read_only = true

[thresholds]
p95_max_ms         = 100.0
error_rate_max_pct = 5.0

[output]
json_report = "migration-report.json"
junit_xml   = "migration-results.xml"
```


## RDS Capture Example

```toml
[capture]
source_type  = "rds"
rds_instance = "prod-db-instance"
rds_region   = "us-west-2"
source_host  = "prod-rds"
mask_values  = true

[replay]
target = "host=staging-db.internal port=5432 user=app dbname=app"

[thresholds]
p95_max_ms           = 75.0
regression_max_count = 3

[output]
json_report = "rds-report.json"
junit_xml   = "rds-results.xml"
```
