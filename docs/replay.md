# Replay Guide

pg-retest replays captured workload profiles against a target PostgreSQL database. The replay engine preserves the original connection parallelism, timing relationships, and transaction boundaries from the captured workload. This guide covers all replay modes, speed control, scaling options, and the full CLI reference.

## Prerequisites

Before replaying, you need:

1. A workload profile (`.wkl` file) produced by `pg-retest capture` or `pg-retest proxy`.
2. A target PostgreSQL instance to replay against.
3. For read-write replay: a database restored from a point-in-time backup matching the captured state.

## Replay Modes

### Read-Write Mode (Default)

Read-write mode replays every query exactly as captured, including all DML statements (`INSERT`, `UPDATE`, `DELETE`) and DDL. This is the default behavior when no mode flag is specified.

```bash
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost port=5432 dbname=myapp user=postgres" \
  --output results.wkl
```

**Important considerations for read-write replay:**

- DML statements mutate data. Each replayed write changes database state, which can alter query plans for subsequent queries. For accurate 1:1 performance comparison, always restore from a point-in-time backup before replaying.
- Transaction control statements (`BEGIN`, `COMMIT`, `ROLLBACK`) are replayed faithfully, preserving the original transaction boundaries.
- If a query within a transaction fails during replay, the engine automatically issues a `ROLLBACK` and skips remaining queries in that transaction (see "Transaction-Aware Replay" below).

### Read-Only Mode (`--read-only`)

Read-only mode strips all DML and transaction control statements, replaying only `SELECT` queries. This mode is safe to run against production replicas or shared environments where data mutation is not acceptable.

```bash
pg-retest replay \
  --workload workload.wkl \
  --target "host=replica.example.com dbname=myapp user=readonly" \
  --output results.wkl \
  --read-only
```

Queries filtered out in read-only mode:
- `INSERT`, `UPDATE`, `DELETE`
- `BEGIN`, `COMMIT`, `ROLLBACK`
- DDL statements (`CREATE`, `ALTER`, `DROP`, etc.)

Only `SELECT` and other read-only queries (`EXPLAIN`, `SHOW`, etc.) are replayed.

## Speed Control (`--speed`)

The `--speed` flag controls the timing between queries relative to the original captured timing. Each query in the workload profile has a `start_offset_us` timestamp recording when it was executed relative to the start of its session. The replay engine uses these offsets, divided by the speed multiplier, to schedule query execution.

| Value | Behavior |
|-------|----------|
| `1.0` | Real-time replay (default). Queries execute at the same pace as the original workload. |
| `0.5` | Half speed. Twice as much time between queries. Useful for reducing load on the target. |
| `2.0` | Double speed. Half the time between queries. Useful for stress testing. |
| `0`   | Maximum speed. No inter-query delays. All queries fire as fast as possible. Best for benchmarking raw throughput. |

```bash
# Max-speed replay (no delays between queries)
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp" \
  --speed 0

# Half-speed replay (gentle load)
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp" \
  --speed 0.5
```

When `--speed 0` is set, the engine skips all `sleep_until` calls entirely and executes queries back-to-back within each session. Connection parallelism is still preserved -- sessions still run concurrently, so you get realistic concurrent access patterns without artificial timing delays.

## Transaction-Aware Replay

The replay engine tracks transaction boundaries using the `transaction_id` field on each query. This enables intelligent error handling within transactions:

1. When a query inside a transaction fails, the engine issues an automatic `ROLLBACK` to the database.
2. The `transaction_id` of the failed query is recorded as the "failed transaction."
3. All subsequent queries with the same `transaction_id` are skipped and recorded as errors with the message `"skipped: transaction failed"`.
4. When a `COMMIT` or `ROLLBACK` for the failed transaction is encountered, it is recorded with `"skipped: transaction already rolled back"` and the failed state is cleared.
5. Queries belonging to a different transaction (or with no transaction) proceed normally.

This prevents cascading errors from a single failed query inside a transaction. Without this behavior, every subsequent query in the same transaction would fail with a "current transaction is aborted" error from PostgreSQL, producing noisy and misleading results.

Example of how the engine handles a failed transaction:

