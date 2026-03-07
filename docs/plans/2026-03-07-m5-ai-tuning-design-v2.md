# M5: AI-Assisted Tuning Design (v2)

**Date:** 2026-03-07
**Status:** Approved
**Supersedes:** `2026-03-04-m5-ai-tuning-design.md`

## Problem

Users capture and replay workloads to validate performance, but optimizing the database itself is manual: a DBA examines slow queries, adjusts config, adds indexes, rewrites SQL, tests again. This cycle is time-consuming and requires deep PG expertise. An AI-assisted tuner automates this loop: collect database context, generate recommendations, apply them, replay the workload, measure improvement, and iterate.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Context source | Direct PG connection | Simpler workflow, always fresh data after each iteration |
| Apply method | Direct SQL execution | Works with Docker test DBs, safety layer prevents dangerous ops |
| LLM providers | Multi-provider (Claude/OpenAI/Ollama) | Reuses transform infrastructure, consistent UX, no vendor lock-in |
| Recommendation scope | All four types (config, indexes, query rewrites, schema) | Full coverage for comprehensive tuning |
| Loop design | Configurable auto-loop with user hints | Fix→test→fix→test with LLM feedback from previous iterations |
| Safety | Allowlist + dry-run default | Safe params only, production hostname protection, --apply required |
| Interface | Both CLI and Web | Consistent with all other features |
| Architecture | Monolithic TuningOrchestrator | Sequential loop, simple to debug, extract traits later if needed |

## Architecture

```
User: --workload captured.wkl --target postgresql://... --hint "focus on reads"
        │
        ▼
┌──────────────────┐
│  TUNING LOOP     │  for iteration in 1..=max_iterations
│  (mod.rs)        │
└────────┬─────────┘
         │
    ┌────┴─────────────────────────────────────────────┐
    │                                                   │
    ▼                                                   │
┌──────────────────┐                                    │
│  1. CONTEXT       │  Connect to PG, collect:          │
│  (context.rs)     │  pg_settings, schema, stats,      │
│                   │  pg_stat_statements, EXPLAIN       │
└────────┬─────────┘                                    │
         │  PgContext                                   │
         ▼                                              │
┌──────────────────┐                                    │
│  2. ADVISOR       │  LLM call with context +          │
│  (advisor.rs)     │  workload + hint + history         │
│                   │  → Vec<Recommendation>             │
└────────┬─────────┘                                    │
         │                                              │
         ▼                                              │
┌──────────────────┐                                    │
│  3. SAFETY        │  Validate against allowlist,       │
│  (safety.rs)      │  block dangerous ops               │
└────────┬─────────┘                                    │
         │  Filtered recommendations                    │
         ▼                                              │
┌──────────────────┐                                    │
│  4. APPLY         │  Execute SET / CREATE INDEX /      │
│  (recommendation  │  ALTER TABLE on target DB           │
│   .rs)            │                                     │
└────────┬─────────┘                                    │
         │                                              │
         ▼                                              │
┌──────────────────┐                                    │
│  5. REPLAY        │  Reuse existing replay engine      │
│  (replay/)        │  against modified target            │
└────────┬─────────┘                                    │
         │  ReplayResults                               │
         ▼                                              │
┌──────────────────┐                                    │
│  6. COMPARE       │  Reuse existing compare module     │
│  (compare/)       │  → ComparisonReport                │
└────────┬─────────┘                                    │
         │                                              │
         ▼                                              │
┌──────────────────┐                                    │
│  7. FEEDBACK      │  Summarize iteration results       │
│  (report.rs)      │  for next LLM call                 │
└────────┬─────────┘                                    │
         │                                              │
         └──────────────────────────────────────────────┘
                    Feed back into next iteration

Final output: TuningReport (JSON)
```

## Data Model

