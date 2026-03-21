# Workload Transform Guide

pg-retest can reshape a captured workload into a new one without re-running against a live database. The transform feature is useful for simulating traffic patterns you haven't captured yet: Black Friday spikes, new feature rollouts, increased analytical load, or anything that differs from your baseline. This guide covers the three-step workflow, the TOML plan format, CLI flags, and the web dashboard wizard.

## Quick Start

```bash
# Step 1 — analyze the workload (no AI, no network calls)
pg-retest transform analyze --workload workload.wkl

# Step 2 — generate a TOML transform plan using AI
pg-retest transform plan \
  --workload workload.wkl \
  --prompt "Simulate Black Friday: 10x product catalog reads, add checkout stress" \
  --provider claude \
  --output black-friday.toml

# Step 3 — apply the plan to produce a new workload file
pg-retest transform apply \
  --workload workload.wkl \
  --plan black-friday.toml \
  --output black-friday.wkl
```

The resulting `black-friday.wkl` is a standard workload profile. Replay it with `pg-retest replay` exactly as you would any other captured workload.

## 3-Layer Architecture

Transform is split into three independent layers. Each layer has a clear contract with the next, and you can enter or exit the pipeline at any point.

```
WorkloadProfile (.wkl)
        |
[ Layer 1: Analyzer ]    -- deterministic, no AI
        |
  WorkloadAnalysis (JSON)
        |
[ Layer 2: Planner ]     -- AI-powered, multi-provider LLM
        |
  TransformPlan (.toml)  -- human-readable, editable
        |
[ Layer 3: Engine ]      -- deterministic, seeded RNG
        |
WorkloadProfile (.wkl)
```

**Analyzer** — Reads the profile and groups queries by shared table access using regex-based table extraction and a Union-Find algorithm. Two tables are placed in the same group when they appear together in a single SQL statement. Tables that only appear in separate queries are not grouped unless a third query references both.

**Planner** — Sends the workload analysis plus your natural language prompt to an LLM. The LLM returns a structured `TransformPlan` in TOML. The plan is saved to disk so you can review and edit it before applying. The LLM is only called once.

**Engine** — Reads the transform plan and applies it to the original workload using a seeded RNG. The same plan with the same seed always produces identical output. The default seed is derived from the plan's prompt string hash.

## Analyze

`transform analyze` reads a workload profile and prints the query groups it identified. No API key or network access required.

```bash
# Human-readable summary
pg-retest transform analyze --workload workload.wkl

# JSON output (useful for scripting)
pg-retest transform analyze --workload workload.wkl --json
```

The JSON output contains `profile_summary` (query count, session count, capture duration, source host), `query_groups` (one entry per identified group with tables, query kinds, average duration, sample SQL, sessions, and filter column patterns), and `ungrouped_queries` (queries whose tables could not be extracted).

Run `analyze` before generating a plan to understand what groups the LLM will see and what names to reference in your prompt.

## Plan

`transform plan` calls an LLM to produce a TOML transform plan.

```bash
pg-retest transform plan \
  --workload workload.wkl \
  --prompt "Scale order queries 5x to simulate holiday load" \
  --provider claude \
  --output holiday-plan.toml
```

### Providers

| Provider | Flag value | Credential |
|----------|-----------|------------|
| Anthropic Claude | `claude` | `ANTHROPIC_API_KEY` or `--api-key` |
| OpenAI | `openai` | `OPENAI_API_KEY` or `--api-key` |
| Google Gemini | `gemini` | `GEMINI_API_KEY` or `--api-key` |
| AWS Bedrock | `bedrock` | Standard AWS credentials (env/profile/IAM) |
| Ollama (local) | `ollama` | No key required |

API key resolution order: `--api-key` flag → `ANTHROPIC_API_KEY` → `OPENAI_API_KEY` → `GEMINI_API_KEY`. Request timeouts: 30s for Claude and OpenAI; 60s for Gemini and Bedrock.

### Dry Run

`--dry-run` prints the analyzer output and the full system prompt the LLM would receive, without making any API call. Use this to verify the analyzer found the right groups before using API quota.

```bash
pg-retest transform plan \
  --workload workload.wkl \
  --prompt "Double the reporting queries" \
  --dry-run
```

## Transform Plan Format

Plans are TOML files. They are human-readable and editable — adjust scale factors, change SQL, or add rules before running `apply`.

```toml
version = 1

[source]
profile = "workload.wkl"
prompt = "Simulate Black Friday: 10x product catalog reads, add checkout stress"

[analysis]
total_queries = 4820
total_sessions = 38
groups_identified = 4

[[groups]]
name = "product_catalog"
description = "Product browsing and search queries"
tables = ["products", "categories", "product_images"]
query_indices = [0, 1, 5, 10, 22]
session_ids = [1, 3, 7]
query_count = 1800

[[groups]]
name = "checkout"
description = "Cart and order creation queries"
tables = ["carts", "orders", "order_items"]
query_indices = [2, 6, 11]
session_ids = [2, 5]
query_count = 620

[[transforms]]
type = "scale"
group = "product_catalog"
factor = 10.0
stagger_ms = 50

[[transforms]]
type = "inject"
description = "Add coupon lookup to checkout flow"
sql = "SELECT * FROM coupons WHERE code = $1 AND expires_at > NOW()"
after_group = "checkout"
frequency = 0.6
estimated_duration_us = 2000

[[transforms]]
type = "inject_session"
description = "Background inventory sync job"
repeat = 5
interval_ms = 2000
queries = [
  { sql = "SELECT id, stock FROM products WHERE updated_at > $1", duration_us = 15000 },
  { sql = "UPDATE products SET stock_synced_at = NOW() WHERE id = $1", duration_us = 3000 },
]

[[transforms]]
type = "remove"
group = "legacy_reporting"
```