```
Session 1:
  BEGIN                      (txn_id=1)  -> executed
  INSERT INTO orders ...     (txn_id=1)  -> FAILS (e.g., constraint violation)
                                            -> engine issues ROLLBACK automatically
  UPDATE inventory ...       (txn_id=1)  -> SKIPPED ("transaction failed")
  COMMIT                     (txn_id=1)  -> SKIPPED ("transaction already rolled back")
  SELECT * FROM products     (no txn)    -> executed normally
```

## Connection Parallelism

The replay engine creates one PostgreSQL connection per captured session. All sessions run concurrently using Tokio async tasks. This preserves the original concurrency patterns from the source workload.

- A workload with 50 captured sessions produces 50 concurrent connections to the target database.
- Each session replays its queries sequentially (respecting inter-query timing via `--speed`), but all sessions execute in parallel.
- This is critical for realistic performance measurement: serializing inherently parallel workloads would produce misleading latency numbers.

Make sure the target PostgreSQL instance has `max_connections` set high enough to accommodate all concurrent sessions. For scaled replays (see below), the connection count equals `original_sessions * scale_factor`.

## Scaled Benchmark (`--scale N`)

Uniform scaling duplicates every session `N` times to simulate increased traffic. This is used for capacity planning -- you can test how the target database handles 2x, 5x, or 10x the original workload.

```bash
# Replay at 3x the original concurrency
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp" \
  --scale 3 \
  --stagger-ms 100
```

### How Scaling Works

Given a workload with 10 sessions and `--scale 3`:

1. The engine creates 30 sessions total (10 original + 10 copy-1 + 10 copy-2).
2. Each copy gets unique session IDs to avoid collisions.
3. All 30 sessions run concurrently, each with its own database connection.
4. A capacity planning report is printed after replay completes, showing throughput (QPS), latency percentiles, and error rate.

### Stagger (`--stagger-ms`)

The `--stagger-ms` flag adds a time offset between each batch of scaled copies. Without stagger, all copies start simultaneously, which creates an unrealistic thundering-herd spike at the beginning of the replay.

With `--stagger-ms 500` and `--scale 3`:
- Copy 0 (original sessions): start at time 0
- Copy 1: start at time +500ms
- Copy 2: start at time +1000ms

The stagger is applied by adding the offset to every query's `start_offset_us` in the copied sessions. This produces a gradual ramp-up that more closely resembles real traffic growth.

### Write Safety Warning

When scaling a workload that contains write queries (`INSERT`, `UPDATE`, `DELETE`, DDL), the engine prints a safety warning:

```
Warning: scaling a workload with 142 write queries (out of 500 total).
Scaled writes will execute multiple times, which changes data state
and may produce different results than the original workload.
```

Scaled writes execute N times, which changes data state in ways that differ from the original workload. This is inherent to the scaling approach -- there is no way to "scale" writes without executing them multiple times. If data integrity matters, consider using `--read-only` with scaling, or accept the benchmark-mode tradeoff.

## Per-Category Scaling

Per-category scaling lets you scale each workload class independently. This is useful for targeted capacity planning -- for example, simulating 5x analytical query load while keeping transactional load at 1x.

```bash
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp" \
  --scale-analytical 5 \
  --scale-transactional 2 \
  --scale-mixed 1 \
  --scale-bulk 0 \
  --stagger-ms 200
```

### Workload Classes

Each session in the workload is classified into one of four categories based on its query mix:

| Class | Criteria |
|-------|----------|
| **Analytical** | >80% reads, average latency >10ms. Typical for reporting and OLAP queries. |
| **Transactional** | >20% writes, average latency <5ms, >2 transactions. Typical for OLTP workloads. |
| **Bulk** | >80% writes, <=2 transactions. Typical for batch data loading. |
| **Mixed** | Everything else. Sessions that do not fit the above patterns. |

Use `pg-retest inspect --classify workload.wkl` to see how your workload is classified before scaling.

### Behavior

- Per-category scaling is **mutually exclusive** with uniform `--scale N`. If any `--scale-*` flag is set, per-category mode takes priority.
- Unspecified classes default to 1x (no scaling).
- A scale factor of `0` excludes all sessions of that class entirely.
- Stagger (`--stagger-ms`) applies globally across all class copies, using a shared counter to ensure consistent spacing.
- Duplicate sessions receive monotonically increasing IDs to avoid collisions.
- Copy 0 for each class keeps the original session IDs and offsets. Duplicates (copy 1, 2, ...) get new IDs and staggered offsets.

### Example: Stress-Test Analytical Queries Only

