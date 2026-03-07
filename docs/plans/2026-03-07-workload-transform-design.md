# AI-Powered Workload Transform Design

**Date:** 2026-03-07
**Status:** Approved

## Problem

Users capture real PostgreSQL workloads, but need to simulate scenarios that don't exist yet: traffic spikes to specific features, new code releases, decommissioned endpoints. The existing scaling infrastructure (`scale_sessions`, `scale_sessions_by_class`) operates at session or workload-class level, which is too coarse. Users need to say "5x product catalog traffic and add review queries" and get a realistic benchmark workload.

## Design Decisions

| Decision | Choice | Rationale |
|----------|--------|-----------|
| Output format | New `.wkl` file (not live stream) | Reproducible, inspectable, works with existing replay/compare pipeline |
| Query grouping | Hybrid: deterministic table extraction + AI labeling | Tables provide structure, AI provides semantic interpretation |
| New query injection | Yes, without schema validation | Simulates unreleased features; AI generates SQL from context |
| LLM integration | Multi-provider (Claude, OpenAI, Ollama) | Maximizes adoption, avoids vendor lock-in |
| Interface | Both CLI and Web UI | Covers both DBA/ops and dashboard users |
| Review step | Two-step: AI generates plan, user reviews, then applies | Auditable, editable, reproducible |
| Mixed-session handling | Weighted session duplication | Preserves transactions and connection state |

## Architecture

Three layers with a structured plan as the contract between AI and engine:

```
User Prompt + .wkl file
        │
        ▼
┌──────────────────┐
│   1. ANALYZER     │  Deterministic: extract tables, group queries,
│   (analyze.rs)    │  compute stats, extract parameter patterns
└────────┬─────────┘
         │  WorkloadAnalysis (JSON)
         ▼
┌──────────────────┐
│   2. PLANNER      │  AI-powered: interpret prompt, map intent to
│   (planner.rs)    │  groups, generate scaling + injection rules
└────────┬─────────┘
         │  TransformPlan (TOML)  ← human reviews/edits here
         ▼
┌──────────────────┐
│   3. ENGINE       │  Deterministic: apply plan to source profile,
│   (engine.rs)     │  produce new .wkl file
└────────┬─────────┘
         │
         ▼
    Modified .wkl → replay → compare
```

The plan file is the critical intermediate artifact. It captures every AI decision as frozen, auditable configuration. Running the same plan against the same source profile always produces identical output.

## Layer 1: Analyzer

**Module:** `src/transform/analyze.rs`

Reads a `.wkl` profile and produces a `WorkloadAnalysis` — structured data for the AI planner.

### Table Extraction

Lightweight regex-based SQL parsing (consistent with existing `transform::mysql_to_pg` approach):
- `FROM\s+(\w+)` — SELECT source tables
- `JOIN\s+(\w+)` — joined tables
- `INTO\s+(\w+)` — INSERT targets
- `UPDATE\s+(\w+)` — UPDATE targets
- `DELETE\s+FROM\s+(\w+)` — DELETE targets

### Query Grouping

Union-Find algorithm on table names:
1. Each query produces a set of table names
2. Tables that appear together in a query are merged into the same group
3. If query A touches `[products, categories]` and query B touches `[categories, inventory]`, they merge via shared `categories`
4. Queries with no extractable tables (SET, transaction control) go to an "ungrouped" catch-all

### Parameter Pattern Extraction

For each query group:
- Extract bind parameter positions (`$1`, `$2`, ...)
- Sample first 3 distinct literal values per position (masked if PII masking was enabled)
- Identify common WHERE clause filter columns
- Compute cardinality hints (distinct value count per parameter position)

### Output: WorkloadAnalysis

```json
{
  "profile_summary": {
    "total_queries": 325,
    "total_sessions": 5,
    "capture_duration_s": 120.5,
    "source_host": "localhost:5432"
  },
  "query_groups": [
    {
      "id": 0,
      "tables": ["products", "categories", "inventory"],
      "query_count": 127,
      "sample_queries": ["SELECT * FROM products WHERE category_id = $1", "..."],
      "kinds": { "Select": 120, "Update": 7 },
      "avg_duration_us": 3200,
      "sessions": [1, 3],
      "pct_of_total": 39.1,
      "parameter_patterns": {
        "common_filters": ["category_id", "product_id"],
        "sample_values": { "$1": ["42", "17", "103"] }
      }
    }
  ],
  "ungrouped_queries": 12
}
```

Designed to be 2-5KB for typical workloads — small enough for any LLM context window.

## Layer 2: AI Planner

**Module:** `src/transform/planner.rs`

### LLM Abstraction

