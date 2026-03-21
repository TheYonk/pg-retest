# Comparison and Reporting Guide

After replaying a workload, `pg-retest compare` produces a performance comparison between the original captured workload and the replay results. This guide covers terminal and JSON output formats, regression detection, threshold evaluation, exit codes, and capacity planning reports.

## Basic Usage

```bash
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl
```

The `--source` flag points to the original workload profile (produced by `pg-retest capture` or `pg-retest proxy`). The `--replay` flag points to the results file (produced by `pg-retest replay`). Both are binary MessagePack files.

## Terminal Report

By default, `pg-retest compare` prints a formatted table to the terminal showing side-by-side metrics from the source workload and the replay:

```
  pg-retest Comparison Report
  ===========================

  Metric               Source     Replay      Delta   Status
  ----------------------------------------------------------
  Total queries          1500       1500          0       OK
  Avg latency          2.3ms      2.5ms      +8.7%   SLOWER
  P50 latency          1.1ms      1.2ms      +9.1%   SLOWER
  P95 latency          8.4ms      9.1ms      +8.3%   SLOWER
  P99 latency         22.1ms     24.8ms     +12.2%   SLOWER
  Errors                   0          3         +3     WARN

  Top 3 Regressions:
  ----------------------------------------------------------
  1. SELECT * FROM orders WHERE customer_id = 42 +156.2% (1.2ms -> 3.1ms)
  2. UPDATE inventory SET qty = qty - 1 WHERE id +89.4% (0.8ms -> 1.5ms)
  3. SELECT count(*) FROM analytics WHERE date > +45.0% (12.3ms -> 17.8ms)

  Result: PASS
```

### Metrics Explained

| Metric | Description |
|--------|-------------|
| **Total queries** | Number of queries in the source workload vs. number replayed. A mismatch indicates filtered queries (read-only mode) or skipped queries (failed transactions). |
| **Avg latency** | Arithmetic mean of all query durations. Source values come from the captured workload; replay values are measured against the target. |
| **P50 latency** | Median query duration (50th percentile). |
| **P95 latency** | 95th percentile query duration. Represents tail latency for most queries. |
| **P99 latency** | 99th percentile query duration. Represents extreme tail latency. |
| **Errors** | Number of queries that failed during replay (connection errors, constraint violations, syntax errors, etc.). Source is always 0 because captured queries were known to have executed. |

### Status Column

The status column uses a +/-5% threshold for latency rows:

| Status | Meaning |
|--------|---------|
| `OK` | Change is within +/-5% of the source value. |
| `FASTER` | Replay latency is more than 5% lower than source. |
| `SLOWER` | Replay latency is more than 5% higher than source. |
| `DIFF` | For non-latency rows (total queries), indicates a difference. |
| `WARN` | For the errors row, indicates one or more errors occurred. |

### Top Regressions

The report lists up to 10 of the worst regressions, sorted by severity (highest percentage increase first). Each entry shows:

- A 50-character preview of the SQL text
- The percentage increase
- The original and replay latency in milliseconds

## JSON Report (`--json`)

Use `--json` to write a machine-readable JSON report to a file. This is useful for CI/CD pipelines, dashboards, or further analysis.

```bash
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --json report.json
```

The terminal report is still printed. The JSON file contains the full `ComparisonReport` structure:

```json
{
  "total_queries_source": 1500,
  "total_queries_replayed": 1500,
  "total_errors": 3,
  "source_avg_latency_us": 2300,
  "replay_avg_latency_us": 2500,
  "source_p50_latency_us": 1100,
  "replay_p50_latency_us": 1200,
  "source_p95_latency_us": 8400,
  "replay_p95_latency_us": 9100,
  "source_p99_latency_us": 22100,
  "replay_p99_latency_us": 24800,
  "regressions": [
    {
      "sql": "SELECT * FROM orders WHERE customer_id = 42 AND status = 'pending'",
      "original_us": 1200,
      "replay_us": 3100,
      "change_pct": 158.3
    },
    {
      "sql": "UPDATE inventory SET qty = qty - 1 WHERE id = 99",
      "original_us": 800,
      "replay_us": 1500,
      "change_pct": 87.5
    }
  ]
}
```

All latency values in the JSON are in **microseconds** (matching the internal representation). The `regressions` array is sorted by `change_pct` descending (worst regressions first).

### JSON Fields

| Field | Type | Description |
|-------|------|-------------|
| `total_queries_source` | `u64` | Number of queries in the original workload. |
| `total_queries_replayed` | `u64` | Number of queries replayed. |
| `total_errors` | `u64` | Count of failed queries during replay. |
| `source_avg_latency_us` | `u64` | Average query latency from the source, in microseconds. |
| `replay_avg_latency_us` | `u64` | Average query latency from replay, in microseconds. |
| `source_p50_latency_us` | `u64` | Source P50 (median) latency in microseconds. |
| `replay_p50_latency_us` | `u64` | Replay P50 latency in microseconds. |
| `source_p95_latency_us` | `u64` | Source P95 latency in microseconds. |
| `replay_p95_latency_us` | `u64` | Replay P95 latency in microseconds. |
| `source_p99_latency_us` | `u64` | Source P99 latency in microseconds. |
| `replay_p99_latency_us` | `u64` | Replay P99 latency in microseconds. |
| `regressions` | `array` | List of regression objects (see below). |

