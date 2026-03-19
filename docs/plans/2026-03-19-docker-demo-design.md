# Docker Demo Environment & README Use Cases Design

**Date:** 2026-03-19
**Status:** Approved

## Overview

Three coordinated additions to pg-retest:

1. **README hero use cases** — A visually prominent section at the top of README.md showcasing 8 real-world use cases
2. **Docker demo environment** — `docker compose up` gives users a fully working pg-retest instance with two PostgreSQL databases and a pre-built e-commerce workload
3. **Web dashboard demo page** — A "Demo" nav item (visible only in demo mode) with a guided wizard for first-time users and scenario cards for exploration

## 1. README Hero Use Cases

### Placement

Immediately after the tagline (line 10), before the Quick Start section.

### Content

Eight use cases presented as a hero grid:

| Use Case | Description |
|----------|-------------|
| **Pre-Migration Validation** | Replay production traffic against your new datacenter, hardware, or cloud target before cutting over. Know it works — don't hope. |
| **Version & Patch Testing** | Upgrading PostgreSQL 15 → 16? Replay your exact workload against the new version and catch regressions before they hit production. |
| **Configuration Benchmarking** | Changed `shared_buffers` or `work_mem`? Compare before and after with real queries, not synthetic benchmarks. |
| **Cloud Provider Evaluation** | RDS vs. Aurora vs. AlloyDB vs. self-hosted — replay identical traffic against each and let the numbers decide. |
| **Capacity Planning** | Scale your workload 2x, 5x, 10x to find where things break — before Black Friday finds it for you. |
| **CI/CD Regression Gates** | Automated pass/fail on every schema migration or config change. Catch performance regressions in the pipeline, not in production. |
| **Cross-Database Migration** | Moving from MySQL to PostgreSQL? Capture your MySQL workload, transform the SQL, and validate it runs correctly on PG. |
| **AI-Assisted Optimization** | Get LLM-powered tuning recommendations — then validate every change against your real workload with automatic rollback on regression. |

### Format

Markdown table styled as cards with emoji icons and bold titles. Each row has a 1-sentence punchy description.

## 2. Docker Demo Environment

### Files

| File | Purpose |
|------|---------|
| `Dockerfile` | Multi-stage build: `rust:1.75-bookworm` builder → `debian:bookworm-slim` runtime |
| `docker-compose.yml` | Three services: pg-retest, db-a, db-b |
| `demo/init-db-a.sql` | E-commerce schema + seed data for Database A |
| `demo/init-db-b.sql` | E-commerce schema + same seed data for Database B (identical to db-a for meaningful replay comparison) |
| `demo/workload.wkl` | Pre-built 2-minute workload (committed binary) |
| `demo/generate-workload.sh` | Script to regenerate the demo workload (dev use only) |

### E-Commerce Schema

```sql
customers    (id SERIAL PK, name TEXT, email TEXT UNIQUE, created_at TIMESTAMPTZ)     ~5,000 rows
products     (id SERIAL PK, name TEXT, category TEXT, price NUMERIC, stock INT)        ~1,000 rows
orders       (id SERIAL PK, customer_id FK, total NUMERIC, status TEXT, created_at TIMESTAMPTZ)  ~20,000 rows
order_items  (id SERIAL PK, order_id FK, product_id FK, qty INT, price NUMERIC)       ~60,000 rows
reviews      (id SERIAL PK, product_id FK, customer_id FK, rating INT, body TEXT)     ~8,000 rows
```

Indexes on all foreign keys plus composite indexes on:
- `orders(customer_id, created_at)`
- `orders(status, created_at)`
- `order_items(product_id, qty)`
- `reviews(product_id, rating)`

### Docker Compose

```yaml
services:
  db-a:
    image: postgres:16
    environment:
      POSTGRES_DB: ecommerce
      POSTGRES_USER: demo
      POSTGRES_PASSWORD: demo
    volumes:
      - ./demo/init-db-a.sql:/docker-entrypoint-initdb.d/init.sql
    ports:
      - "5450:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U demo -d ecommerce"]
      interval: 5s
      timeout: 5s
      retries: 5

  db-b:
    image: postgres:16
    environment:
      POSTGRES_DB: ecommerce
      POSTGRES_USER: demo
      POSTGRES_PASSWORD: demo
    volumes:
      - ./demo/init-db-b.sql:/docker-entrypoint-initdb.d/init.sql
    ports:
      - "5451:5432"
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U demo -d ecommerce"]
      interval: 5s
      timeout: 5s
      retries: 5

  pg-retest:
    build: .
    command: ["pg-retest", "web", "--port", "8080", "--data-dir", "/data"]
    environment:
      PG_RETEST_DEMO: "true"
      DEMO_DB_A: "host=db-a dbname=ecommerce user=demo password=demo"
      DEMO_DB_B: "host=db-b dbname=ecommerce user=demo password=demo"
      DEMO_WORKLOAD: "/demo/workload.wkl"
    ports:
      - "8080:8080"
    volumes:
      - ./demo:/demo:ro
      - pgdata:/data
    depends_on:
      db-a:
        condition: service_healthy
      db-b:
        condition: service_healthy

volumes:
  pgdata:
```