### Recommendations

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Recommendation {
    ConfigChange {
        parameter: String,
        current_value: String,
        recommended_value: String,
        rationale: String,
    },
    CreateIndex {
        table: String,
        columns: Vec<String>,
        index_type: Option<String>,  // btree, hash, gin, gist
        sql: String,
        rationale: String,
    },
    QueryRewrite {
        original_sql: String,
        rewritten_sql: String,
        rationale: String,
    },
    SchemaChange {
        sql: String,
        description: String,
        rationale: String,
    },
}
```

### Applied Change Tracking

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppliedChange {
    pub recommendation: Recommendation,
    pub success: bool,
    pub error: Option<String>,
    pub rollback_sql: Option<String>,  // e.g. DROP INDEX for created indexes
}
```

### Iteration & Report

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningIteration {
    pub iteration: u32,
    pub recommendations: Vec<Recommendation>,
    pub applied: Vec<AppliedChange>,
    pub comparison: Option<ComparisonSummary>,
    pub llm_feedback: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComparisonSummary {
    pub p50_change_pct: f64,
    pub p95_change_pct: f64,
    pub p99_change_pct: f64,
    pub regressions: usize,
    pub improvements: usize,
    pub errors_delta: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TuningReport {
    pub workload: String,
    pub target: String,
    pub provider: String,
    pub hint: Option<String>,
    pub iterations: Vec<TuningIteration>,
    pub total_improvement_pct: f64,
    pub all_changes: Vec<AppliedChange>,
}
```

## PG Context Collection

**Module:** `src/tuner/context.rs`

Connects to the target PG database and collects introspection data for the LLM.

### Data Sources

| Source | Query | Purpose |
|--------|-------|---------|
| PG version | `SHOW server_version` | Version-specific recommendations |
| Non-default settings | `SELECT name, setting, unit, source FROM pg_settings WHERE source != 'default'` | Current config |
| Hardware hints | `SHOW shared_buffers; SHOW effective_cache_size; SHOW max_connections` | Sizing context |
| Top slow queries | Top-N from workload profile by `duration_us` | Focus optimization targets |
| pg_stat_statements | `SELECT query, calls, mean_exec_time, ... ORDER BY total_exec_time DESC LIMIT 20` | Runtime query stats (if extension available) |
| Schema | `information_schema.tables`, `information_schema.columns`, `pg_indexes` | Table structure |
| Index usage | `pg_stat_user_indexes` (idx_scan, idx_tup_read, idx_tup_fetch) | Unused/underused indexes |
| Table stats | `pg_stat_user_tables` (seq_scan, idx_scan, n_dead_tup, last_vacuum) | Scan patterns, maintenance state |
| EXPLAIN plans | `EXPLAIN (FORMAT JSON, COSTS, BUFFERS) <query>` for top-N slow queries | Plan analysis |

### PgContext Structure

```rust
pub struct PgContext {
    pub pg_version: String,
    pub non_default_settings: Vec<PgSetting>,
    pub top_slow_queries: Vec<SlowQuery>,
    pub stat_statements: Option<Vec<StatStatement>>,  // None if extension not installed
    pub schema: Vec<TableSchema>,
    pub index_usage: Vec<IndexUsage>,
    pub table_stats: Vec<TableStats>,
    pub explain_plans: Vec<ExplainPlan>,
}
```

**Size budget:** ~5-10KB JSON for typical databases. Well within all LLM context windows.

**Slow query selection:** Cross-reference the workload profile's queries (sorted by duration_us) with the database. Take the top 10 distinct normalized queries and run EXPLAIN for each.

## LLM Integration

**Module:** `src/tuner/advisor.rs`

### Trait

```rust
#[async_trait]
pub trait TuningAdvisor: Send + Sync {
    async fn recommend(
        &self,
        context: &PgContext,
        workload_summary: &WorkloadAnalysis,
        hint: Option<&str>,
        previous_iterations: &[TuningIteration],
    ) -> Result<Vec<Recommendation>>;
    fn name(&self) -> &str;
}
```

### Providers

Reuses `LlmProvider` enum and `PlannerConfig` from `transform/planner.rs`:

| Provider | API | Structured Output |
|----------|-----|-------------------|
| `ClaudeAdvisor` | `api.anthropic.com/v1/messages` | tool_use (4 tools) |
| `OpenAiAdvisor` | `api.openai.com/v1/chat/completions` | function_calling |
| `OllamaAdvisor` | `localhost:11434/api/generate` | JSON mode |

All providers use direct HTTP via `reqwest`. Same prompt template, different response parsing.

### Prompt Structure

```
System: You are a PostgreSQL tuning expert. Given a database's current
configuration, schema, query performance, and workload patterns, recommend
changes to improve performance.

You have four tools:
1. config_change(parameter, current_value, recommended_value, rationale)
2. create_index(table, columns, index_type, sql, rationale)
3. query_rewrite(original_sql, rewritten_sql, rationale)
4. schema_change(sql, description, rationale)

User:
## Database Context
{PgContext JSON}

## Workload Summary
{WorkloadAnalysis — query groups, patterns, latency distribution}

## User Hint
{optional --hint value}

## Previous Iterations
{summarized results from each prior iteration}

## Instructions
- Prioritize changes with highest expected impact
- Consider previous iteration results — don't repeat ineffective changes
- For config changes, specify parameter, current, and recommended values
- For indexes, provide the full CREATE INDEX statement
- For query rewrites, show original and optimized SQL
- Use parameter patterns from the workload to inform index column choices
```

### Cost/Latency

- Input: ~5-10K tokens (context + workload + history)
- Output: ~1-3K tokens (recommendations)
- Latency: 5-15 seconds per iteration
- Cost: ~$0.03-0.10 per iteration (Sonnet-class pricing)

## Tuning Loop

**Module:** `src/tuner/mod.rs`

```rust
pub struct TuningConfig {
    pub workload_path: PathBuf,
    pub target: String,
    pub provider: LlmProvider,
    pub api_key: Option<String>,
    pub api_url: Option<String>,
    pub model: Option<String>,
    pub max_iterations: u32,
    pub hint: Option<String>,
    pub apply: bool,         // false = dry-run (default)
    pub force: bool,         // allow production hostnames
    pub speed: f64,          // replay speed multiplier
    pub read_only: bool,     // replay SELECT only
}
```

### Loop Flow

```
1. Load workload profile
2. Analyze workload (reuse transform/analyze)
3. Validate target (safety: production hostname check)
4. Collect baseline: replay workload, store as iteration 0

for iteration in 1..=max_iterations:
    5. Collect PG context from target
    6. Call LLM: context + workload + hint + previous iterations
    7. Validate recommendations (safety layer)
    8. If dry-run: print recommendations, stop
    9. Apply recommendations to target DB
    10. Replay workload against modified target
    11. Compare vs. baseline (or vs. previous iteration)
    12. Build feedback string from comparison
    13. If regression or no improvement: stop early
    14. Store iteration results
    15. Broadcast WebSocket progress

16. Generate TuningReport
17. Print summary / write JSON
```

### Early Stop Conditions

- p95 latency worsened vs. previous iteration
- LLM returned zero recommendations
- All recommendations rejected by safety layer
- User cancelled (CancellationToken via web/CLI signal)

### Baseline Collection

Before any tuning, replay the workload once to establish baseline metrics. This ensures comparison is always against the same starting point, not against the captured workload's original timing (which may have been on different hardware).

## Safety Layer

**Module:** `src/tuner/safety.rs`

### Config Parameter Allowlist

~50 parameters that are safe to change at runtime via `SET` or `ALTER SYSTEM`:

```rust
const SAFE_CONFIG_PARAMS: &[&str] = &[
    // Memory
    "shared_buffers", "work_mem", "maintenance_work_mem",
    "effective_cache_size", "temp_buffers", "huge_pages",
    // Planner
    "random_page_cost", "seq_page_cost", "cpu_tuple_cost",
    "cpu_index_tuple_cost", "cpu_operator_cost",
    "default_statistics_target", "enable_seqscan",
    "enable_indexscan", "enable_bitmapscan", "enable_hashjoin",
    "enable_mergejoin", "enable_nestloop", "enable_hashagg",
    "enable_material", "enable_sort",
    // Parallelism
    "max_parallel_workers_per_gather", "max_parallel_workers",
    "max_parallel_maintenance_workers", "parallel_tuple_cost",
    "parallel_setup_cost", "min_parallel_table_scan_size",
    "min_parallel_index_scan_size",
    // WAL & Checkpoint
    "checkpoint_completion_target", "wal_buffers",
    "commit_delay", "commit_siblings",
    // JIT
    "jit", "jit_above_cost", "jit_inline_above_cost",
    "jit_optimize_above_cost",
    // Logging (safe)
    "log_min_duration_statement", "log_statement",
    // Autovacuum
    "autovacuum_vacuum_scale_factor", "autovacuum_analyze_scale_factor",
    "autovacuum_vacuum_cost_delay", "autovacuum_vacuum_cost_limit",
];
```

### Blocked Operations

- Any `DROP` statement (database, table, index, column, schema)
- `TRUNCATE`
- `ALTER TABLE ... DROP COLUMN`
- `DELETE FROM` (without WHERE or on system tables)
- Config parameters not in the allowlist
- Changes to: `data_directory`, `listen_addresses`, `port`, `hba_file`, `pg_hba_file`, `ssl_cert_file`, `ssl_key_file`, `password_encryption`

### Production Hostname Protection

If the target connection string matches any of these patterns, refuse without `--force`:
- Contains `prod`, `production`, `primary`, `master`, `main`
- Port 5432 on a non-localhost host

### Dry-Run Default

The `--apply` flag must be explicitly passed to execute recommendations. Without it, the tuner:
1. Collects context
2. Calls LLM
3. Prints recommendations with rationale
4. Exits without modifying anything

## CLI Interface

```bash
# Dry-run (default): show recommendations only
pg-retest tune \
  --workload captured.wkl \
  --target "postgresql://localhost/testdb" \
  --provider claude \
  --api-key $ANTHROPIC_API_KEY \
  --max-iterations 3 \
  --hint "focus on read latency, don't change shared_buffers"

# Apply mode: execute the full loop
pg-retest tune \
  --workload captured.wkl \
  --target "postgresql://localhost/testdb" \
  --apply \
  --max-iterations 5 \
  --json tuning-report.json

# With Ollama (local, no API key needed)
pg-retest tune \
  --workload captured.wkl \
  --target "postgresql://localhost/testdb" \
  --provider ollama \
  --model llama3 \
  --apply
```

### Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--workload` | required | Path to workload profile (.wkl) |
| `--target` | required | Target PG connection string |
| `--provider` | `claude` | LLM provider: claude, openai, ollama |
| `--api-key` | env var | API key (or ANTHROPIC_API_KEY/OPENAI_API_KEY) |
| `--api-url` | provider default | Override API endpoint |
| `--model` | provider default | Override model name |
| `--max-iterations` | `3` | Maximum tuning iterations |
| `--hint` | none | Natural language guidance for the LLM |
| `--apply` | false | Execute recommendations (default is dry-run) |
| `--force` | false | Allow targeting production-looking hostnames |
| `--json` | none | Output JSON report path |
| `--speed` | `1.0` | Replay speed multiplier |
| `--read-only` | false | Replay only SELECTs |

## Web Dashboard

### Tuning Page

1. **Configuration panel** — Select workload, enter target connection string, configure LLM provider/key/model, enter optional hint, set max iterations
2. **Start/Cancel buttons** — Launches tuning as background task
3. **Iteration timeline** — Each iteration shown as a card:
   - Recommendations listed with type badges (Config, Index, Query, Schema)
   - Applied status (success/failure per change)
   - Before/after metrics (p50, p95, p99, regressions)
   - Improvement percentage badge
4. **Final summary** — Total improvement, all changes applied, recommendation to keep or rollback

### API Endpoints

```
POST /api/v1/tuning/start     { workload_id, target, provider, api_key, hint, max_iterations, apply }
GET  /api/v1/tuning/status/:id  → { status, iterations, report }
POST /api/v1/tuning/cancel/:id
```

### WebSocket Messages

```rust
TuningIterationStarted { task_id, iteration },
TuningRecommendations { task_id, iteration, recommendations: Vec<Recommendation> },
TuningChangeApplied { task_id, iteration, change: AppliedChange },
TuningReplayCompleted { task_id, iteration, improvement_pct },
TuningCompleted { task_id, total_improvement_pct, iterations_completed },
```

## Modules

| Module | Purpose |
|--------|---------|
| `src/tuner/mod.rs` | TuningOrchestrator — owns the loop, TuningConfig, public API |
| `src/tuner/context.rs` | PgContext collector — connect, introspect, build context |
| `src/tuner/recommendation.rs` | Recommendation enum, AppliedChange, application logic (execute SQL) |
| `src/tuner/advisor.rs` | TuningAdvisor trait + ClaudeAdvisor/OpenAiAdvisor/OllamaAdvisor |
| `src/tuner/safety.rs` | Allowlist, blocked ops, production hostname check, validation |
| `src/tuner/report.rs` | TuningReport, TuningIteration, ComparisonSummary, output formatting |
| `src/web/handlers/tuning.rs` | Web API endpoints (start, status, cancel) |
| `src/web/static/js/pages/tuning.js` | Web UI page |

## Reused Infrastructure

| Component | Source | How Used |
|-----------|--------|----------|
| LlmProvider enum | `transform/planner.rs` | Provider selection, config pattern |
| HTTP patterns | `transform/planner.rs` | reqwest client, timeout, error handling |
| WorkloadAnalysis | `transform/analyze.rs` | Workload summary for LLM context |
| run_replay() | `replay/mod.rs` | Replay workload after each iteration |
| ComparisonReport | `compare/mod.rs` | Measure improvement per iteration |
| TaskManager | `web/tasks.rs` | Background task management for web |
| WsMessage broadcast | `web/ws.rs` | Real-time iteration progress |
| AppState pattern | `web/state.rs` | Web handler state sharing |

## New Dependencies

None required. All needed crates are already in Cargo.toml:
- `reqwest` (HTTP for LLM APIs)
- `tokio-postgres` (PG introspection connection)
- `serde` + `serde_json` (data serialization)
- `chrono` (timestamps)

## Testing Strategy

| Test Type | What | How |
|-----------|------|-----|
| Context unit tests | PgContext construction, query generation | Mock PG connection or test against Docker PG |
| Safety unit tests | Allowlist validation, blocked op detection, hostname check | Pure logic tests with sample recommendations |
| Advisor integration | LLM request/response format | Mock HTTP server with canned responses |
| Recommendation unit tests | SQL generation, application, rollback SQL | Test each recommendation type in isolation |
| Report unit tests | ComparisonSummary computation, TuningReport serialization | Known inputs → expected outputs |
| Loop integration | Full iteration cycle | Docker PG + mock LLM → verify recommendations applied + comparison computed |
| CLI integration | `tune --dry-run` end-to-end | Docker PG + mock LLM → verify output format |

## Future Extensions (Not in This Design)

- **Rollback command**: Undo all changes from a tuning session
- **Recommendation history**: Compare tuning sessions across time
- **Heuristic-based advisor**: Non-LLM rule-based recommendations (e.g., "shared_buffers should be 25% of RAM")
- **Multi-database tuning**: Tune multiple databases with the same workload simultaneously
- **Continuous tuning**: Monitor production and suggest changes proactively
