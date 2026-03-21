# AI-Assisted Tuning Guide

`pg-retest tune` uses an LLM to analyze your PostgreSQL workload and database state, generate tuning recommendations, apply them, replay the workload, and measure the impact — automatically rolling back changes that cause regressions. This guide covers the full tuning loop, safety features, LLM provider options, and the complete CLI reference.

## Quick Start

```bash
# Dry-run (default): see what the LLM recommends without changing anything
pg-retest tune \
  --workload workload.wkl \
  --target "host=localhost port=5432 dbname=myapp user=postgres"

# Apply recommendations (requires --apply)
pg-retest tune \
  --workload workload.wkl \
  --target "host=localhost port=5432 dbname=myapp user=postgres" \
  --apply

# Multi-iteration tuning with a hint
pg-retest tune \
  --workload workload.wkl \
  --target "host=localhost port=5432 dbname=myapp user=postgres" \
  --apply \
  --max-iterations 5 \
  --hint "Focus on memory settings and missing indexes for the orders table"
```

### Quick Reference

| Flag | Default | Description |
|------|---------|-------------|
| `--workload <PATH>` | _(required)_ | Workload profile to replay during each iteration. |
| `--target <CONNSTRING>` | _(required)_ | PostgreSQL target to tune. |
| `--provider <NAME>` | `claude` | LLM provider: `claude`, `openai`, `gemini`, `bedrock`, `ollama`. |
| `--model <NAME>` | _(provider default)_ | Override the default model for the chosen provider. |
| `--api-key <KEY>` | _(env var)_ | API key (falls back to provider-specific env var). |
| `--max-iterations <N>` | `3` | Maximum tuning iterations to run. |
| `--hint <TEXT>` | _(none)_ | Natural-language hint to guide the LLM's focus. |
| `--apply` | `false` | Apply recommendations. Without this flag, dry-run mode is active. |
| `--force` | `false` | Bypass the production hostname safety check. |
| `--json <PATH>` | _(none)_ | Write the tuning report to a JSON file. |

## How It Works

Each tuning iteration follows this loop:

```
Collect PG context  →  LLM recommendations  →  Safety validation
        ↑                                               ↓
  (next iteration)                              Apply changes
        |                                               ↓
  Auto-rollback if              Replay workload against target
  p95 regresses >5%   ←              ↓
                          Compare replay vs. baseline
```

**Baseline collection:** Before the first iteration, the tuner replays the workload against the unmodified target to establish a performance baseline. All iteration comparisons are made against this baseline — not against the original captured timings — ensuring changes are evaluated fairly against the actual target environment.

**Context collection:** At the start of each iteration, the tuner queries the target for:

- `pg_settings` — current configuration parameters
- Schema information (tables, indexes, column types)
- `pg_stat_statements` — top queries by total time (if the extension is installed)
- `EXPLAIN` plans for SELECT queries in the workload (queries with bind parameters are skipped)

If `pg_stat_statements` is not installed, that portion of the context is omitted. The LLM still produces recommendations from schema and workload data alone.

## Recommendation Types

| Type | Mechanism | Rollback |
|------|-----------|---------|
| **Config Change** | `ALTER SYSTEM SET param = value` + `pg_reload_conf()` | `ALTER SYSTEM RESET param` + `pg_reload_conf()` |
| **Index Creation** | `CREATE INDEX` (supports `CONCURRENTLY`) | `DROP INDEX index_name` |
| **Query Rewrite** | Informational only — no changes applied | N/A |
| **Schema Change** | DDL split on semicolons, executed per-statement | Manual — no automatic rollback |

Query rewrites are printed as suggestions. Use them to update your application queries manually.

## Safety Features

### Dry-Run by Default

Without `--apply`, the tuner collects context, gets recommendations, validates them, and prints a report — without modifying the target database. Dry-run mode is safe to run against any instance.

### Parameter Allowlist

Config changes are validated against an allowlist of approximately 50 PostgreSQL parameters that are safe to modify at runtime without risk of data loss or corruption. This includes memory parameters (`shared_buffers`, `work_mem`, `effective_cache_size`), planner settings (`random_page_cost`, `enable_seqscan`), autovacuum tuning, and checkpoint settings.

