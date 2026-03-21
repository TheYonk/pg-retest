# Getting Started with pg-retest

This guide walks you through installing pg-retest, capturing your first workload, replaying it against a target database, and comparing the results. By the end, you will have a working capture-replay-compare cycle and know how to use the web dashboard.

## Prerequisites

Before you begin, ensure you have the following installed:

**Rust toolchain (1.70+)**

Install via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

**PostgreSQL (14+)**

pg-retest captures and replays workloads against PostgreSQL. You need at least one accessible PG instance. For this guide, we use a Docker container.

**Docker (optional, for provisioning)**

Docker is required for the automated pipeline provisioner, which creates throwaway PG containers for replay. It is also the easiest way to get a local PG instance running.

## Installation

Clone the repository and build the release binary:

```bash
git clone <repository-url> pg-retest
cd pg-retest
cargo build --release
```

The binary is at `target/release/pg-retest`. You can add it to your PATH or invoke it directly:

```bash
# Option A: add to PATH
export PATH="$PWD/target/release:$PATH"

# Option B: invoke directly
./target/release/pg-retest --help
```

Verify the installation:

```bash
pg-retest --help
```

You should see the list of subcommands: `capture`, `replay`, `compare`, `inspect`, `proxy`, `run`, `ab`, `web`, `transform`, `tune`, and `proxy-ctl`.

## Start a Test Database

For this walkthrough, we use a PostgreSQL container on port 5441 with a sample database:

```bash
docker run -d \
  --name pg-retest-demo \
  -e POSTGRES_USER=sales_demo_app \
  -e POSTGRES_PASSWORD=salesdemo123 \
  -e POSTGRES_DB=sales_demo \
  -p 5441:5432 \
  postgres:16
```

Wait a few seconds for the container to start, then verify connectivity:

```bash
psql "host=localhost port=5441 user=sales_demo_app password=salesdemo123 dbname=sales_demo" \
  -c "SELECT version();"
```

Create some sample tables and data to work with:

```bash
psql "host=localhost port=5441 user=sales_demo_app password=salesdemo123 dbname=sales_demo" <<'SQL'
CREATE TABLE IF NOT EXISTS customers (
    id SERIAL PRIMARY KEY,
    name TEXT NOT NULL,
    email TEXT,
    created_at TIMESTAMPTZ DEFAULT NOW()
);

CREATE TABLE IF NOT EXISTS orders (
    id SERIAL PRIMARY KEY,
    customer_id INT REFERENCES customers(id),
    total NUMERIC(10,2),
    status TEXT DEFAULT 'pending',
    created_at TIMESTAMPTZ DEFAULT NOW()
);

INSERT INTO customers (name, email)
SELECT
    'Customer ' || i,
    'customer' || i || '@example.com'
FROM generate_series(1, 100) AS i;

INSERT INTO orders (customer_id, total, status)
SELECT
    (random() * 99 + 1)::int,
    (random() * 500)::numeric(10,2),
    CASE WHEN random() < 0.7 THEN 'completed' ELSE 'pending' END
FROM generate_series(1, 1000);
SQL
```

## Step 1: Configure PostgreSQL Logging

pg-retest's CSV log capture reads PostgreSQL's CSV-format log files. You need to enable CSV logging with statement durations.

Add these settings to your `postgresql.conf` (or set them via `ALTER SYSTEM`):

```ini
# Enable the logging collector and CSV output
logging_collector = on
log_destination = 'csvlog'

# Log all statements with their durations
log_min_duration_statement = 0
log_statement = 'none'

# Ensure the log directory exists
log_directory = 'pg_log'
log_filename = 'postgresql-%Y-%m-%d.csv'

# Include session ID in logs (essential for per-session grouping)
log_line_prefix = '%m [%p] %q%u@%d '
```

**Important:** Changing `logging_collector` requires a full PostgreSQL restart. The other settings can be applied with a reload (`SELECT pg_reload_conf();` or `pg_ctl reload`).

For the Docker container in this guide, you can set these at startup:

```bash
docker stop pg-retest-demo && docker rm pg-retest-demo

docker run -d \
  --name pg-retest-demo \
  -e POSTGRES_USER=sales_demo_app \
  -e POSTGRES_PASSWORD=salesdemo123 \
  -e POSTGRES_DB=sales_demo \
  -p 5441:5432 \
  postgres:16 \
  -c logging_collector=on \
  -c log_destination=csvlog \
  -c log_min_duration_statement=0 \
  -c log_directory=pg_log \
  -c "log_filename=postgresql-%Y-%m-%d.csv"
```