### Regression Object

| Field | Type | Description |
|-------|------|-------------|
| `sql` | `string` | Full SQL text of the regressed query. |
| `original_us` | `u64` | Original query duration in microseconds. |
| `replay_us` | `u64` | Replay query duration in microseconds. |
| `change_pct` | `f64` | Percentage change (positive = slower). |

## Regression Detection

A query is flagged as a regression when its replay duration exceeds its original duration by more than the configured threshold percentage. The threshold is controlled by `--threshold` (default: 20%).

The formula:

```
change_pct = ((replay_us - original_us) / original_us) * 100
```

If `change_pct > threshold`, the query is added to the regressions list.

Queries with an original duration of 0 microseconds are excluded from regression detection to avoid division-by-zero artifacts.

**ReadOnly mode note:** When comparing a replay that used `--read-only`, the source percentile calculations correctly filter to only the queries that were actually replayed (i.e., SELECT queries). This ensures the source P50/P95/P99 values are computed over the same query population as the replay, producing accurate apples-to-apples comparisons.

```bash
# Flag queries that are >50% slower (stricter threshold)
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --threshold 50.0

# Flag queries that are >10% slower (more sensitive)
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --threshold 10.0
```

## Exit Codes

By default, `pg-retest compare` always exits with code 0 regardless of results. Use `--fail-on-regression` and `--fail-on-error` to enable non-zero exit codes for CI/CD integration.

| Exit Code | Label | Condition |
|-----------|-------|-----------|
| `0` | `PASS` | No failures, or failure flags not enabled. |
| `1` | `FAIL (regressions detected)` | `--fail-on-regression` is set and at least one regression was found. |
| `2` | `FAIL (query errors detected)` | `--fail-on-error` is set and at least one query error occurred during replay. |

Error detection takes priority over regression detection. If both `--fail-on-error` and `--fail-on-regression` are set and both errors and regressions are present, the exit code is `2` (errors).

```bash
# Fail the CI job if any regressions are detected
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --fail-on-regression

# Fail on both errors and regressions
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --fail-on-regression \
  --fail-on-error

# Use in a shell script
pg-retest compare --source workload.wkl --replay results.wkl --fail-on-regression
exit_code=$?
if [ $exit_code -eq 1 ]; then
  echo "Performance regressions detected"
elif [ $exit_code -eq 2 ]; then
  echo "Query errors during replay"
fi
```

## Threshold Evaluation (Pipeline Mode)

When using `pg-retest run` with a TOML pipeline config, the `[thresholds]` section provides fine-grained pass/fail criteria beyond the simple regression/error flags of the `compare` subcommand.

```toml
[thresholds]
p95_max_ms = 50.0
p99_max_ms = 200.0
error_rate_max_pct = 1.0
regression_max_count = 5
regression_threshold_pct = 20.0
```

### Threshold Checks

| Threshold | Description |
|-----------|-------------|
| `p95_max_ms` | Maximum allowed P95 replay latency in milliseconds. Fails if the replay P95 exceeds this value. |
| `p99_max_ms` | Maximum allowed P99 replay latency in milliseconds. Fails if the replay P99 exceeds this value. |
| `error_rate_max_pct` | Maximum allowed error rate as a percentage of total replayed queries. For example, `1.0` means at most 1% of queries may fail. |
| `regression_max_count` | Maximum number of regressed queries allowed. If the number of regressions exceeds this count, the threshold fails. |
| `regression_threshold_pct` | Percentage threshold for what counts as a regression (default: 20%). A query must be this much slower than the original to be counted as regressed. |

Each threshold is evaluated independently. The pipeline reports which thresholds passed and which failed, with actual vs. limit values. All threshold fields are optional -- omitted thresholds are not checked.

### Threshold Result Output

Each threshold check produces a result with:

| Field | Description |
|-------|-------------|
| `name` | Identifier (e.g., `p95_latency`, `error_rate`, `regression_count`). |
| `passed` | Whether the actual value is within the limit. |
| `actual` | The measured value. |
| `limit` | The configured maximum. |
| `message` | Human-readable failure description (only when failed). |

Example failure messages:
- `P95 latency 62.3ms exceeds limit 50.0ms`
- `Error rate 2.10% exceeds limit 1.0%`
- `8 regressions found, max allowed: 5`

### JUnit XML Output

When `[output].junit_xml` is set in the pipeline config, threshold results are written as JUnit XML for integration with CI systems (Jenkins, GitLab CI, GitHub Actions, etc.):

```toml
[output]
junit_xml = "results.xml"
```

The XML contains one `<testcase>` per threshold check. Failed thresholds include a `<failure>` element with the failure message.

### Pipeline Exit Codes

The full pipeline (`pg-retest run`) uses a broader set of exit codes than the standalone `compare` subcommand:

| Code | Stage | Meaning |
|------|-------|---------|
| `0` | -- | All stages passed, all thresholds met. |
| `1` | Threshold | One or more threshold checks failed. |
| `2` | Config | Configuration file is invalid or missing. |
| `3` | Capture | Workload capture failed. |
| `4` | Provision | Database provisioning (Docker container) failed. |
| `5` | Replay | Replay execution failed. |

## Capacity Planning Reports

When replay is run with `--scale N` (where N > 1), the engine automatically prints a capacity planning report after replay completes. This report focuses on aggregate throughput and latency under scaled load rather than per-query comparison.

### Terminal Output

```
  Scaled Replay Report
  ====================

  Scale factor:    5x
  Total sessions:  50
  Total queries:   7500
  Throughput:      1234.5 queries/sec
  Avg latency:     3.21 ms
  P95 latency:     12.40 ms
  P99 latency:     28.90 ms
  Errors:          12
  Error rate:      0.16%
```

### Metrics

| Metric | Description |
|--------|-------------|
| **Scale factor** | The `--scale` value used for the replay. |
| **Total sessions** | Number of concurrent sessions (original sessions * scale factor). |
| **Total queries** | Total queries executed across all sessions. |
| **Throughput** | Queries per second, calculated as `total_queries / elapsed_seconds`. Measured wall-clock time, including all concurrent sessions. |
| **Avg latency** | Arithmetic mean of all query durations across all scaled sessions. |
| **P95 latency** | 95th percentile of all query durations. |
| **P99 latency** | 99th percentile of all query durations. |
| **Errors** | Count of failed queries across all sessions. |
| **Error rate** | `(errors / total_queries) * 100`, as a percentage. |

### Scale Report JSON Structure

When used in a pipeline with `[output].json_report`, the scale report is serialized as:

```json
{
  "scale_factor": 5,
  "total_sessions": 50,
  "total_queries": 7500,
  "throughput_qps": 1234.5,
  "avg_latency_us": 3210,
  "p95_latency_us": 12400,
  "p99_latency_us": 28900,
  "error_count": 12,
  "error_rate_pct": 0.16
}
```

All latency values in the JSON are in **microseconds**.

### Using Capacity Reports

Capacity reports answer questions like:

- "Can our database handle 3x current traffic?" -- Run `--scale 3` and check if P95 latency stays acceptable and error rate stays near 0%.
- "At what scale does the database start degrading?" -- Run multiple replays with increasing `--scale` values and compare throughput and latency trends.
- "Which query category is the bottleneck?" -- Use per-category scaling (`--scale-analytical 5 --scale-transactional 1`) to isolate which workload class causes degradation.

## CLI Reference

```
pg-retest compare [OPTIONS] --source <PATH> --replay <PATH>
```

### Required Arguments

| Flag | Description |
|------|-------------|
| `--source <PATH>` | Path to the original workload profile (`.wkl` file from capture). |
| `--replay <PATH>` | Path to the replay results file (`.wkl` file from replay). |

### Optional Arguments

| Flag | Default | Description |
|------|---------|-------------|
| `--json <PATH>` | _(none)_ | Write a JSON comparison report to the specified file. |
| `--output-format <FMT>` | `text` | Output format for the terminal report: `text` (formatted table) or `json` (structured JSON to stdout). |
| `--threshold <FLOAT>` | `20.0` | Regression threshold percentage. Queries slower by more than this percentage are flagged as regressions. |
| `--fail-on-regression` | `false` | Exit with code 1 if any regressions are detected. |
| `--fail-on-error` | `false` | Exit with code 2 if any query errors occurred during replay. |

### Global Options

| Flag | Description |
|------|-------------|
| `-v, --verbose` | Enable debug-level logging (`RUST_LOG=debug`). |

## Examples

### Basic Comparison

```bash
pg-retest compare --source workload.wkl --replay results.wkl
```

### JSON Report for CI

```bash
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --json report.json \
  --fail-on-regression \
  --threshold 15.0
```

### Strict CI Gate

```bash
pg-retest compare \
  --source workload.wkl \
  --replay results.wkl \
  --fail-on-regression \
  --fail-on-error \
  --threshold 10.0
```

### Pipeline with Thresholds

```toml
# .pg-retest.toml
[capture]
workload = "workload.wkl"

[replay]
target = "host=localhost dbname=myapp_test"
speed = 0

[thresholds]
p95_max_ms = 50.0
p99_max_ms = 200.0
error_rate_max_pct = 0.5
regression_max_count = 3
regression_threshold_pct = 15.0

[output]
json_report = "report.json"
junit_xml = "results.xml"
```

```bash
pg-retest run --config .pg-retest.toml
```

### Compare After Scaled Replay

```bash
# Step 1: Replay at 5x scale
pg-retest replay \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp_test" \
  --scale 5 \
  --stagger-ms 200 \
  --output results_5x.wkl

# Step 2: Compare (uses the same source workload)
# Note: the comparison report shows per-query metrics.
# The capacity report (throughput, error rate) was already
# printed by the replay command above.
pg-retest compare \
  --source workload.wkl \
  --replay results_5x.wkl \
  --json report_5x.json
```