Parameters not on the allowlist are blocked and reported as safety violations. This prevents the LLM from recommending changes to parameters that could cause instability, data corruption, or require a restart in unexpected ways.

### API Key Validation

For `claude`, `openai`, and `gemini` providers, the tuner validates that the API key is present at startup (via `--api-key` or the corresponding environment variable). If the key is missing, the command exits immediately with an error message rather than failing mid-iteration after context collection.

### SchemaChange Safety

Schema change recommendations are restricted to a safe subset of DDL: `CREATE INDEX`, `ANALYZE`, and `REINDEX`. Any other DDL (e.g., `ALTER TABLE`, `DROP`, `CREATE TABLE`) is rejected with an explanation of why it was blocked. This prevents the LLM from recommending destructive schema modifications.

### SQL Value Escaping

Config change values applied via `ALTER SYSTEM SET` are properly escaped to prevent SQL injection. String values are single-quoted with internal single quotes doubled.

### Production Hostname Check

The tuner refuses to apply changes to targets whose hostname contains `prod`, `production`, `primary`, `master`, or `main`. Pass `--force` to override:

```bash
# Override the production hostname check
pg-retest tune \
  --workload workload.wkl \
  --target "host=prod-db.example.com dbname=myapp" \
  --apply \
  --force
```

### Auto-Rollback on Regression

After each applied iteration, if p95 latency regresses by more than 5% compared to baseline, the tuner automatically rolls back:

- Config changes via `ALTER SYSTEM RESET <param>` + `pg_reload_conf()`
- Indexes via `DROP INDEX`

Rollback events appear in the terminal output, JSON report, and web dashboard event stream.

## LLM Providers

| Provider | Default Model | API Key |
|----------|--------------|---------|
| `claude` (default) | `claude-sonnet-4-20250514` | `ANTHROPIC_API_KEY` |
| `openai` | `gpt-4o` | `OPENAI_API_KEY` |
| `gemini` | `gemini-2.5-flash` | `GEMINI_API_KEY` |
| `bedrock` | `us.anthropic.claude-sonnet-4-20250514-v1:0` | _(AWS credentials)_ |
| `ollama` | `llama3` | _(none — local)_ |

All providers have a 120-second LLM request timeout.

Set the API key via environment variable or the `--api-key` flag (`--api-key` takes priority):

```bash
# Via environment variable
export ANTHROPIC_API_KEY="sk-ant-..."
pg-retest tune --workload workload.wkl --target "..." --provider claude

# Via flag
pg-retest tune --workload workload.wkl --target "..." --provider claude \
  --api-key "sk-ant-..."
```

**Bedrock** uses the `aws` CLI subprocess and standard AWS credentials (environment variables, profiles, or IAM roles). The `aws` CLI must be installed and configured. No additional API key is needed.

**Ollama** runs entirely on-premises with no external API calls. Run `ollama pull llama3` before using. For best recommendation quality, use a model with at least 13B parameters.

## CLI Reference

```
pg-retest tune [OPTIONS] --workload <PATH> --target <CONNSTRING>
```

### Required Arguments

| Flag | Description |
|------|-------------|
| `--workload <PATH>` | Workload profile (`.wkl` file) to replay during each iteration. |
| `--target <CONNSTRING>` | PostgreSQL connection string. Format: `"host=... port=... dbname=... user=... password=..."` or a `postgresql://` URI. |

### Optional Arguments

| Flag | Default | Description |
|------|---------|-------------|
| `--provider <NAME>` | `claude` | LLM provider: `claude`, `openai`, `gemini`, `bedrock`, `ollama`. |
| `--model <NAME>` | _(provider default)_ | Override the default model for the chosen provider. |
| `--api-key <KEY>` | _(env var)_ | API key. Takes priority over the corresponding environment variable. |
| `--max-iterations <N>` | `3` | Maximum tuning iterations. Each iteration is one full collect → recommend → apply → replay → compare cycle. |
| `--hint <TEXT>` | _(none)_ | Natural-language hint to guide the LLM. Example: `"slow analytical queries on the reporting schema"`. |
| `--apply` | `false` | Apply recommendations. Without this flag, dry-run mode is active. |
| `--force` | `false` | Bypass the production hostname safety check. |
| `--json <PATH>` | _(none)_ | Write the full tuning report to a JSON file. Terminal report is still printed. |
| `--target-env <VAR>` | _(none)_ | Read the target connection string from the named environment variable instead of `--target`. |
| `--tls-mode <MODE>` | `prefer` | TLS mode for the target connection: `disable`, `prefer`, or `require`. |
| `--tls-ca-cert <PATH>` | _(none)_ | Path to a custom CA certificate file for TLS verification. |