Re-create your sample tables and data after restarting the container (use the same SQL from above).

## Step 2: Generate Some Workload

Run a few queries against the database to generate log entries:

```bash
psql "host=localhost port=5441 user=sales_demo_app password=salesdemo123 dbname=sales_demo" <<'SQL'
SELECT count(*) FROM customers;
SELECT c.name, count(o.id) AS order_count, sum(o.total) AS total_spent
  FROM customers c
  JOIN orders o ON o.customer_id = c.id
  GROUP BY c.name
  ORDER BY total_spent DESC
  LIMIT 10;
SELECT * FROM orders WHERE status = 'pending' LIMIT 20;
UPDATE orders SET status = 'completed' WHERE id = 1;
BEGIN;
  INSERT INTO customers (name, email) VALUES ('New Customer', 'new@example.com');
  INSERT INTO orders (customer_id, total) VALUES (currval('customers_id_seq'), 99.99);
COMMIT;
SQL
```

## Step 3: Capture the Workload

Copy the CSV log file out of the Docker container:

```bash
# Find the log file name
docker exec pg-retest-demo ls /var/lib/postgresql/data/pg_log/

# Copy it locally (adjust the filename to match)
docker cp pg-retest-demo:/var/lib/postgresql/data/pg_log/postgresql-2026-03-06.csv ./demo.csv
```

Now capture it into a workload profile:

```bash
pg-retest capture \
  --source-log ./demo.csv \
  --source-type pg-csv \
  --source-host localhost:5441 \
  --pg-version 16 \
  --output demo-workload.wkl
```

You should see output like:

```
Captured 7 queries across 1 sessions
Wrote workload profile to demo-workload.wkl
```

## Step 4: Inspect the Workload

Before replaying, inspect the captured workload to verify it looks correct:

```bash
pg-retest inspect demo-workload.wkl
```

This prints the workload profile as JSON, showing sessions, queries, timing offsets, and transaction boundaries.

To also see the workload classification breakdown (Analytical, Transactional, Mixed, Bulk):

```bash
pg-retest inspect demo-workload.wkl --classify
```

## Step 5: Replay the Workload

Replay the captured workload against the same database (or a different target):

```bash
pg-retest replay \
  --workload demo-workload.wkl \
  --target "host=localhost port=5441 user=sales_demo_app password=salesdemo123 dbname=sales_demo" \
  --output demo-results.wkl
```

The replay engine creates one async task per captured session, preserving the original connection parallelism and inter-query timing. You should see:

```
Replaying 1 sessions (7 queries) against host=localhost port=5441 ...
Mode: ReadWrite, Speed: 1x
Replay complete: 7 queries replayed, 0 errors
Results written to demo-results.wkl
```

**Read-only mode:** To replay only SELECT queries (stripping all DML), add `--read-only`:

```bash
pg-retest replay \
  --workload demo-workload.wkl \
  --target "host=localhost port=5441 user=sales_demo_app password=salesdemo123 dbname=sales_demo" \
  --output demo-results.wkl \
  --read-only
```

**Speed multiplier:** To replay at 2x speed (halving all inter-query delays):

```bash
pg-retest replay \
  --workload demo-workload.wkl \
  --target "..." \
  --output demo-results.wkl \
  --speed 2.0
```

## Step 6: Compare Source vs. Replay

Compare the original workload timings against the replay results:

```bash
pg-retest compare \
  --source demo-workload.wkl \
  --replay demo-results.wkl \
  --threshold 20.0
```

This produces a terminal report showing per-query latency comparison, regressions (queries that got slower by more than the threshold percentage), improvements, and error counts.

**JSON report:** Add `--json` to save a machine-readable report:

```bash
pg-retest compare \
  --source demo-workload.wkl \
  --replay demo-results.wkl \
  --threshold 20.0 \
  --json demo-report.json
```

**Exit codes for CI:** Use `--fail-on-regression` and/or `--fail-on-error` to make pg-retest exit non-zero when problems are detected:

```bash
pg-retest compare \
  --source demo-workload.wkl \
  --replay demo-results.wkl \
  --threshold 20.0 \
  --fail-on-regression \
  --fail-on-error
```

### Understanding the Comparison Output

The comparison report includes:

- **Per-query latency:** Source duration vs. replay duration, with percentage change.
- **Regressions:** Queries where the replay was slower than the source by more than the threshold (default 20%). These indicate potential performance problems on the target.
- **Improvements:** Queries that got faster.
- **Errors:** Queries that failed during replay (syntax errors, missing tables, permission issues).
- **Summary:** Total queries compared, regression count, improvement count, error count.

A typical output looks like:

```
=== Workload Comparison Report ===
  Total queries compared: 7
  Regressions (>20.0%):  1
  Improvements:           3
  Errors:                 0
  Result: PASS
```

## Step 7: Using the Web Dashboard

pg-retest includes a built-in web dashboard for managing workloads, running replays, and viewing reports through a browser.

Start the dashboard:

```bash
pg-retest web --port 8080
```

Open `http://localhost:8080` in your browser. The dashboard provides 11 pages:

- **Dashboard** -- Overview of workloads, recent runs, and system status.
- **Workloads** -- Upload, import, inspect, classify, and delete workload profiles.
- **Proxy** -- Start and stop the capture proxy with live traffic monitoring via WebSocket.
- **Replay** -- Configure and launch replays with real-time progress updates.
- **A/B Test** -- Compare replay performance across different database targets.
- **Compare** -- View detailed comparison reports with per-query breakdowns.
- **Pipeline** -- Configure and run full CI/CD pipelines.
- **History** -- Browse historical runs with filtering and trend analysis.
- **Transform** -- AI-powered workload transformation (analyze, plan, apply).
- **Tuning** -- AI-assisted database tuning with history and recommendations.
- **Help** -- In-app documentation and reference.

### Quick workflow via the dashboard

1. Navigate to **Workloads** and click **Upload** to import your `.wkl` file.
2. Click **Inspect** on the uploaded workload to review its contents.
3. Go to **Replay**, select the workload, enter the target connection string, and click **Start Replay**.
4. Watch the progress bar update in real time via WebSocket.
5. When complete, go to **Compare** to view the source vs. replay report.

The web dashboard stores metadata in a SQLite database under the data directory (default `./data/`). Workload `.wkl` files are stored on disk in `data/workloads/`. Changes to the embedded frontend require recompilation.

### Custom data directory

```bash
pg-retest web --port 8080 --data-dir /path/to/my/data
```

## Next Steps

Now that you have the basic capture-replay-compare cycle working, explore these topics:

- **[Capture Methods](capture.md)** -- All four capture backends (CSV log, proxy, MySQL slow log, RDS/Aurora) and PII masking.
- **Scaled benchmarks** -- Use `--scale N` or per-category scaling (`--scale-analytical 2 --scale-transactional 4`) to simulate increased traffic for capacity planning.
- **A/B testing** -- Compare two or more database configurations side by side with `pg-retest ab`.
- **CI/CD pipelines** -- Automate the entire cycle with `pg-retest run --config .pg-retest.toml`, including Docker provisioning, threshold evaluation, and JUnit XML output.
- **Cross-database capture** -- Capture from MySQL slow query logs and transform SQL automatically for PostgreSQL replay.

## Quick Reference

| Command | Purpose |
|---------|---------|
| `pg-retest capture` | Capture workload from logs |
| `pg-retest proxy` | Capture via PG wire protocol proxy |
| `pg-retest replay` | Replay workload against a target |
| `pg-retest compare` | Compare source vs. replay results |
| `pg-retest inspect` | View workload profile as JSON |
| `pg-retest run` | Full CI/CD pipeline |
| `pg-retest ab` | A/B test across database targets |
| `pg-retest web` | Launch the web dashboard |
| `pg-retest transform` | AI-powered workload transformation |
| `pg-retest tune` | AI-assisted database tuning |
| `pg-retest proxy-ctl` | Control a running persistent proxy |

For verbose logging on any command, add `-v` or set `RUST_LOG=debug`:

```bash
pg-retest -v capture --source-log ./demo.csv --output demo-workload.wkl
# or
RUST_LOG=debug pg-retest capture --source-log ./demo.csv --output demo-workload.wkl
```
