# pg-retest

![Rust](https://img.shields.io/badge/language-Rust-orange?logo=rust)
![Version](https://img.shields.io/badge/version-0.2.0-green)
![PostgreSQL](https://img.shields.io/badge/PostgreSQL-12%2B-336791?logo=postgresql&logoColor=white)
![Tests](https://img.shields.io/badge/tests-216-brightgreen)
![Clippy](https://img.shields.io/badge/clippy-zero%20warnings-brightgreen)
![License](https://img.shields.io/badge/license-Apache%202.0-blue)

Capture, replay, and compare PostgreSQL workloads. Validate configuration changes, server migrations, capacity planning, and cross-database migrations with confidence.

```mermaid
graph LR
    A[Capture] -->|.wkl profile| B[Replay]
    B -->|results.wkl| C[Compare]
    C --> D{Pass/Fail}
    D -->|Pass| E[Deploy]
    D -->|Fail| F[Investigate]

    style A fill:#4a9eff,stroke:#333,color:#fff
    style B fill:#4a9eff,stroke:#333,color:#fff
    style C fill:#4a9eff,stroke:#333,color:#fff
    style D fill:#ffa726,stroke:#333,color:#fff
    style E fill:#66bb6a,stroke:#333,color:#fff
    style F fill:#ef5350,stroke:#333,color:#fff
```

---

## Quick Start

### Prerequisites

- **Rust** toolchain (1.70+): [Install via rustup](https://rustup.rs/)
- **PostgreSQL** target database for replay
- *Optional:* `aws` CLI (for RDS capture), Docker (for CI/CD provisioning)

### Install & Build

```bash
git clone https://github.com/your-org/pg-retest.git
cd pg-retest
cargo build --release

# Binary is at ./target/release/pg-retest
# Optionally copy to your PATH:
cp target/release/pg-retest /usr/local/bin/
```

### 3-Step Workflow

```bash
# Step 1: Capture workload from PostgreSQL CSV logs
pg-retest capture \
  --source-log /path/to/postgresql.csv \
  --output workload.wkl

# Step 2: Replay against a target database
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=mydb user=postgres" \
  --output results.wkl

# Step 3: Compare and get a report
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --json report.json \
  --fail-on-regression
```

### Other Quick Commands

```bash
# Inspect a workload profile
pg-retest inspect workload.wkl

# Inspect with classification breakdown
pg-retest inspect workload.wkl --classify

# Launch the web dashboard
pg-retest web --port 8080

# Run a full CI/CD pipeline from config
pg-retest run --config .pg-retest.toml

# AI-assisted tuning (dry-run by default)
pg-retest tune \
  --workload workload.wkl \
  --target "host=localhost dbname=mydb user=postgres" \
  --provider claude
```

---

## Architecture

```mermaid
flowchart TB
    subgraph Capture["Capture Sources"]
        PG[PG CSV Logs]
        PROXY[Wire Protocol Proxy]
        MYSQL[MySQL Slow Log]
        RDS[AWS RDS/Aurora]
    end

    subgraph Core["Core Engine"]
        WKL[(workload.wkl<br/>MessagePack v2)]
        REPLAY[Replay Engine<br/>Tokio async]
        CLASSIFY[Classifier<br/>Analytical/Transactional<br/>Mixed/Bulk]
    end

    subgraph Analysis["Analysis & Reports"]
        COMPARE[Compare]
        CAPACITY[Capacity Report]
        AB[A/B Variant]
        JUNIT[JUnit XML]
        THRESH[Threshold Check]
    end

    subgraph AI["AI-Powered"]
        TRANSFORM[Workload Transform<br/>Analyze → Plan → Apply]
        TUNER[Tuning Advisor<br/>Config/Index/Query/Schema]
    end

    subgraph Providers["LLM Providers"]
        CLAUDE[Claude]
        OPENAI[OpenAI]
        GEMINI[Gemini]
        BEDROCK[Bedrock]
        OLLAMA[Ollama]
    end

    PG --> WKL
    PROXY --> WKL
    MYSQL --> WKL
    RDS --> WKL
    WKL --> REPLAY
    WKL --> CLASSIFY
    REPLAY --> COMPARE
    REPLAY --> CAPACITY
    REPLAY --> AB
    COMPARE --> THRESH
    COMPARE --> JUNIT
    THRESH --> PASS{Pass/Fail}

    WKL --> TRANSFORM
    TRANSFORM --> WKL
    TUNER --> REPLAY

    TRANSFORM --> Providers
    TUNER --> Providers

    style PG fill:#336791,stroke:#333,color:#fff
    style PROXY fill:#336791,stroke:#333,color:#fff
    style MYSQL fill:#4479a1,stroke:#333,color:#fff
    style RDS fill:#ff9900,stroke:#333,color:#fff
    style WKL fill:#2e7d32,stroke:#333,color:#fff
    style REPLAY fill:#1565c0,stroke:#333,color:#fff
    style TUNER fill:#7b1fa2,stroke:#333,color:#fff
    style TRANSFORM fill:#7b1fa2,stroke:#333,color:#fff
```

### Data Flow

```mermaid
sequenceDiagram
    participant Source as Source DB
    participant Capture as pg-retest capture
    participant Profile as workload.wkl
    participant Replay as pg-retest replay
    participant Target as Target DB
    participant Compare as pg-retest compare
    participant Report as Report (JSON/JUnit)

    Source->>Capture: SQL logs / proxy traffic
    Capture->>Profile: Serialize (MessagePack)
    Profile->>Replay: Load workload
    Replay->>Target: Execute queries<br/>(parallel sessions)
    Target->>Replay: Results + timing
    Replay->>Compare: results.wkl
    Profile->>Compare: Original timing
    Compare->>Report: Latency diff, regressions,<br/>error rate, pass/fail
```

---

## Features

| Feature | Description |
|---------|-------------|
| **Multi-source capture** | PG CSV logs, wire protocol proxy, MySQL slow logs, AWS RDS/Aurora |
| **Transaction-aware replay** | BEGIN/COMMIT/ROLLBACK boundaries, auto-rollback on failure |
| **Read-only mode** | Strip DML for safe replay against production replicas |
| **Speed control** | Compress or stretch inter-query timing (0.1x to 10x) |
| **Scaled benchmark** | Duplicate sessions Nx with staggered offsets |
| **Per-category scaling** | Scale Analytical, Transactional, Mixed, Bulk independently |
| **PII masking** | Strip string/numeric literals from SQL (`--mask-values`) |
| **Comparison reports** | Per-query latency regression detection with thresholds |
| **A/B variant testing** | Compare performance across database targets |
| **CI/CD pipeline** | TOML config, Docker provisioning, JUnit XML, exit codes |
| **Cross-DB capture** | MySQL slow log capture with automatic SQL transform |
| **Web dashboard** | 11-page SPA with real-time WebSocket updates |
| **AI workload transform** | Reshape workloads with AI (scale, inject, remove queries) |
| **AI-assisted tuning** | LLM-powered config, index, query, and schema recommendations |
| **Capacity planning** | Throughput QPS, latency percentiles, error rates at scale |

---

## Subcommands

| Command | Description |
|---------|-------------|
| `capture` | Capture workload from PostgreSQL logs, MySQL logs, or RDS |
| `replay` | Replay a captured workload against a target database |
| `compare` | Compare source workload with replay results |
| `inspect` | Inspect a workload profile (optionally with classification) |
| `proxy` | Run a capture proxy between clients and PostgreSQL |
| `run` | Run full CI/CD pipeline from TOML config |
| `ab` | A/B test across different database targets |
| `web` | Launch the web dashboard |
| `transform` | AI-powered workload transformation (analyze/plan/apply) |
| `tune` | AI-assisted database tuning |

---

## Capture Methods

pg-retest supports four capture backends. All produce the same `.wkl` workload profile format.

```mermaid
graph TD
    subgraph Sources
        A[PG CSV Log<br/><code>--source-type pg-csv</code>]
        B[Wire Protocol Proxy<br/><code>pg-retest proxy</code>]
        C[MySQL Slow Log<br/><code>--source-type mysql-slow</code>]
        D[AWS RDS/Aurora<br/><code>--source-type rds</code>]
    end

    A --> E[(workload.wkl)]
    B --> E
    C --> E
    D --> E

    E --> F[Replay / Compare / Transform / Tune]

    style E fill:#2e7d32,stroke:#333,color:#fff
```

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

<details>
<summary><strong>PostgreSQL Logging Setup</strong></summary>

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

</details>

### Proxy Capture

A PostgreSQL wire protocol proxy that sits between your application and database, capturing all SQL traffic with zero application changes.

```bash
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --output workload.wkl \
  --pool-size 100 \
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

---

## Replay

The replay engine reads a workload profile and replays it against a target PostgreSQL instance, preserving connection parallelism and inter-query timing.

```bash
pg-retest replay --workload workload.wkl --target "host=localhost dbname=mydb user=postgres"
```

### Replay Modes

| Mode | Flag | Description |
|------|------|-------------|
| Read-write | *(default)* | Replays all queries including DML. Use a backup/snapshot. |
| Read-only | `--read-only` | Strips DML/DDL/transactions. Safe for production replicas. |
| Scaled | `--scale N` | Duplicates sessions Nx with `--stagger-ms` offset. |
| Per-category | `--scale-analytical 2` | Scale workload categories independently. |
| Speed-adjusted | `--speed 2.0` | Compress (>1) or stretch (<1) inter-query timing. |

### Transaction-Aware Replay

pg-retest tracks transaction boundaries (BEGIN/COMMIT/ROLLBACK) during capture and provides transaction-aware replay:

- Queries within a transaction share a `transaction_id`
- If a query inside a transaction fails, the replay engine automatically issues a ROLLBACK and skips remaining queries in that transaction
- COMMIT for a failed transaction is converted to a no-op

---

## Compare

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

---

## A/B Variant Testing

Compare replay performance across two or more database targets -- useful for evaluating configuration changes, version upgrades, or hardware differences.

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

---

## CI/CD Pipeline

Automate the full capture-provision-replay-compare cycle with a single TOML config file:

```bash
pg-retest run --config .pg-retest.toml
```

```mermaid
flowchart LR
    A[Capture] --> B[Provision<br/>Docker]
    B --> C[Replay]
    C --> D[Compare]
    D --> E[Threshold<br/>Check]
    E --> F[Report<br/>JSON + JUnit]

    style A fill:#4a9eff,stroke:#333,color:#fff
    style B fill:#4a9eff,stroke:#333,color:#fff
    style C fill:#4a9eff,stroke:#333,color:#fff
    style D fill:#4a9eff,stroke:#333,color:#fff
    style E fill:#ffa726,stroke:#333,color:#fff
    style F fill:#66bb6a,stroke:#333,color:#fff
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

| Section | Purpose |
|---------|---------|
| `[capture]` | Specify `workload` path (skip capture) or `source_log` to capture. Supports `source_type`: `pg-csv`, `mysql-slow`, `rds`. |
| `[provision]` | Docker provisioning with optional `restore_from`. Or use `connection_string` for a pre-existing target. |
| `[replay]` | Speed, read-only mode, scaling options. If no `[provision]`, specify `target` here. |
| `[thresholds]` | Pass/fail criteria: `p95_max_ms`, `p99_max_ms`, `error_rate_max_pct`, `regression_max_count`. |
| `[output]` | `json_report` for JSON output, `junit_xml` for CI integration. |
| `[[variants]]` | A/B testing mode: bypasses provisioning, replays against each variant target. |

### Pipeline Exit Codes

| Code | Meaning |
|------|---------|
| 0 | PASS |
| 1 | Threshold violation |
| 2 | Config error |
| 3 | Capture error |
| 4 | Provision error |
| 5 | Replay error |

<details>
<summary><strong>Minimal Pipeline Config</strong></summary>

If you already have a captured workload and a running target database:

```toml
[capture]
workload = "existing.wkl"

[replay]
target = "host=localhost dbname=test user=postgres"
```

</details>

<details>
<summary><strong>MySQL Pipeline Config</strong></summary>

```toml
[capture]
source_log = "mysql_slow.log"
source_type = "mysql-slow"
source_host = "mysql-prod"

[replay]
target = "host=localhost dbname=test user=postgres"
read_only = true
```

</details>

<details>
<summary><strong>A/B Variant Pipeline Config</strong></summary>

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

</details>

<details>
<summary><strong>Per-Category Scaling Pipeline Config</strong></summary>

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

</details>

---

## AI-Powered Workload Transform

Reshape workloads using AI-generated transform plans. The 3-layer architecture keeps AI in an advisory role while execution is fully deterministic.

```mermaid
flowchart LR
    A[workload.wkl] --> B[Analyze<br/>Deterministic]
    B --> C[Plan<br/>AI-Powered]
    C --> D[Apply<br/>Deterministic]
    D --> E[transformed.wkl]

    style B fill:#1565c0,stroke:#333,color:#fff
    style C fill:#7b1fa2,stroke:#333,color:#fff
    style D fill:#1565c0,stroke:#333,color:#fff
```

```bash
# Step 1: Analyze workload (no AI needed)
pg-retest transform analyze --workload workload.wkl --json

# Step 2: Generate a transform plan via AI
pg-retest transform plan \
  --workload workload.wkl \
  --prompt "Simulate 3x Black Friday traffic on the orders table group" \
  --provider claude \
  --output transform-plan.toml

# Step 3: Apply the plan (deterministic, reproducible)
pg-retest transform apply \
  --workload workload.wkl \
  --plan transform-plan.toml \
  --output transformed.wkl
```

Transform operations: **Scale** (duplicate sessions by weight), **Inject** (add new queries), **InjectSession** (add new session), **Remove** (drop query groups).

---

## AI-Assisted Tuning

Iterative tuning loop: collect database context, get LLM recommendations, validate safety, apply, replay, compare, auto-rollback on regression.

```mermaid
flowchart TD
    A[Collect PG Context<br/>pg_settings, schema,<br/>pg_stat_statements, EXPLAIN] --> B[LLM Advisor]
    B --> C[Safety Validation<br/>Allowlist check]
    C --> D{Safe?}
    D -->|Yes| E[Apply Changes]
    D -->|No| B
    E --> F[Replay Workload]
    F --> G[Compare vs Baseline]
    G --> H{p95 regressed >5%?}
    H -->|Yes| I[Auto-Rollback]
    H -->|No| J{More iterations?}
    I --> J
    J -->|Yes| A
    J -->|No| K[Tuning Report]

    style A fill:#1565c0,stroke:#333,color:#fff
    style B fill:#7b1fa2,stroke:#333,color:#fff
    style C fill:#ffa726,stroke:#333,color:#fff
    style E fill:#66bb6a,stroke:#333,color:#fff
    style I fill:#ef5350,stroke:#333,color:#fff
    style K fill:#2e7d32,stroke:#333,color:#fff
```

```bash
# Dry-run (default): see recommendations without applying
pg-retest tune \
  --workload workload.wkl \
  --target "host=localhost dbname=mydb user=postgres" \
  --provider claude \
  --max-iterations 3 \
  --hint "optimize for OLTP latency"

# Apply recommendations (use --apply)
pg-retest tune \
  --workload workload.wkl \
  --target "host=localhost dbname=mydb user=postgres" \
  --provider openai \
  --apply \
  --json tuning-report.json
```

**Recommendation types:** Config changes (`ALTER SYSTEM`), Index creation, Query rewrites, Schema changes.

**Safety:** Production hostname check blocks targets containing "prod", "production", "primary", "master", "main" (bypass with `--force`). Allowlist permits ~41 safe PG parameters; dangerous params are blocked.

**LLM Providers:** Claude, OpenAI, Gemini, Bedrock, Ollama. Set API keys via `--api-key` flag or environment variables (`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`). Bedrock uses standard AWS credentials.

---

## Web Dashboard

A browser-based dashboard for managing the full workflow without the command line.

```bash
pg-retest web --port 8080 --data-dir ./data
# Open http://localhost:8080
```

**11 pages:** Dashboard, Workloads, Proxy, Replay, A/B Testing, Compare, Pipeline, History, Transform, Tuning, Help.

- Real-time updates via WebSocket (proxy traffic, replay progress, tuning iterations)
- SQLite metadata storage with `.wkl` files as source of truth on disk
- Frontend: Alpine.js + Chart.js + Tailwind CSS via CDN (no build step)
- Tuning page shows history with expandable recommendations and comparison stats

---

## Workload Classification

```bash
pg-retest inspect workload.wkl --classify
```

| Class | Criteria |
|-------|---------|
| **Analytical** | >80% reads, avg latency >10ms (OLAP pattern) |
| **Transactional** | >20% writes, avg latency <5ms, >2 transactions (OLTP pattern) |
| **Bulk** | >80% writes, <=2 transactions (data loading) |
| **Mixed** | Everything else |

Classification drives per-category scaling behavior.

---

## PII Masking

The `--mask-values` flag (available on `capture` and `proxy` commands) replaces string literals with `$S` and numeric literals with `$N`:

```sql
-- Original:
SELECT * FROM users WHERE email = 'alice@corp.com' AND id = 42

-- Masked:
SELECT * FROM users WHERE email = $S AND id = $N
```

Masking uses a hand-written character-level state machine (not regex) to correctly handle SQL edge cases: escaped quotes (`''`), dollar-quoted strings (`$$...$$`), and numbers inside identifiers.

---

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

---

## Building & Testing

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Run all tests (216 tests)
cargo test

# Run a single test file
cargo test --test profile_io_test

# Run a single test function
cargo test --test compare_test test_comparison_regressions

# Lint (zero warnings)
cargo clippy

# Format
cargo fmt
```

---

## Documentation

Design documents and implementation plans are available in the `docs/` directory:

- `docs/plans/2026-03-03-pg-retest-m1-design.md` -- M1 Capture & Replay design
- `docs/plans/2026-03-04-m3-cicd-design.md` -- M3 CI/CD integration design
- `docs/plans/2026-03-04-m4-mysql-capture-design.md` -- M4 MySQL capture design
- `docs/plans/2026-03-04-proxy-gateway-design.md` -- Proxy capture design
- `docs/plans/2026-03-05-gap-closure-design.md` -- Per-category scaling, A/B testing, RDS capture
- `docs/plans/2026-03-07-workload-transform-design.md` -- AI workload transform design
- `docs/plans/2026-03-07-m5-ai-tuning-design-v2.md` -- M5 AI-Assisted Tuning design