### Transform Rule Types

| Type | `type` tag | Effect |
|------|-----------|--------|
| Scale | `scale` | Duplicates all sessions in the group by `factor`. `stagger_ms` spreads copies over time to avoid thundering-herd spikes. |
| Inject | `inject` | Inserts a single query into existing sessions after the named group. `frequency` (0.0–1.0) controls the fraction of sessions that receive it. Seeded RNG selects which sessions. |
| InjectSession | `inject_session` | Creates entirely new standalone sessions with the specified query list. `repeat` controls how many copies; `interval_ms` sets timing between queries within each session. |
| Remove | `remove` | Drops all queries in the named group. The group's `query_indices` field identifies which queries are removed; an empty `query_indices` removes nothing. |

The `type` field uses snake_case: `scale`, `inject`, `inject_session`, `remove`.

## Apply

`transform apply` produces a new workload profile from the original and the TOML plan.

```bash
pg-retest transform apply \
  --workload workload.wkl \
  --plan black-friday.toml \
  --output black-friday.wkl
```

Override the RNG seed for explicit control over reproducibility:

```bash
pg-retest transform apply \
  --workload workload.wkl \
  --plan black-friday.toml \
  --seed 42 \
  --output black-friday-seed42.wkl
```

After `apply`, replay the result normally:

```bash
pg-retest replay \
  --workload black-friday.wkl \
  --target "host=staging-db dbname=myapp_test" \
  --output results.wkl

pg-retest compare --source workload.wkl --replay results.wkl
```

## CLI Reference

### `transform analyze`

| Flag | Default | Description |
|------|---------|-------------|
| `--workload <PATH>` | _(required)_ | Input workload profile (`.wkl`). |
| `--json` | `false` | Output analysis as JSON. |
| `--output-format <FMT>` | `text` | Output format: `text` (human-readable summary) or `json` (structured JSON to stdout). Equivalent to `--json` when set to `json`. |

### `transform plan`

| Flag | Default | Description |
|------|---------|-------------|
| `--workload <PATH>` | _(required)_ | Input workload profile (`.wkl`). |
| `--prompt <TEXT>` | _(required)_ | Natural language scenario description. |
| `--provider <NAME>` | `claude` | LLM provider: `claude`, `openai`, `gemini`, `bedrock`, `ollama`. |
| `--api-key <KEY>` | _(from env)_ | API key. Falls back to `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`. |
| `--api-url <URL>` | _(provider default)_ | Override API endpoint (useful for proxies or self-hosted models). |
| `--model <NAME>` | _(provider default)_ | Override model name. |
| `-o, --output <PATH>` | `transform-plan.toml` | Path for the generated TOML plan. |
| `--dry-run` | `false` | Print analyzer output and system prompt; skip the API call. |

### `transform apply`

| Flag | Default | Description |
|------|---------|-------------|
| `--workload <PATH>` | _(required)_ | Input workload profile (`.wkl`). |
| `--plan <PATH>` | _(required)_ | TOML transform plan. |
| `-o, --output <PATH>` | `transformed.wkl` | Path for the output workload profile. |
| `--seed <N>` | _(from prompt hash)_ | RNG seed for reproducible injection. |

## Web Dashboard

The web dashboard (`pg-retest web --port 8080`) includes a three-step transform wizard under the **Transform** page.

1. **Analyze** — Select a workload from your uploaded profiles. The dashboard displays the query group breakdown including tables, query counts, and session IDs.
2. **Plan** — Enter a prompt and select a provider. The generated TOML plan appears in an editable text area so you can adjust it before proceeding.
3. **Apply** — Click Apply to produce the transformed workload. It is saved to the server's data directory and immediately available for replay from the Workloads page.

All three steps are also available via the CLI for scripted or CI/CD workflows.

## Examples

### Analyze and Inspect Groups

```bash
pg-retest transform analyze --workload prod-capture.wkl --json \
  | jq '.query_groups[] | {tables, query_count, pct_of_total}'
```

### Black Friday Simulation (full workflow)

```bash
export ANTHROPIC_API_KEY=sk-ant-...

# Generate plan
pg-retest transform plan \
  --workload baseline.wkl \
  --prompt "Black Friday: 10x product catalog, 5x checkout, remove admin reporting" \
  --output black-friday.toml

# Review, edit if needed, then apply
pg-retest transform apply \
  --workload baseline.wkl \
  --plan black-friday.toml \
  --output black-friday.wkl

# Restore DB and replay
pg_restore -d myapp_test backup.dump
pg-retest replay \
  --workload black-friday.wkl \
  --target "host=staging-db dbname=myapp_test" \
  --output bf-results.wkl
pg-retest compare --source baseline.wkl --replay bf-results.wkl
```

### Preview LLM Input Without Making an API Call

```bash
pg-retest transform plan \
  --workload workload.wkl \
  --prompt "Double the transactional write load" \
  --dry-run
```

### Local Model with Ollama

```bash
ollama serve &
ollama pull llama3

pg-retest transform plan \
  --workload workload.wkl \
  --prompt "Scale order processing sessions 3x" \
  --provider ollama \
  --model llama3 \
  --output local-plan.toml
```