### Dockerfile

```dockerfile
# Stage 1: Build
FROM rust:1.75-bookworm AS builder
WORKDIR /src
COPY . .
RUN cargo build --release

# Stage 2: Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y libssl3 ca-certificates && rm -rf /var/lib/apt/lists/*
COPY --from=builder /src/target/release/pg-retest /usr/local/bin/pg-retest
RUN mkdir -p /data/workloads
EXPOSE 8080
ENTRYPOINT ["pg-retest"]
```

### Demo Workload Profile

The `demo/workload.wkl` contains ~2 minutes of mixed traffic across 8-10 concurrent sessions:

- **Analytical sessions (2-3):** `SELECT` joins across orders/order_items/products with aggregations, date range filters, GROUP BY category
- **Transactional sessions (3-4):** `INSERT` into orders + order_items within BEGIN/COMMIT transactions, `UPDATE` product stock
- **Mixed sessions (2):** Customer lookups with recent order history (SELECT + light writes)
- **Bulk session (1):** Batch `UPDATE` on order statuses (e.g., "pending" → "shipped")

This provides classification variety so scaled benchmark and per-category scaling are meaningful.

### Workload Generation Script

`demo/generate-workload.sh`:
1. Starts a temporary PG container with `init-db-a.sql`
2. Starts pg-retest proxy against it
3. Runs a scripted workload via 8-10 backgrounded `psql` processes (one per session, running concurrently with `&` and `wait`) to produce genuinely parallel sessions with overlapping timestamps
4. Stops proxy → captures `demo/workload.wkl`
5. Tears down the temporary container

Each `psql` process runs a session-specific SQL script (e.g., `demo/sessions/analytical-1.sql`, `demo/sessions/transactional-1.sql`) through the proxy. This ensures the captured `.wkl` has realistic concurrent session behavior.

This is a developer tool — run once, commit the resulting `.wkl`. Users never need to run it.

### Demo Mode Detection

Environment variable `PG_RETEST_DEMO=true` enables demo mode. The web server reads:

| Env Var | Purpose |
|---------|---------|
| `PG_RETEST_DEMO` | Enables demo mode when set to `"true"` |
| `DEMO_DB_A` | Connection string for Database A (source) |
| `DEMO_DB_B` | Connection string for Database B (target) |
| `DEMO_WORKLOAD` | Path to the demo `.wkl` file (e.g., `/demo/workload.wkl`) |

## 3. Web Dashboard Demo Page

### Detection & Nav

On startup, if `PG_RETEST_DEMO=true`:
1. Parse `DEMO_DB_A` and `DEMO_DB_B` env vars
2. Store as `Option<DemoConfig>` in `AppState`
3. Register `/api/v1/demo/` routes
4. Frontend `/api/v1/demo/config` response includes `enabled: true` → sidebar shows "Demo" nav item

### Page Layout

Two sections on a single page:

#### Top: Guided Wizard

A horizontal 5-step stepper:

| Step | Title | Action | What Happens |
|------|-------|--------|-------------|
| 1 | Explore | Inspect workload | Loads the demo `.wkl`, shows session count, query count, classification breakdown |
| 2 | Replay | Replay against DB-B | Replays the demo workload against Database B, shows progress via WebSocket |
| 3 | Compare | Compare results | Runs comparison between source and replay, shows latency diffs and regressions |
| 4 | Scale | Capacity test | Replays at 3x scale (analytical 2x, transactional 4x), shows capacity report |
| 5 | AI Tune | Run tuning advisor | Runs tuning advisor (dry-run) against DB-B, shows recommendations. Prompts for LLM API key if not set. |

Each step:
- Shows a brief explanation of what's happening
- Has a single "Run Step" action button
- Shows results inline (stats, tables, charts)
- Unlocks the next step on completion
- Can be re-run independently

**DB-B Reset:** A "Reset Database B" button is available at the top of the wizard. It re-runs `init-db-b.sql` against DB-B to restore it to the original seeded state. This is needed before re-running the wizard since Steps 2 and 4 execute writes that mutate DB-B's data. The reset is implemented via `POST /api/v1/demo/reset-db` which reads the init SQL from the mounted `/demo/init-db-b.sql` and executes it against DB-B.

#### Bottom: Scenario Cards

Four cards in a 2×2 grid:

| Card | Description | Internal Flow |
|------|-------------|---------------|
| **Migration Test** | Replay workload from DB-A schema against DB-B, compare | replay → compare |
| **Capacity Planning** | Replay at 3x scale with per-category breakdown | replay (scaled) → capacity report |
| **A/B Comparison** | Compare DB-A vs DB-B with identical workload | ab test with 2 variants |
| **AI Tuning** | Run tuning advisor against DB-B (requires API key) | tune (dry-run) |

Each card shows:
- Status badge: Ready / Running / Complete
- "Run" and "Reset" buttons
- Expandable results section after completion

### API Endpoints

```
GET  /api/v1/demo/config              — Returns { enabled, db_a_host, db_b_host }
POST /api/v1/demo/reset-db            — Reset DB-B to initial seeded state
POST /api/v1/demo/wizard/:step        — Run wizard step (1-5)
GET  /api/v1/demo/wizard/:step        — Get step status and results
POST /api/v1/demo/scenario/:name      — Run a scenario (migration, capacity, ab, tuning)
GET  /api/v1/demo/scenario/:name      — Get scenario status and results
```

### Rust Changes

**New types** in `src/web/demo.rs`:

```rust
pub struct DemoConfig {
    pub db_a: String,
    pub db_b: String,
    pub workload_path: PathBuf,
    pub init_sql_path: PathBuf,
}
```

Added as `Option<DemoConfig>` to `AppState`.

**Initialization flow:**
1. `run_server()` in `src/web/mod.rs` reads `PG_RETEST_DEMO` env var on startup
2. If set to `"true"`, reads `DEMO_DB_A`, `DEMO_DB_B`, and `DEMO_WORKLOAD` env vars
3. Constructs `Some(DemoConfig { ... })` and passes to `AppState::new()`
4. On first startup, auto-imports the demo workload into SQLite's `workloads` table (so replay/compare handlers can find it by UUID via the normal `db::get_workload()` path)

No changes to `src/cli` or `src/main.rs` needed — the env var reading stays within the web module.

**New handler module:** `src/web/handlers/demo.rs`

Handlers are thin orchestrators that call existing internal functions:
- `get_config()` → returns `{ enabled, db_a_host, db_b_host }` (hostnames only, no full connection strings)
- `reset_db()` → reads `/demo/init-db-b.sql`, executes against DB-B to restore initial state
- `run_wizard_step(step=1)` → calls inspect/classify logic
- `run_wizard_step(step=2)` → calls replay engine with pre-filled target
- `run_wizard_step(step=3)` → calls compare logic
- `run_wizard_step(step=4)` → calls replay with scaling params → capacity report
- `run_wizard_step(step=5)` → calls tuner (dry-run)

Step/scenario state stored in-memory in `AppState` (reset on restart — acceptable for demo).

**Route registration** in `src/web/routes.rs`:
- Demo routes are always registered under `/api/v1/demo/`
- Handlers check `state.demo_config.is_some()` and return `404 Not Found` with `{ "error": "Demo mode not enabled" }` when disabled
- This avoids conditional routing complexity in Axum and matches the existing handler pattern

**Frontend changes:**
- New file `src/web/static/js/pages/demo.js` following Alpine.js component pattern
- `src/web/static/js/app.js` — add `demo` to `navItems` (conditionally shown based on `/api/v1/demo/config` response)
- `src/web/static/index.html` — add `x-show="page === 'demo'"` section

### What Does NOT Change

- No changes to core capture/replay/compare/tune logic
- No changes to existing web pages or their handlers
- No changes to CLI subcommands
- The demo page is purely additive
- Existing tests unaffected

## 4. Implementation Notes

### Workload Generation Strategy

The demo workload needs to be realistic enough to showcase:
- Multiple concurrent sessions (8-10) for parallel replay
- Transaction boundaries (BEGIN/COMMIT) for transaction-aware replay
- Query variety for classification (analytical, transactional, mixed, bulk)
- ~2 minutes duration for manageable demo runs
- Diverse query patterns for interesting AI tuning recommendations

### Docker Build Considerations

- Multi-stage build keeps the final image small (~50MB + binary)
- `rust-embed` means static files are compiled into the binary — no file mounting needed for the web UI
- The `demo/` directory is mounted read-only at `/demo` — separate from the `/data` named volume to avoid volume precedence conflicts
- The demo `.wkl` is auto-imported into SQLite on first startup, so it's accessible via the normal workload UUID lookup path
- Health checks on databases ensure pg-retest starts only after PG is ready
- DB-B is seeded with the same data as DB-A (not empty) so replayed queries return realistic results for meaningful comparison

### Frontend Demo Page Considerations

- The demo page follows the existing dark industrial theme
- WebSocket integration for real-time progress on replay/tuning steps
- Chart.js for results visualization (reuses existing chart components)
- API key input for AI tuning step (stored in browser session only, never persisted)