```bash
# Scale analytical sessions to 10x, keep everything else at 1x
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp" \
  --scale-analytical 10 \
  --stagger-ms 500 \
  --speed 0
```

### Example: Remove Bulk and Scale Transactional

```bash
# Drop bulk sessions entirely, triple transactional load
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp" \
  --scale-transactional 3 \
  --scale-bulk 0
```

## Output

The replay engine writes results to a binary MessagePack file (default: `results.wkl`). This file contains per-session, per-query replay results including:

- The original SQL text
- The original duration (microseconds) from the captured workload
- The replay duration (microseconds) measured on the target
- Success/failure status
- Error message (if the query failed)

These results are consumed by `pg-retest compare` to produce comparison reports.

When `--scale` is greater than 1, the engine also prints a capacity planning report to the terminal with throughput QPS, latency percentiles, and error rate. See the [compare guide](compare.md) for details on the capacity report format.

## CLI Reference

```
pg-retest replay [OPTIONS] --workload <PATH> --target <CONNSTRING>
```

### Required Arguments

| Flag | Description |
|------|-------------|
| `--workload <PATH>` | Path to the input workload profile (`.wkl` file). |
| `--target <CONNSTRING>` | PostgreSQL connection string for the target database. Format: `"host=... port=... dbname=... user=... password=..."` or a `postgresql://` URI. |

### Optional Arguments

| Flag | Default | Description |
|------|---------|-------------|
| `-o, --output <PATH>` | `results.wkl` | Path for the output results file. |
| `--read-only` | `false` | Strip DML and transaction control; replay only `SELECT` queries. |
| `--speed <FLOAT>` | `1.0` | Speed multiplier. `0` = max speed, `0.5` = half speed, `2.0` = double speed. |
| `--scale <N>` | `1` | Uniform scale factor. Duplicate all sessions N times for load testing. |
| `--stagger-ms <MS>` | `0` | Milliseconds between each batch of scaled session copies. |
| `--scale-analytical <N>` | _(unset)_ | Scale analytical sessions by N. Enables per-category mode. |
| `--scale-transactional <N>` | _(unset)_ | Scale transactional sessions by N. Enables per-category mode. |
| `--scale-mixed <N>` | _(unset)_ | Scale mixed sessions by N. Enables per-category mode. |
| `--scale-bulk <N>` | _(unset)_ | Scale bulk sessions by N. Enables per-category mode. |
| `--max-connections <N>` | _(unlimited)_ | Limit the number of concurrent database connections via a semaphore. Useful when the target has a low `max_connections` setting or when replaying highly scaled workloads. |
| `--tls-mode <MODE>` | `prefer` | TLS mode for the target connection: `disable`, `prefer`, or `require`. |
| `--tls-ca-cert <PATH>` | _(none)_ | Path to a custom CA certificate file for TLS verification. Required when the target uses a private CA. |
| `--target-env <VAR>` | _(none)_ | Read the target connection string from the named environment variable instead of `--target`. |

### Global Options

| Flag | Description |
|------|-------------|
| `-v, --verbose` | Enable debug-level logging (`RUST_LOG=debug`). |

## Examples

### Basic Replay

```bash
# Capture from CSV log
pg-retest capture --source-log pg_log.csv --output workload.wkl

# Restore backup to target database
pg_restore -d myapp_test backup.dump

# Replay at real-time speed
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost port=5432 dbname=myapp_test user=postgres" \
  --output results.wkl

# Compare results
pg-retest compare --source workload.wkl --replay results.wkl
```

### Read-Only Against a Replica

```bash
pg-retest replay \
  --workload workload.wkl \
  --target "host=replica.internal dbname=myapp user=readonly" \
  --read-only \
  --speed 0
```

### Capacity Planning at 5x Load

```bash
pg-retest replay \
  --workload workload.wkl \
  --target "host=staging-db dbname=myapp" \
  --scale 5 \
  --stagger-ms 200 \
  --speed 0 \
  --output results_5x.wkl
```

### Mixed Scaling for Targeted Testing

```bash
pg-retest replay \
  --workload workload.wkl \
  --target "host=staging-db dbname=myapp" \
  --scale-analytical 10 \
  --scale-transactional 3 \
  --scale-mixed 1 \
  --scale-bulk 0 \
  --stagger-ms 500 \
  --speed 0
```