### Global Options

| Flag | Description |
|------|-------------|
| `-v, --verbose` | Enable debug-level logging (`RUST_LOG=debug`). |

## Web Dashboard

The tuning feature is available at `http://localhost:8080/tuning.html`. The web UI provides:

- A form to launch a tuning session (provider, model, API key, iterations, hint).
- Real-time progress via WebSocket, showing each iteration's recommendations, applied changes, and comparison results as they happen.
- A history view of all past tuning sessions with expandable recommendation details, applied/failed/dry-run badges, and p50/p95/p99 change statistics.

Tuning sessions run as background tasks and persist to the SQLite `tuning_reports` table. See the [web dashboard guide](web-dashboard.md) for starting the web server.

## Examples

### 1. Dry-Run to See Recommendations

```bash
# Safe to run against any target — no changes are made
pg-retest tune \
  --workload workload.wkl \
  --target "host=staging-db.internal dbname=myapp user=postgres" \
  --provider claude \
  --max-iterations 1
```

The output shows each recommendation with its rationale. Review the recommendations and decide which to apply, then re-run with `--apply`.

### 2. Apply with Auto-Rollback

```bash
pg-retest tune \
  --workload workload.wkl \
  --target "host=staging-db.internal dbname=myapp user=postgres" \
  --provider claude \
  --apply \
  --max-iterations 3
```

The tuner applies changes after each iteration and automatically rolls back anything that causes a p95 regression greater than 5%. The final report shows which changes were kept and which were reverted.

### 3. Multi-Iteration Tuning with Hint

```bash
pg-retest tune \
  --workload workload.wkl \
  --target "host=staging-db.internal dbname=myapp user=postgres" \
  --provider openai \
  --model gpt-4o \
  --apply \
  --max-iterations 5 \
  --hint "Primarily analytical workload with large aggregations on orders and
          line_items. Suspect missing indexes and suboptimal sort memory." \
  --json tuning-report.json
```

With multiple iterations, the tuner re-collects context before each LLM call so the model sees changes already applied. This allows increasingly targeted recommendations as the database state improves.

### 4. Local Tuning with Ollama

```bash
# Pull a model first (one-time setup)
ollama pull llama3

pg-retest tune \
  --workload workload.wkl \
  --target "host=localhost dbname=myapp user=postgres" \
  --provider ollama \
  --model llama3 \
  --apply \
  --max-iterations 2 \
  --hint "OLTP workload with high write throughput. Focus on autovacuum and WAL settings."
```

Ollama runs entirely on-premises — no data leaves the machine. Use this for air-gapped environments or where sending database schema to an external API is not acceptable.

## Tuning Report

After all iterations complete, the tuner prints a summary:

```
Tuning Report
=============

Iterations run:      3
Changes applied:     5
Changes rolled back: 1

Baseline p50:  1.2ms   Final p50:  0.9ms   (-25.0%)
Baseline p95:  8.4ms   Final p95:  5.1ms   (-39.3%)
Baseline p99: 22.1ms   Final p99: 14.7ms   (-33.5%)

Applied Changes:
  [OK]          shared_buffers = '2GB'
  [OK]          work_mem = '64MB'
  [OK]          CREATE INDEX idx_orders_customer_status ON orders(customer_id, status)
  [OK]          CREATE INDEX idx_line_items_order_id ON line_items(order_id)
  [ROLLED BACK] effective_cache_size = '16GB'  (p95 regressed 8.2%)
```

The `--json` report includes per-iteration results, all recommendations with rationale, apply/rollback status, and full comparison statistics. Reports are also persisted to SQLite and appear in the web dashboard history view.