```rust
#[async_trait]
pub trait LlmPlanner: Send + Sync {
    async fn generate_plan(
        &self,
        analysis: &WorkloadAnalysis,
        prompt: &str,
    ) -> Result<TransformPlan>;

    fn name(&self) -> &str;
}
```

### Built-in Providers

| Provider | Endpoint | Structured Output |
|----------|----------|-------------------|
| `ClaudePlanner` | `api.anthropic.com/v1/messages` | tool_use |
| `OpenAiPlanner` | `api.openai.com/v1/chat/completions` | function_calling |
| `OllamaPlanner` | `localhost:11434/api/generate` | JSON mode |

All providers use direct HTTP (no SDK dependencies). Same prompt template, different response parsing. Each implementation is ~100 lines.

### Prompt Structure

```
System: You are a PostgreSQL workload planning assistant. Given a captured
workload analysis and a user's scenario description, generate a transform
plan that modifies the workload to simulate the described scenario.

[Tool definition: generate_transform_plan with full schema]

User:
## Workload Analysis
{analyzer JSON output — includes groups, stats, parameter patterns}

## User Scenario
{user's natural language prompt}

## Instructions
- Map the user's intent to the identified query groups
- Assign scaling factors to groups that should change
- Generate SQL for any new queries the scenario requires
- Use parameter patterns from the analysis to generate realistic SQL
- Preserve groups not mentioned in the scenario at scale 1.0
- Provide human-readable descriptions for each group and transform
```

### Cost/Latency

- Input: ~3-5K tokens (analysis + prompt + tool schema)
- Output: ~1-2K tokens (plan)
- Latency: 3-8 seconds
- Cost: ~$0.02-0.05 per plan (Sonnet-class pricing)

### Error Handling

- Invalid plan (references nonexistent group): validate and return error
- API key missing: skip AI, offer manual plan creation
- API timeout: 30 second deadline
- Rate limit: surface error to user with retry suggestion

## Transform Plan Format

**File format:** TOML (human-readable, editable)

```toml
version = 1

[source]
profile = "sales-demo-proxy.wkl"
prompt = "5x product catalog traffic, add review queries"

[analysis]
total_queries = 325
total_sessions = 5
groups_identified = 4

# Query groups identified by analyzer + labeled by AI
[[groups]]
name = "product_catalog"
description = "Product browsing and search queries"
tables = ["products", "categories", "inventory"]
query_indices = [3, 7, 12, 15, 22, 31]
session_ids = [1, 3]
query_count = 127

[[groups]]
name = "checkout"
description = "Cart and order processing"
tables = ["orders", "order_items", "payments"]
query_indices = [8, 9, 10, 25, 26]
session_ids = [2, 4]
query_count = 89

# Transform rules
[[transforms]]
type = "scale"
group = "product_catalog"
factor = 5.0
stagger_ms = 10

[[transforms]]
type = "scale"
group = "checkout"
factor = 1.0

[[transforms]]
type = "inject"
description = "Product review lookups on product page views"
sql = "SELECT r.rating, r.text, u.name FROM product_reviews r JOIN users u ON r.user_id = u.id WHERE r.product_id = $1 ORDER BY r.created_at DESC LIMIT 10"
after_group = "product_catalog"
frequency = 0.8
estimated_duration_us = 5000

[[transforms]]
type = "inject_session"
description = "New review submission background job"
queries = [
    { sql = "INSERT INTO product_reviews (...) VALUES (...)", duration_us = 2000 },
    { sql = "UPDATE products SET avg_rating = (...) WHERE id = $1", duration_us = 8000 },
]
repeat = 50
interval_ms = 2000

[[transforms]]
type = "remove"
group = "reporting"
```

### Transform Types

| Type | Purpose |
|------|---------|
| `scale` | Duplicate sessions containing group queries at given factor |
| `inject` | Insert new queries after group queries with configurable frequency |
| `inject_session` | Create entirely new sessions with specified queries |
| `remove` | Exclude a group's queries (simulate feature removal) |

### Query Indices

Reference a flattened query list: session 0 query 0, session 0 query 1, ..., session 1 query 0, etc. This provides unambiguous cross-session references.

## Layer 3: Transform Engine

**Module:** `src/transform/engine.rs`

### Operations

1. **Group assignment** — Map each query to its group via `query_indices`
2. **Session scaling** — Weighted duplication for mixed sessions
3. **Query injection** — Insert new queries after group queries (seeded RNG)
4. **Session injection** — Create new sessions spaced across capture window
5. **Query removal** — Remove group queries from sessions
6. **Timing recalculation** — Recompute `start_offset_us` after modifications
7. **Metadata update** — Recompute totals

### Mixed-Session Strategy: Weighted Duplication

For sessions touching multiple groups, compute effective scale as weighted average of group scales:

```
effective_scale = Σ(group_pct × group_scale) for all groups in session
```

Example: session is 70% product_catalog (scale 5x) + 30% checkout (scale 1x):
```
effective_scale = 0.7 × 5.0 + 0.3 × 1.0 = 3.8 → round to 4 copies
```

Duplicate the **entire session** (preserving transactions, connection state, prepared statements) by the effective scale. Original session always kept (minimum 1 copy).

### Injection Reproducibility

Frequency-based injection uses a seeded RNG:
- Seed = hash of (plan file content + source profile path)
- Same plan + same profile = identical injection decisions
- Changing the plan changes the seed, producing different (but still deterministic) results

### Output Profile

The generated `.wkl` profile:
- `capture_method` = `"transformed"`
- Source profile and plan file referenced in metadata
- All other fields (version, `captured_at`, `source_host`, `pg_version`) inherited from source

## CLI Interface

### Subcommands

```bash
# Analyze: show query groups (no AI, no API key needed)
pg-retest transform analyze --workload captured.wkl

# Plan: generate transform plan (requires API key)
pg-retest transform plan \
    --workload captured.wkl \
    --prompt "5x product catalog traffic, add review queries" \
    --provider claude \
    --api-key $ANTHROPIC_API_KEY \
    --model claude-sonnet-4-20250514 \
    --output transform-plan.toml

# Plan dry-run: show what AI would see without calling API
pg-retest transform plan --workload captured.wkl --dry-run

# Apply: execute plan to produce new workload (no API key needed)
pg-retest transform apply \
    --workload captured.wkl \
    --plan transform-plan.toml \
    --output scaled-v2.wkl \
    --seed 42
```

### Flags

| Flag | Default | Description |
|------|---------|-------------|
| `--provider` | `claude` | LLM provider: `claude`, `openai`, `ollama` |
| `--model` | Provider default | Override model (e.g., `claude-sonnet-4-20250514`, `gpt-4o`, `llama3`) |
| `--api-key` | `$ANTHROPIC_API_KEY` / `$OPENAI_API_KEY` | API authentication |
| `--api-url` | Provider default | Custom API endpoint (useful for Ollama, proxies) |
| `--seed` | Hash of plan | RNG seed for reproducible injection |
| `--dry-run` | false | Show analyzer output without calling AI |

## Web API & UI

### Endpoints

```
POST /api/v1/transform/analyze   { workload_id }
  → { analysis: WorkloadAnalysis }

POST /api/v1/transform/plan      { workload_id, prompt, provider, api_key, model? }
  → { plan: TransformPlan }

POST /api/v1/transform/apply     { workload_id, plan }
  → { workload_id: "new-transformed-id" }
```

### Web UI Page

New "Transform" page in the dashboard:
1. **Workload selector** — dropdown of imported workloads
2. **Analysis view** — query groups as cards (tables, query count, sample SQL, parameter patterns)
3. **Prompt box** — textarea for natural language scenario description
4. **Provider config** — provider dropdown, API key input, model override
5. **Plan review** — generated transform rules displayed as editable form (sliders for scale factors, toggle injections, edit SQL)
6. **Apply button** — produces new `.wkl`, appears in workloads list
7. **Quick actions** — links to replay or compare the transformed workload

## Testing Strategy

| Test Type | What | How |
|-----------|------|-----|
| Analyzer unit tests | Table extraction, Union-Find grouping, parameter patterns | Hand-crafted profiles with known SQL |
| Engine unit tests | Scaling, injection, removal, timing, metadata | Known plan + profile → expected output |
| Plan serde tests | TOML round-trip | Serialize/deserialize TransformPlan |
| Planner integration | HTTP request/response | Mock HTTP server with canned AI responses |
| CLI integration | `transform analyze` end-to-end | Test `.wkl` file → verify output |
| Reproducibility | Same plan + profile = identical .wkl | Byte-for-byte comparison |
| Mixed-session | Weighted duplication correctness | Profile with multi-group sessions |

## New Dependencies

| Crate | Purpose | Feature |
|-------|---------|---------|
| `reqwest` | HTTP client for LLM APIs | `json` feature |
| `rand` | Seeded RNG for injection frequency | `std_rng` |
| `toml` | Transform plan serialization | (already may be in deps for pipeline config) |

## Future Extensions (Not in This Design)

- **Live stream mode**: Generate and replay on the fly without intermediate `.wkl`
- **Temporal shaping**: Time-of-day traffic variation (peak hours, ramp up/down)
- **Schema-validated injection**: Query target DB schema before generating injected SQL
- **Iterative refinement**: Multi-turn AI conversation to refine the plan
- **Plan templates**: Pre-built plans for common scenarios (traffic spike, feature rollout, migration)
