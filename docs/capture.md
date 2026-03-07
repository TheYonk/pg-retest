# Capture Methods

pg-retest supports four capture backends for recording database workloads. Each backend produces the same `.wkl` workload profile format, which can then be replayed, compared, and analyzed using the rest of the tool.

The capture backends are:

1. [CSV Log Capture](#csv-log-capture) -- Parse PostgreSQL CSV log files (default)
2. [Proxy Capture](#proxy-capture) -- Intercept live traffic via PG wire protocol proxy
3. [MySQL Slow Log Capture](#mysql-slow-log-capture) -- Parse MySQL slow query logs with automatic SQL transformation
4. [RDS/Aurora Capture](#rdsaurora-capture) -- Download and parse logs from AWS RDS/Aurora instances
5. [PII Masking](#pii-masking) -- Mask sensitive values across any capture method

---

## CSV Log Capture

**Source type:** `--source-type pg-csv` (default)

This is the simplest capture method. It parses PostgreSQL's CSV-format log files to extract per-session query streams with timing metadata. There is zero runtime overhead on the database -- you are reading files that PostgreSQL already writes.

### PostgreSQL Logging Configuration

Add these settings to `postgresql.conf`:

```ini
# Required: enable the logging collector and CSV output
logging_collector = on
log_destination = 'csvlog'

# Required: log all statements with execution duration
# Set to 0 to capture everything; set higher (e.g., 100) to
# capture only queries slower than 100ms
log_min_duration_statement = 0

# Recommended: disable log_statement to avoid duplicate entries
log_statement = 'none'

# Optional: configure log file location and naming
log_directory = 'pg_log'
log_filename = 'postgresql-%Y-%m-%d.csv'
log_rotation_age = 1d
log_rotation_size = 0
```

**Restart vs. reload:**

| Setting | Requires Restart? |
|---------|-------------------|
| `logging_collector` | Yes -- full restart required |
| `log_destination` | No -- reload is sufficient |
| `log_min_duration_statement` | No -- reload is sufficient |
| `log_statement` | No -- reload is sufficient |
| `log_directory` | No -- reload is sufficient |
| `log_filename` | No -- reload is sufficient |

To reload without restart:

```sql
SELECT pg_reload_conf();
```

Or from the command line:

```bash
pg_ctl reload -D /path/to/data
```

### Command Examples

Basic capture:

```bash
pg-retest capture \
  --source-log /var/lib/postgresql/data/pg_log/postgresql-2026-03-06.csv \
  --source-type pg-csv \
  --output workload.wkl
```

With metadata:

```bash
pg-retest capture \
  --source-log ./server-logs/postgresql-2026-03-06.csv \
  --source-type pg-csv \
  --source-host prod-db-01.example.com \
  --pg-version 16.2 \
  --output workload.wkl
```

With PII masking:

```bash
pg-retest capture \
  --source-log ./postgresql.csv \
  --source-type pg-csv \
  --mask-values \
  --output workload-masked.wkl
```

### What Gets Captured

The CSV log parser extracts:

- **Simple queries:** Lines matching `duration: X.XXX ms  statement: SQL...`
- **Prepared statement executions:** Lines matching `duration: X.XXX ms  execute <name>: SQL...` with parameter inlining from the detail field
- **Session grouping:** Queries are grouped by PG session ID (field 5 in the CSV format)
- **Transaction boundaries:** `BEGIN`, `COMMIT`, and `ROLLBACK` statements are captured and assigned transaction IDs

The parser skips:

- `bind` and `parse` entries (to avoid duplicating prepared statement queries -- only the `execute` entry is captured, since it carries the actual execution duration)
- Non-`LOG` severity entries (errors, warnings, etc.)
- Lines without a `duration:` prefix

### Performance Impact

CSV log capture reads log files after the fact. The only overhead is the cost of PostgreSQL writing those log files in the first place.

Setting `log_min_duration_statement = 0` causes PostgreSQL to log every query, which adds I/O overhead proportional to your query volume. For high-throughput production systems, consider:

- Setting a higher threshold (e.g., `log_min_duration_statement = 10` for queries >10ms)
- Using a separate disk for `log_directory`
- Capturing for a limited time window and then reverting the setting

---

## Proxy Capture

**Command:** `pg-retest proxy`

The proxy capture method places pg-retest between your application and PostgreSQL as a transparent wire protocol proxy. It intercepts all SQL traffic in real time, records it, and passes it through to the real database.

### How It Works

```
Application  -->  pg-retest proxy (port 5433)  -->  PostgreSQL (port 5432)
```

The proxy:

1. Listens for incoming PG connections on the listen address
2. For each client connection, checks out a server connection from the session pool
3. Relays the full PG wire protocol bidirectionally (startup, authentication, queries, results)
4. Records every SQL statement with timing metadata as a `CaptureEvent`
5. On shutdown (Ctrl+C, SIGTERM, or duration elapsed), builds a workload profile and writes it to disk

The proxy supports SCRAM-SHA-256 authentication and handles prepared statements.

### Command Reference

```bash
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --output workload.wkl \
  --pool-size 100 \
  --pool-timeout 30 \
  --duration 5m \
  --mask-values
```

| Flag | Default | Description |
|------|---------|-------------|
| `--listen` | `0.0.0.0:5433` | Address and port the proxy listens on |
| `--target` | (required) | Target PostgreSQL address (e.g., `localhost:5432`) |
| `-o, --output` | `workload.wkl` | Output workload profile path |
| `--pool-size` | `100` | Maximum server connections in the pool |
| `--pool-timeout` | `30` | Seconds to wait for a pool connection before timing out |
| `--mask-values` | `false` | Enable PII masking on captured SQL |
| `--no-capture` | `false` | Run as a pure proxy without recording (useful for testing) |
| `--duration` | (none) | Auto-stop after duration (e.g., `60s`, `5m`). Without this, runs until Ctrl+C. |

### Connection Pooling

The proxy uses session-mode connection pooling. Each client connection is paired with a dedicated server connection for the lifetime of that client session. This preserves transaction state, prepared statements, and session-level settings.

The pool pre-allocates up to `--pool-size` connections to the target. If all connections are in use, new clients wait up to `--pool-timeout` seconds before receiving an error.

### Example: Capture Live Traffic

Start the proxy:

```bash
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5441 \
  --output captured.wkl
```

Point your application at the proxy instead of the real database:

```bash
# Before: app connects to localhost:5441
# After:  app connects to localhost:5433

psql "host=localhost port=5433 user=sales_demo_app password=salesdemo123 dbname=sales_demo"
```

Run your application workload, then press Ctrl+C to stop the proxy and save the capture.

### Example: Time-Limited Capture

Capture for exactly 5 minutes, then auto-stop:

```bash
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5441 \
  --output captured.wkl \
  --duration 5m
```

### Performance Overhead

The proxy uses buffered I/O (`BufReader`/`BufWriter`) on all relay paths and flushes strategically (immediately to the server, on `ReadyForQuery` to the client). With this approach:

- **Batch workloads:** Effectively 0% overhead. Buffered I/O batches responses together.
- **Per-query overhead:** ~0 microseconds measured with 1,000 sequential SELECTs.
- **Concurrent connections:** Tested with 20+ simultaneous connections without issues.

The proxy handles both SIGINT (Ctrl+C) and SIGTERM for graceful shutdown, making it compatible with Docker and Kubernetes environments.

### Capturing via the Web Dashboard

The web dashboard provides a GUI for starting and stopping the proxy:

1. Start the dashboard: `pg-retest web --port 8080`
2. Navigate to the **Proxy** page
3. Enter the target address and configure pool settings
4. Click **Start Proxy**
5. Monitor live traffic in real time via WebSocket
6. Click **Stop Proxy** to save the workload profile

The dashboard uses `TaskManager` with a `CancellationToken` to manage the proxy lifecycle, so the proxy runs as a background task and can be cancelled cleanly from the UI.

---

## MySQL Slow Log Capture

**Source type:** `--source-type mysql-slow`

This backend parses MySQL's slow query log and automatically transforms the SQL into PostgreSQL-compatible syntax. This enables cross-database workload testing: capture what your MySQL application does, then replay it against PostgreSQL.

### MySQL Slow Query Log Configuration

Enable the slow query log in MySQL's `my.cnf`:

```ini
[mysqld]
slow_query_log = 1
slow_query_log_file = /var/log/mysql/slow.log
long_query_time = 0          # Set to 0 to capture all queries
log_queries_not_using_indexes = 1   # Optional: also log unindexed queries
```

Restart MySQL after changing these settings.

Alternatively, enable at runtime (does not persist across restarts):

```sql
SET GLOBAL slow_query_log = 1;
SET GLOBAL long_query_time = 0;
```

### Command Examples

```bash
pg-retest capture \
  --source-log /var/log/mysql/slow.log \
  --source-type mysql-slow \
  --source-host mysql-prod-01 \
  --output mysql-workload.wkl
```

### SQL Transform Pipeline

When `--source-type mysql-slow` is used, pg-retest automatically applies a composable transform pipeline that converts MySQL-specific SQL syntax into PostgreSQL equivalents. The pipeline applies these transforms in order:

**1. Skip MySQL-specific commands**

Commands that have no PostgreSQL equivalent are dropped entirely:

| Skipped Commands |
|-----------------|
| `SHOW ...` |
| `SET NAMES ...` |
| `SET CHARACTER SET ...` |
| `SET AUTOCOMMIT ...` |
| `SET SQL_MODE ...` |
| `FLUSH ...` |
| `HANDLER ...` |
| `RESET ...` |
| `DESCRIBE ...` / `DESC ...` |
| `USE ...` |

**2. Backtick to double-quote identifier quoting**

```
MySQL:  SELECT `id`, `name` FROM `users`
PG:     SELECT "id", "name" FROM "users"
```

**3. LIMIT offset,count to LIMIT count OFFSET offset**

```
MySQL:  SELECT * FROM orders LIMIT 10, 20
PG:     SELECT * FROM orders LIMIT 20 OFFSET 10
```

Note: `LIMIT N` (without offset) is already compatible and passes through unchanged.

**4. IFNULL to COALESCE**

```
MySQL:  SELECT IFNULL(name, 'unknown') FROM users
PG:     SELECT COALESCE(name, 'unknown') FROM users
```

**5. IF() to CASE WHEN**

```
MySQL:  SELECT IF(status = 1, 'active', 'inactive') FROM users
PG:     SELECT CASE WHEN status = 1 THEN 'active' ELSE 'inactive' END FROM users
```

**6. UNIX_TIMESTAMP to EXTRACT(EPOCH FROM ...)**

```
MySQL:  SELECT UNIX_TIMESTAMP()
PG:     SELECT EXTRACT(EPOCH FROM NOW())::bigint

MySQL:  SELECT UNIX_TIMESTAMP(created_at)
PG:     SELECT EXTRACT(EPOCH FROM created_at)::bigint
```

**7. NOW() passthrough**

`NOW()` is compatible between MySQL and PostgreSQL -- no transform needed.

### Transform Report

After capture, pg-retest prints a summary showing how many queries were transformed, passed through unchanged, and skipped:

```
Transform summary:
  Transformed: 142
  Unchanged:   58
  Skipped:     23 (MySQL-specific commands)
```

### Limitations

The transform pipeline uses regex-based pattern matching, not a full SQL parser. This covers approximately 80-90% of real-world MySQL queries. Known limitations:

- **Backtick replacement inside string literals:** A backtick inside a quoted string (e.g., `'it\'s a \`test\`'`) will be incorrectly replaced with a double quote.
- **Single LIMIT rewrite per query:** Only the first `LIMIT offset, count` in a query is rewritten. Nested subqueries with their own LIMIT clauses may not be transformed.
- **MySQL-specific functions not listed above** (e.g., `GROUP_CONCAT`, `DATE_FORMAT` with MySQL format specifiers) are not transformed and will cause errors during replay.

For queries that fall outside the transform coverage, you will see errors during replay. The comparison report flags these so you can review them.

### MySQL Log Format

The parser expects the standard MySQL slow query log format:

```
# Time: 2024-03-08T10:00:00.100000Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.001234  Lock_time: 0.000100 Rows_sent: 1  Rows_examined: 100
SET timestamp=1709892000;
SELECT * FROM orders WHERE status = 'pending';
```

Queries are grouped by thread ID (the `Id` field in the `User@Host` line), which serves as the equivalent of a PostgreSQL session for replay parallelism.

---

## RDS/Aurora Capture

**Source type:** `--source-type rds`

This backend downloads PostgreSQL CSV log files directly from AWS RDS or Aurora instances using the AWS CLI, then parses them using the same CSV log parser.

### Prerequisites

**AWS CLI (v2):** Must be installed and configured with credentials that have permission to access RDS logs.

```bash
# Install (macOS)
brew install awscli

# Configure
aws configure
```

Required IAM permissions:

- `rds:DescribeDBLogFiles`
- `rds:DownloadDBLogFilePortion`

**RDS Instance Configuration:** The RDS/Aurora instance must have CSV logging enabled. In the RDS parameter group, set:

```
log_destination = csvlog
log_min_duration_statement = 0    # or your preferred threshold
```

Apply the parameter group and reboot the instance if required.

### Command Examples

Capture from the latest log file:

```bash
pg-retest capture \
  --source-type rds \
  --rds-instance my-prod-db \
  --rds-region us-east-1 \
  --source-host my-prod-db.us-east-1 \
  --output rds-workload.wkl
```

Capture from a specific log file:

```bash
pg-retest capture \
  --source-type rds \
  --rds-instance my-prod-db \
  --rds-region us-west-2 \
  --rds-log-file "error/postgresql.log.2026-03-06-10" \
  --output rds-workload.wkl
```

With PII masking:

```bash
pg-retest capture \
  --source-type rds \
  --rds-instance my-prod-db \
  --rds-region us-east-1 \
  --mask-values \
  --output rds-workload-masked.wkl
```

### Command Reference

| Flag | Default | Description |
|------|---------|-------------|
| `--source-type rds` | -- | Required: selects the RDS capture backend |
| `--rds-instance` | (required) | RDS DB instance identifier |
| `--rds-region` | `us-east-1` | AWS region where the instance is running |
| `--rds-log-file` | (latest) | Specific log file name. If omitted, the most recent log file is used. |
| `--source-host` | `unknown` | Metadata label for the source host |
| `--mask-values` | `false` | Enable PII masking |
| `-o, --output` | `workload.wkl` | Output path |

### How Pagination Works

RDS returns a maximum of 1MB of log data per API call. For larger log files, pg-retest automatically paginates using the `--marker` parameter:

1. Call `aws rds download-db-log-file-portion` with `--marker 0`
2. Check the `AdditionalDataPending` field in the response
3. If true, use the returned `Marker` value for the next call
4. Repeat until `AdditionalDataPending` is false

If a single download call fails, pg-retest retries once before reporting an error.

The progress is displayed as bytes downloaded:

```
Downloading RDS log file: error/postgresql.log.2026-03-06-10
Downloaded 2457832 bytes from RDS log
Captured 1542 queries across 23 sessions
```

### Listing Available Log Files

If you omit `--rds-log-file`, pg-retest calls `aws rds describe-db-log-files` to list all available log files for the instance, then selects the one with the most recent `LastWritten` timestamp.

---

## PII Masking

**Flag:** `--mask-values`

Available on all capture methods (`capture` and `proxy` subcommands), PII masking replaces literal values in captured SQL to prevent personally identifiable information from being stored in workload profiles.

### What It Does

The masker replaces:

- **Single-quoted strings** with `$S`
- **Dollar-quoted strings** (`$$...$$`) with `$S`
- **Numeric literals** with `$N`

Double-quoted identifiers (table and column names) are preserved unchanged.

### Examples

**Before masking:**

```sql
SELECT * FROM users WHERE email = 'alice@example.com' AND id = 42
```

**After masking:**

```sql
SELECT * FROM users WHERE email = $S AND id = $N
```

**More examples:**

```sql
-- Original
INSERT INTO orders (customer_id, total, note) VALUES (7, 149.99, 'Rush delivery')
-- Masked
INSERT INTO orders (customer_id, total, note) VALUES ($N, $N, $S)

-- Original (escaped quotes)
INSERT INTO t (s) VALUES ('it''s a test')
-- Masked
INSERT INTO t (s) VALUES ($S)

-- Original (dollar-quoting)
SELECT $$hello world$$
-- Masked
SELECT $S

-- Original (negative numbers)
SELECT * FROM t WHERE balance = -500
-- Masked
SELECT * FROM t WHERE balance = $N

-- Original (scientific notation)
SELECT * FROM t WHERE val = 1.5e10
-- Masked
SELECT * FROM t WHERE val = $N

-- Original (identifiers with numbers are preserved)
SELECT col1, col2 FROM table3
-- Masked (unchanged -- numbers in identifiers are not masked)
SELECT col1, col2 FROM table3

-- Original (double-quoted identifiers preserved)
SELECT "column1" FROM "table2" WHERE id = 5
-- Masked
SELECT "column1" FROM "table2" WHERE id = $N
```

### Edge Cases Handled

The masking engine uses a hand-written character-level state machine (not regex) specifically designed to handle SQL edge cases correctly:

- **Escaped single quotes** (`''`): Treated as part of the string literal, not as a string terminator. The entire string including the escaped quote is replaced with `$S`.
- **Dollar-quoting** (`$$...$$`): The standard PostgreSQL mechanism for quoting strings that contain single quotes. Recognized and replaced with `$S`.
- **Numbers in identifiers**: `table3`, `col_2`, `idx42` -- the digits are recognized as part of an identifier and not masked. The masker checks whether the preceding character is alphanumeric or an underscore.
- **Negative numbers**: A leading `-` is recognized as part of a numeric literal (not a subtraction operator) when it follows an operator or delimiter (`(`, `,`, `=`, `<`, `>`, `+`, `-`, `*`, `/`, `|`).
- **Decimal numbers and scientific notation**: `19.99`, `1.5e10`, `3E-4` are all recognized as single numeric literals and replaced with a single `$N`.
- **Double-quoted identifiers**: Everything between double quotes is passed through unchanged, preserving table and column names that happen to contain digits.

### Usage with Capture

```bash
# CSV log capture with masking
pg-retest capture --source-log ./pg.csv --mask-values --output masked.wkl

# Proxy capture with masking
pg-retest proxy --target localhost:5432 --mask-values --output masked.wkl

# RDS capture with masking
pg-retest capture --source-type rds --rds-instance my-db --mask-values --output masked.wkl
```

### When to Use Masking

Use `--mask-values` when:

- The workload profile will be shared with others who should not see production data
- The profile will be stored in version control or CI systems
- Compliance requirements (GDPR, HIPAA, etc.) restrict how query data is handled
- You are capturing from a production system with real user data

Masking does not affect replay accuracy for most workloads. The masked queries are syntactically valid SQL (with placeholder tokens), and the replay engine executes them as-is. However, queries with `WHERE` clauses referencing specific values will return different result sets, which may affect dependent logic in transaction sequences.

For workloads where exact value fidelity matters, capture without masking and control access to the `.wkl` file through other means (encryption, access controls, etc.).
