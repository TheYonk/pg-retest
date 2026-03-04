# SQL Gateway: Universal Agent Data Access Layer — Design

**Date:** 2026-03-04
**Status:** Draft (future product)
**Scope:** Standalone proxy/gateway product between AI agents and any database
**Lineage:** Evolves from `pg-retest proxy` (capture proxy with session pooling)

---

## Vision

A standalone proxy/gateway that sits between AI agents and any database. Agents never connect to databases directly. They connect to the gateway, ask for data (natural language or governed SQL), and the gateway figures out where the data lives, generates dialect-specific SQL, enforces governance, and returns results.

The gateway is not dumb plumbing. It has a brain — a central PostgreSQL instance that stores everything it learns: schemas, data profiles, query patterns, semantic mappings, agent behavior, and governance rules. It gets smarter over time by crawling databases, observing queries, sampling data, and learning from every interaction.

```
                         ┌─────────────────────────────────────┐
                         │          AI Agents                   │
                         │  (Claude, GPT, custom, LangChain)   │
                         └────────────┬────────────────────────┘
                                      │ NL or governed SQL
                                      ▼
┌──────────────────────────────────────────────────────────────────────┐
│                          SQL GATEWAY                                  │
│                                                                       │
│  ┌──────────┐  ┌───────────┐  ┌───────────┐  ┌───────────────────┐  │
│  │ Agent    │  │ Semantic  │  │ Query     │  │ Governance        │  │
│  │ API      │  │ Router    │  │ Generator │  │ Engine            │  │
│  │ (NL+SQL) │→ │ (where is │→ │ (PG/MySQL/│→ │ (validate, log,  │  │
│  │          │  │  the data)│  │ MSSQL/Ora)│  │  learn)           │  │
│  └──────────┘  └───────────┘  └───────────┘  └───────────────────┘  │
│                                                                       │
│  ┌────────────────────────────────────────────────────────────────┐  │
│  │                    THE BRAIN (PostgreSQL)                       │  │
│  │  Schema maps | Data profiles | Query patterns | Agent profiles │  │
│  │  Semantic layer | Whitelist | Access logs | Learned knowledge  │  │
│  └────────────────────────────────────────────────────────────────┘  │
│                                                                       │
│  ┌──────────┐  ┌───────────┐  ┌───────────┐  ┌───────────────────┐  │
│  │ Schema   │  │ Query     │  │ Data      │  │ Relationship      │  │
│  │ Crawler  │  │ Observer  │  │ Sampler   │  │ Mapper            │  │
│  │ (async)  │  │ (async)   │  │ (async)   │  │ (async)           │  │
│  └────┬─────┘  └─────┬─────┘  └─────┬─────┘  └──────────┬────────┘  │
└───────┼───────────────┼──────────────┼───────────────────┼───────────┘
        │               │              │                   │
        ▼               ▼              ▼                   ▼
   ┌─────────┐   ┌─────────┐   ┌──────────┐   ┌──────────────┐
   │PostgreSQL│   │  MySQL  │   │SQL Server│   │    Oracle    │
   └─────────┘   └─────────┘   └──────────┘   └──────────────┘
```

## Why a Gateway, Not a Database Extension

| Dimension | PostgreSQL Extension | SQL Gateway |
|-----------|---------------------|-------------|
| Database scope | One database | Any number of databases, any engine |
| Agent connection | Agents must know which DB to connect to | Agents connect to one place |
| Schema knowledge | Manual allowlists | Auto-discovered via crawling |
| Learning source | Own access logs only | Existing production query traffic |
| SQL dialect | PG only | Generates PG, MySQL, MSSQL, Oracle SQL |
| Deployment | Per-DB extension install/upgrade | Standalone service, deploy once |
| Cross-database | Not possible | Federates queries across databases |
| Credentials | Agents hold DB credentials | Agents never see DB credentials |

---

## System 1: The Knowledge Crawlers

Four async background processes that continuously build the gateway's understanding of connected databases. They run on configurable schedules (not on the query hot path) and write everything to The Brain.

### 1a. Schema Crawler

Connects to each registered data source and discovers structure.

**Per data source:**
1. Query `information_schema` (or engine equivalent) for databases, schemas, tables, columns, data types, nullability, defaults, primary keys, foreign keys, unique constraints, indexes, views, stored procedures
2. Per table: record column names/types/constraints, detect PII columns by name pattern (email, phone, ssn), detect date/time columns, record row count estimates and data size
3. Store in Brain: unified schema representation normalized across engines, change detection (new/modified/deleted since last crawl), crawl timestamp for staleness

**Engine-specific discovery:**

| Database | Schema Source | Extras |
|----------|-------------|--------|
| PostgreSQL | `information_schema` + `pg_catalog` | RLS policies, extensions, partitioning, inheritance |
| MySQL | `information_schema` + `SHOW CREATE TABLE` | Engine type (InnoDB/MyISAM), charset, partitioning |
| SQL Server | `INFORMATION_SCHEMA` + `sys.*` catalog views | Schemas, filegroups, computed columns, temporal tables |
| Oracle | `ALL_TAB_COLUMNS`, `ALL_CONSTRAINTS`, `ALL_INDEXES` | Tablespaces, materialized views, DB links |

### 1b. Query Observer

Taps into query traffic on each connected database to learn what queries people and applications actually run.

**Per data source:**

| Database | Query Stats Source |
|----------|-------------------|
| PostgreSQL | `pg_stat_statements` — top queries by frequency and duration |
| MySQL | `performance_schema.events_statements_summary_by_digest` |
| SQL Server | `sys.dm_exec_query_stats` + `sys.dm_exec_sql_text`, or Query Store |
| Oracle | `V$SQL` / `V$SQLAREA`, AWR snapshots for historical patterns |

**Per observed query:**
1. Normalize (strip literals → `$N`)
2. Parse into AST: extract tables, columns, joins, aggregations
3. Classify: reporting, OLTP lookup, ETL, ad-hoc
4. Detect common patterns:
   - "Everyone joins orders + customers on customer_id"
   - "Revenue queries always filter by date and group by region"
   - "This table is always accessed with WHERE tenant_id = ..."
5. Store pattern + frequency + performance stats in Brain

**What this enables:**
- Gateway knows the canonical way to query for revenue data (it's seen it 10K times)
- When an agent asks for something similar, generate SQL that matches proven patterns
- Queries that look nothing like anything seen before get flagged as unusual
- Gateway can suggest whitelist entries based on most common real-world queries

### 1c. Data Sampler

Inspects actual data (small samples) to understand what's in the tables — content profile, not just schema definition.

**Per table (configurable scope):**
1. Sample N rows (default 1000):
   - PostgreSQL: `TABLESAMPLE SYSTEM(percent)`
   - MySQL: `ORDER BY RAND() LIMIT N`
   - SQL Server: `TABLESAMPLE(N ROWS)`
   - Oracle: `SAMPLE(N)`
2. Per column, compute: cardinality, NULL percentage, value distribution (top N for low-cardinality), min/max for numerics/dates, average length for text, pattern detection (emails, phones, UUIDs)
3. Detect implicit relationships: column name matching across tables, value overlap analysis
4. Store in Brain: column-level statistics, detected relationships, PII confidence scores, data freshness indicators

**Why sampling matters:**
- Schema says `status VARCHAR(50)`. Sampling reveals it's always one of: `active`, `inactive`, `pending`, `suspended` — effectively an enum
- Two tables both have `region` columns. Sampling reveals they share the same values — implicit relationship
- PII detection by content: a column named `notes` might contain email addresses
- Agent asks for "active customers" — gateway knows `status = 'active'` is the right filter

### 1d. Relationship Mapper

Combines schema, query patterns, and data sampling to build a complete semantic map.

**Inputs:**
- Foreign key constraints (from schema crawler)
- Implicit relationships (from data sampler — value overlap)
- Join patterns (from query observer — what tables are joined and on what)
- Naming conventions (column name similarity)

**Output (stored in Brain):**
- **Entity graph:** which tables represent the same entity across databases
- **Join graph:** how to get from table A to table B (direct or multi-hop)
- **Cross-database relationships:** customer in PG <-> subscription in MySQL
- **Canonical entity definitions:** "customer" means `customers` table in PG joined with `subscriptions` in MySQL on `customer_id`
- **Confidence scores:** FK = 1.0, query pattern = 0.8, name match + value overlap = 0.6

---

## System 2: The Brain (Central PostgreSQL Instance)

A dedicated PostgreSQL instance that stores everything the gateway knows. This is the system of record — not a cache.

**Why PostgreSQL:**
- `pgvector` for embedding storage and semantic search
- `JSONB` for flexible metadata storage
- Full SQL for complex analytical queries on learning views
- Extensions ecosystem (`pg_cron` for scheduling, `pgAudit`, etc.)

### Brain Schema

```sql
CREATE SCHEMA brain;

-- =====================================================
-- KNOWLEDGE: What the crawlers discovered
-- =====================================================

-- Connected data sources
CREATE TABLE brain.data_sources (
    id                SERIAL PRIMARY KEY,
    name              TEXT NOT NULL UNIQUE,         -- "production_pg", "analytics_mysql"
    engine            TEXT NOT NULL,                 -- postgresql, mysql, sqlserver, oracle
    host              TEXT NOT NULL,
    port              INTEGER NOT NULL,
    database_name     TEXT NOT NULL,
    connection_params JSONB DEFAULT '{}',            -- SSL, pool size, timeouts
    crawl_schedule    TEXT DEFAULT '0 */6 * * *',   -- Cron expression
    last_crawled_at   TIMESTAMPTZ,
    status            TEXT DEFAULT 'active',         -- active, paused, error
    created_at        TIMESTAMPTZ DEFAULT NOW()
);

-- Unified schema catalog (normalized across all engines)
CREATE TABLE brain.catalog_tables (
    id                    SERIAL PRIMARY KEY,
    source_id             INTEGER REFERENCES brain.data_sources(id),
    schema_name           TEXT NOT NULL,
    table_name            TEXT NOT NULL,
    table_type            TEXT DEFAULT 'table',      -- table, view, materialized_view
    row_count_est         BIGINT,
    size_bytes            BIGINT,
    description           TEXT,                       -- From DB comments or generated
    description_embedding VECTOR(1536),
    last_crawled_at       TIMESTAMPTZ,
    UNIQUE(source_id, schema_name, table_name)
);

CREATE TABLE brain.catalog_columns (
    id               SERIAL PRIMARY KEY,
    table_id         INTEGER REFERENCES brain.catalog_tables(id),
    column_name      TEXT NOT NULL,
    data_type        TEXT NOT NULL,                  -- Normalized type name
    native_type      TEXT NOT NULL,                  -- Original DB-specific type
    is_nullable      BOOLEAN,
    is_primary_key   BOOLEAN DEFAULT false,
    is_foreign_key   BOOLEAN DEFAULT false,
    fk_references    JSONB,                          -- {table_id: N, column: "name"}
    pii_detected     BOOLEAN DEFAULT false,
    pii_type         TEXT,                            -- email, phone, ssn, name, address
    pii_confidence   DOUBLE PRECISION,
    description      TEXT,
    -- Data profile (from sampler)
    cardinality      BIGINT,
    null_pct         DOUBLE PRECISION,
    top_values       JSONB,                          -- [{value: "active", count: 50000}, ...]
    min_value        TEXT,
    max_value        TEXT,
    avg_length       DOUBLE PRECISION,
    sample_values    TEXT[],                          -- Redacted if PII
    last_profiled_at TIMESTAMPTZ,
    UNIQUE(table_id, column_name)
);

-- Relationships (explicit and discovered)
CREATE TABLE brain.relationships (
    id                SERIAL PRIMARY KEY,
    source_table_id   INTEGER REFERENCES brain.catalog_tables(id),
    source_column     TEXT NOT NULL,
    target_table_id   INTEGER REFERENCES brain.catalog_tables(id),
    target_column     TEXT NOT NULL,
    relationship_type TEXT NOT NULL,                 -- fk, implicit_join, value_overlap, name_match
    confidence        DOUBLE PRECISION NOT NULL,     -- 0.0 to 1.0
    evidence          JSONB,                          -- What supports this relationship
    discovered_at     TIMESTAMPTZ DEFAULT NOW()
);

-- Semantic entities (cross-database concepts)
CREATE TABLE brain.semantic_entities (
    id                    SERIAL PRIMARY KEY,
    name                  TEXT NOT NULL UNIQUE,        -- "customer", "order", "product"
    description           TEXT,
    description_embedding VECTOR(1536),
    canonical_source_id   INTEGER REFERENCES brain.data_sources(id),
    canonical_table       TEXT,                        -- Primary table for this entity
    related_tables        JSONB,                       -- [{source_id, table, join_path}]
    created_at            TIMESTAMPTZ DEFAULT NOW(),
    last_updated_at       TIMESTAMPTZ DEFAULT NOW()
);

-- Schema change tracking (for staleness alerts)
CREATE TABLE brain.schema_changelog (
    id              SERIAL PRIMARY KEY,
    source_id       INTEGER REFERENCES brain.data_sources(id),
    change_type     TEXT NOT NULL,                    -- added, modified, dropped
    affected_table  TEXT NOT NULL,
    affected_column TEXT,
    details         JSONB,
    changed_at      TIMESTAMPTZ DEFAULT NOW()
);

-- =====================================================
-- OBSERVED PATTERNS: What the query observer found
-- =====================================================

CREATE TABLE brain.observed_queries (
    id                 SERIAL PRIMARY KEY,
    source_id          INTEGER REFERENCES brain.data_sources(id),
    query_normalized   TEXT NOT NULL,
    query_hash         BYTEA NOT NULL,
    tables_accessed    TEXT[],
    columns_accessed   TEXT[],
    join_pattern       JSONB,                         -- [{left_table, right_table, join_col}]
    has_aggregation    BOOLEAN,
    has_where          BOOLEAN,
    query_type         TEXT,                          -- select, insert, update, delete
    classification     TEXT,                          -- reporting, oltp, etl, ad_hoc
    -- Execution stats
    total_calls        BIGINT,
    avg_duration_ms    DOUBLE PRECISION,
    last_seen_at       TIMESTAMPTZ,
    first_seen_at      TIMESTAMPTZ,
    -- For semantic matching
    inferred_purpose   TEXT,                          -- What this query seems to do
    purpose_embedding  VECTOR(1536),
    UNIQUE(source_id, query_hash)
);

-- =====================================================
-- GOVERNANCE: Rules, whitelists, access control
-- =====================================================

-- Agent registry
CREATE TABLE brain.agents (
    id                   SERIAL PRIMARY KEY,
    agent_id             TEXT NOT NULL UNIQUE,
    agent_name           TEXT,
    role                 TEXT NOT NULL,                -- readonly, analyst, readwrite
    allowed_sources      INTEGER[],                   -- Which data sources
    allowed_entities     TEXT[],                       -- Which semantic entities
    max_rows             INTEGER DEFAULT 1000,
    require_aggregation  BOOLEAN DEFAULT false,
    pii_access           TEXT DEFAULT 'masked',       -- masked, denied, allowed
    created_at           TIMESTAMPTZ DEFAULT NOW(),
    last_active_at       TIMESTAMPTZ
);

-- Approved queries (cross-database, NL-matchable)
CREATE TABLE brain.approved_queries (
    id                    SERIAL PRIMARY KEY,
    name                  TEXT NOT NULL UNIQUE,
    description           TEXT NOT NULL,
    description_embedding VECTOR(1536),
    target_source_id      INTEGER REFERENCES brain.data_sources(id),
    sql_template          TEXT NOT NULL,
    sql_dialect           TEXT NOT NULL,                -- postgresql, mysql, tsql, plsql
    parameters            JSONB DEFAULT '[]',
    allowed_roles         TEXT[] NOT NULL,
    max_rows              INTEGER DEFAULT 1000,
    pii_columns_masked    TEXT[] DEFAULT '{}',
    requires_aggregation  BOOLEAN DEFAULT false,
    source                TEXT DEFAULT 'manual',       -- manual, promoted, observed
    usage_count           BIGINT DEFAULT 0,
    last_used_at          TIMESTAMPTZ,
    created_by            TEXT NOT NULL,
    created_at            TIMESTAMPTZ DEFAULT NOW()
);

-- SQL whitelist (per data source, dialect-specific)
CREATE TABLE brain.sql_whitelist (
    id                     SERIAL PRIMARY KEY,
    name                   TEXT NOT NULL,
    source_id              INTEGER REFERENCES brain.data_sources(id),
    sql_normalized         TEXT NOT NULL,
    sql_hash               BYTEA NOT NULL,
    sql_dialect            TEXT NOT NULL,
    allowed_roles          TEXT[] NOT NULL,
    parameter_constraints  JSONB DEFAULT '{}',
    max_rows               INTEGER DEFAULT 1000,
    source                 TEXT DEFAULT 'manual',
    usage_count            BIGINT DEFAULT 0,
    created_at             TIMESTAMPTZ DEFAULT NOW(),
    UNIQUE(source_id, sql_hash)
);

-- Schema-level access control (per agent role, per data source)
CREATE TABLE brain.access_rules (
    id                    SERIAL PRIMARY KEY,
    role                  TEXT NOT NULL,
    source_id             INTEGER REFERENCES brain.data_sources(id),
    schema_name           TEXT NOT NULL,
    table_name            TEXT NOT NULL,
    allowed_columns       TEXT[],                     -- NULL = all non-PII
    allowed_ops           TEXT[] DEFAULT ARRAY['SELECT'],
    row_filter            TEXT,                        -- Source-dialect WHERE clause
    require_aggregation   BOOLEAN DEFAULT false,
    max_rows              INTEGER DEFAULT 100,
    UNIQUE(role, source_id, schema_name, table_name)
);

-- =====================================================
-- LOGS: Everything that happened
-- =====================================================

CREATE TABLE brain.access_log (
    id               BIGSERIAL PRIMARY KEY,
    agent_id         TEXT NOT NULL,
    agent_role       TEXT NOT NULL,
    session_id       TEXT,
    -- Request
    request_type     TEXT NOT NULL,                   -- natural_language, sql_direct
    request_text     TEXT NOT NULL,
    request_embedding VECTOR(1536),
    -- Resolution
    resolution_path  TEXT NOT NULL,                   -- approved_query, whitelist_exact,
                                                      -- whitelist_pattern, generated, denied
    matched_item     TEXT,                            -- Name of matched query/whitelist entry
    target_source_id INTEGER,
    generated_sql    TEXT,
    sql_dialect      TEXT,
    -- Result
    decision         TEXT NOT NULL,                   -- allowed, denied
    denial_reason    TEXT,
    result_row_count INTEGER,
    tables_accessed  TEXT[],
    columns_accessed TEXT[],
    -- Performance
    total_duration_ms   DOUBLE PRECISION,
    routing_ms          DOUBLE PRECISION,
    generation_ms       DOUBLE PRECISION,             -- NL-to-SQL time if applicable
    execution_ms        DOUBLE PRECISION,
    llm_ms              DOUBLE PRECISION,
    -- Context
    requested_at     TIMESTAMPTZ DEFAULT NOW(),
    client_ip        INET
);

-- Agent feedback
CREATE TABLE brain.feedback (
    id            BIGSERIAL PRIMARY KEY,
    access_log_id BIGINT REFERENCES brain.access_log(id),
    agent_id      TEXT NOT NULL,
    helpful       BOOLEAN NOT NULL,
    comment       TEXT,
    created_at    TIMESTAMPTZ DEFAULT NOW()
);
```

---

## System 3: The Semantic Router

When an agent asks for data, the router determines where the data lives and how to get it.

### Routing Flow

```
Agent: "What's the monthly churn rate by customer segment for Q4 2024?"
  │
  ▼
┌─ Step 1: Understand the request ─────────────────────────────────┐
│  Extract key concepts:                                            │
│    - Entity: "customer" → brain.semantic_entities                 │
│    - Metric: "churn rate" → subscription state changes            │
│    - Dimension: "customer segment" → segmentation data            │
│    - Filter: "Q4 2024" → time range Oct-Dec 2024                 │
│    - Aggregation: "monthly" → GROUP BY month                      │
└──────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─ Step 2: Find the data ─────────────────────────────────────────┐
│  Look up "customer" in semantic_entities:                         │
│    → canonical source: production_pg.public.customers             │
│    → related: analytics_mysql.subscriptions (join on cust_id)    │
│    → related: production_pg.public.customer_segments              │
│                                                                   │
│  Look up "churn rate" in observed_queries:                        │
│    → Closest match: query #4521 (embedding similarity: 0.91)     │
│    → That query joins customers + subscriptions                   │
│    → Uses: WHERE status_change_date BETWEEN ... AND ...           │
│            subscription_status = 'cancelled'                      │
│    → Groups by segment, month                                     │
└──────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─ Step 3: Choose resolution path ────────────────────────────────┐
│  Check approved queries → match "monthly_churn_by_segment"?      │
│    → YES: use approved query with params (Q4, 2024)              │
│    → Target: analytics_mysql                                      │
│                                                                   │
│  If no approved match:                                            │
│    Single-source? → generate SQL in target dialect                │
│    Cross-database? → generate separate queries per source,        │
│                      merge results in gateway                     │
└──────────────────────────────────────────────────────────────────┘
  │
  ▼
┌─ Step 4: Generate dialect-specific SQL ─────────────────────────┐
│  Target: MySQL (analytics_mysql)                                  │
│  Template from observed query #4521, adapted:                     │
│                                                                   │
│  SELECT                                                           │
│    cs.segment_name,                                               │
│    DATE_FORMAT(s.status_change_date, '%Y-%m') as month,           │
│    COUNT(CASE WHEN s.subscription_status = 'cancelled'            │
│               THEN 1 END) as churned,                             │
│    COUNT(*) as total,                                             │
│    ROUND(...) as churn_rate                                       │
│  FROM subscriptions s                                             │
│  JOIN customer_segments cs ON s.customer_id = cs.customer_id      │
│  WHERE s.status_change_date >= '2024-10-01'                       │
│    AND s.status_change_date < '2025-01-01'                        │
│  GROUP BY cs.segment_name, DATE_FORMAT(...)                       │
│  ORDER BY month, churn_rate DESC                                  │
│  LIMIT 1000;                                                      │
└──────────────────────────────────────────────────────────────────┘
```

### Cross-Database Query Federation

When data spans multiple databases, the gateway decomposes and federates:

```
Agent: "Show customers with their subscription status and billing totals"

Router determines:
  - customers       → production_pg.public.customers
  - subscriptions   → analytics_mysql.subscriptions
  - billing         → billing_sqlserver.dbo.invoices

Execution plan:
  1. Query PG:    SELECT id, name, email FROM customers WHERE ...
  2. Query MySQL: SELECT customer_id, status, plan FROM subscriptions
                  WHERE customer_id IN ($customer_ids_from_step_1)
  3. Query MSSQL: SELECT customer_id, SUM(amount) as total
                  FROM invoices
                  WHERE customer_id IN ($customer_ids_from_step_1)
                  GROUP BY customer_id
  4. Join results in gateway memory
  5. Apply PII masking (email → masked)
  6. Return unified result to agent
```

This is simpler than federated SQL engines. The gateway controls the fan-out, can parallelize sub-queries, and applies governance uniformly to the merged result.

---

## System 4: The SQL Generator

Generates correct, dialect-specific SQL for each target database.

### Dialect Differences That Matter

| Concept | PostgreSQL | MySQL | SQL Server | Oracle |
|---------|-----------|-------|------------|--------|
| String concat | `\|\|` | `CONCAT()` | `+` | `\|\|` |
| Date extract | `EXTRACT(month FROM d)` | `MONTH(d)` | `DATEPART(month, d)` | `EXTRACT(MONTH FROM d)` |
| Date format | `TO_CHAR(d, 'YYYY-MM')` | `DATE_FORMAT(d, '%Y-%m')` | `FORMAT(d, 'yyyy-MM')` | `TO_CHAR(d, 'YYYY-MM')` |
| Limit rows | `LIMIT N` | `LIMIT N` | `TOP N` / `OFFSET..FETCH` | `FETCH FIRST N ROWS ONLY` |
| Boolean | `TRUE/FALSE` | `1/0` or `TRUE/FALSE` | `1/0` | `1/0` |
| Upsert | `ON CONFLICT DO UPDATE` | `ON DUPLICATE KEY UPDATE` | `MERGE` | `MERGE` |
| JSON access | `->`, `->>` | `JSON_EXTRACT()`, `->` | `JSON_VALUE()` | `JSON_VALUE()` |
| Identifier quoting | `"double quotes"` | `` `backticks` `` | `[brackets]` | `"double quotes"` |

### Generation Modes (tried in order)

**Mode 1: Approved Query Template.** Pre-written SQL in the correct dialect. Fill in parameters. No generation needed. Fastest, most reliable.

**Mode 2: Adapt Observed Query.** A real query has been observed running successfully on the target database. Adapt it with the agent's parameters and filters. Produces SQL modeled after SQL that actually works.

**Mode 3: Generate from Schema.** LLM with constrained schema (only tables/columns the agent can access). System prompt includes: target engine and version, dialect-specific syntax rules, constrained schema subset, few-shot examples from observed patterns, mandatory constraints (WHERE, LIMIT, aggregation). Validate via AST parsing before execution.

### SQL Validation Per Dialect

| Database | Parser/Validator |
|----------|-----------------|
| PostgreSQL | `libpg_query` (extracted PG parser) |
| MySQL | `sqlparser` crate or vitess-based |
| SQL Server | Regex-based validation + structural checks |
| Oracle | Regex-based validation + structural checks |

**Validation checks (all dialects):**
1. Only allowed tables/columns for agent's role
2. Only allowed operations (SELECT for readonly agents)
3. Required clauses present (WHERE, LIMIT)
4. No forbidden clauses (DROP, TRUNCATE, etc.)
5. Parameters within constraints
6. Inject mandatory WHERE + LIMIT if missing

---

## System 5: The Governance Engine

### Resolution Chain

```
Agent request (NL or SQL)
  │
  ├─ 1. Approved query match?  (NL → semantic search brain.approved_queries)
  │     YES → execute template on target database
  │     NO ↓
  │
  ├─ 2. SQL whitelist match?   (SQL → hash/pattern/structural against brain.sql_whitelist)
  │     YES → validate params → execute on target
  │     NO ↓
  │
  ├─ 3. Observed query match?  (NL → match against brain.observed_queries patterns)
  │     YES → adapt observed query → validate → execute
  │     NO ↓
  │
  ├─ 4. Generate SQL?          (NL → LLM with constrained schema → validate → execute)
  │     ENABLED + PASSES → execute
  │     DISABLED or FAILS ↓
  │
  └─ 5. DENY with suggestions
        Return closest approved queries and observed patterns
```

Step 3 is unique to the gateway — it matches agent requests against real queries observed running on the database. If a reporting team runs a churn query every Monday, and an agent asks for churn data, the gateway adapts that proven query rather than generating from scratch.

### Agent Authentication

Agents authenticate to the gateway (not to databases). The gateway manages its own credential store for database connections.

```
Agent connects to gateway:
  → Authenticates via API key / mTLS / OAuth token
  → Gateway maps identity to brain.agents role
  → Role determines: allowed sources, entities, access rules, row limits

Gateway connects to databases:
  → Uses service accounts with appropriate privileges
  → Agents never see database credentials
  → Different DB credentials per data source
  → Connection pooling per data source
```

**Critical security property:** Agents never have database credentials. They can't connect directly even if they wanted to.

---

## The Learning Loop

Five feedback mechanisms, all human-reviewed before governance changes take effect.

### 1. Query Observer → Whitelist Promotion

When the query observer sees a query pattern that's been running successfully for months on production, and it matches something agents frequently ask for, suggest adding it as an approved query.

```sql
-- Candidates: observed production queries that match denied agent requests
CREATE VIEW brain.observation_to_approval_candidates AS
SELECT
    oq.source_id,
    ds.name as source_name,
    oq.query_normalized,
    oq.total_calls as production_uses,
    oq.avg_duration_ms,
    oq.inferred_purpose,
    denied.request_count as agent_demand,
    denied.sample_requests
FROM brain.observed_queries oq
JOIN brain.data_sources ds ON ds.id = oq.source_id
JOIN LATERAL (
    SELECT
        COUNT(*) as request_count,
        array_agg(DISTINCT request_text ORDER BY request_text) as sample_requests
    FROM brain.access_log al
    WHERE al.decision = 'denied'
      AND al.request_embedding <-> oq.purpose_embedding < 0.3
      AND al.requested_at > NOW() - INTERVAL '14 days'
) denied ON denied.request_count >= 3
WHERE oq.total_calls >= 100
  AND oq.classification = 'reporting'
ORDER BY denied.request_count DESC;
```

### 2. Generated Query → Whitelist Promotion

Generated queries used successfully multiple times get suggested for promotion to approved queries.

### 3. Denied Request → Gap Identification

Clusters of denied requests reveal what agents need but can't get.

### 4. Data Profile Changes → Staleness Alerts

When the schema crawler detects changes (new tables, renamed columns), flag affected approved queries and whitelist entries.

```sql
CREATE VIEW brain.stale_query_alerts AS
SELECT
    aq.name,
    aq.sql_template,
    changes.change_type,
    changes.details
FROM brain.approved_queries aq
JOIN LATERAL (
    SELECT change_type, details
    FROM brain.schema_changelog
    WHERE source_id = aq.target_source_id
      AND changed_at > aq.last_used_at
      AND affected_table = ANY(aq.tables_referenced)
) changes ON true;
```

### 5. Agent Behavior → Anomaly Detection

Profile-based anomaly detection with cross-database visibility:
- Agent suddenly querying a database it's never accessed before
- Dramatic increase in query volume
- Shift from approved queries to NL-to-SQL generated queries
- Queries at unusual times
- Sudden interest in tables with PII

---

## Agent API

Three protocol options. All share the same core logic — the API layer is just transport.

### Option A: PostgreSQL Wire Protocol

Agents connect as if connecting to PostgreSQL and call gateway functions:

```sql
-- Agent connects to gateway on port 5433
-- Authenticates with gateway credentials (not DB credentials)

-- Natural language request
SELECT * FROM gateway.ask('What is the churn rate by segment for Q4 2024?');

-- SQL request (validated against whitelist)
SELECT * FROM gateway.query(
    'SELECT segment, COUNT(*) FROM subscriptions WHERE status = $1 GROUP BY segment',
    '["cancelled"]'::jsonb
);

-- Feedback
SELECT gateway.feedback(12345, true, 'Exactly what I needed');

-- Discover available data
SELECT * FROM gateway.list_approved_queries('analyst');
SELECT * FROM gateway.list_entities();
SELECT * FROM gateway.describe_entity('customer');
```

**Advantage:** Works with any PostgreSQL client library. LangChain, psycopg2, JDBC — no changes needed.

### Option B: HTTP/REST API

```
POST /api/v1/ask
{ "request": "What is the churn rate by segment for Q4 2024?" }

POST /api/v1/query
{ "sql": "SELECT ...", "params": ["cancelled"] }

POST /api/v1/feedback
{ "request_id": 12345, "helpful": true }

GET  /api/v1/entities
GET  /api/v1/entities/customer
GET  /api/v1/approved-queries?role=analyst
```

**Advantage:** Works with any HTTP client. Language-agnostic.

### Option C: MCP Server

```json
{
    "tools": [
        {
            "name": "ask_data",
            "description": "Ask for data in natural language.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "question": {"type": "string"},
                    "context": {"type": "object"}
                },
                "required": ["question"]
            }
        },
        {
            "name": "submit_query",
            "description": "Submit SQL for validation and execution.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "sql": {"type": "string"},
                    "params": {"type": "array"}
                },
                "required": ["sql"]
            }
        },
        {
            "name": "explore_data",
            "description": "Discover available data entities and queries.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "entity": {"type": "string"}
                }
            }
        }
    ],
    "resources": [
        {
            "uri": "gateway://entities",
            "name": "Available Data Entities"
        },
        {
            "uri": "gateway://approved-queries",
            "name": "Approved Queries"
        }
    ]
}
```

**Advantage:** Native integration with Claude, GPT, and any MCP-compatible agent framework.

### Recommendation: All Three

Build the core once, expose via PG wire protocol, HTTP, and MCP. The PG wire protocol interface reuses the proxy infrastructure from `pg-retest proxy`.

---

## Deployment Architecture

```
┌──────────────────────────────────────────────────────────────────┐
│  Gateway Cluster                                                  │
│                                                                   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐           │
│  │ Gateway Node │  │ Gateway Node │  │ Gateway Node │  stateless │
│  │  (API +      │  │  (API +      │  │  (API +      │  workers,  │
│  │   Router +   │  │   Router +   │  │   Router +   │  scale     │
│  │   Generator) │  │   Generator) │  │   Generator) │  horizontally
│  └──────┬───────┘  └──────┬───────┘  └──────┬───────┘           │
│         └──────────────────┼──────────────────┘                   │
│                            │                                      │
│  ┌─────────────────────────┴──────────────────────────────────┐  │
│  │                    The Brain (PostgreSQL)                    │  │
│  │  Primary ──── Replica (read scaling for routing lookups)   │  │
│  └────────────────────────────────────────────────────────────┘  │
│                                                                   │
│  ┌──────────────┐  ┌──────────────┐  ┌──────────────┐           │
│  │ Schema       │  │ Query        │  │ Data         │  background│
│  │ Crawler      │  │ Observer     │  │ Sampler      │  workers,  │
│  │ Worker       │  │ Worker       │  │ Worker       │  scheduled │
│  └──────────────┘  └──────────────┘  └──────────────┘           │
└──────────────────────────────────────────────────────────────────┘
         │                   │                  │
         ▼                   ▼                  ▼
    ┌─────────┐        ┌─────────┐        ┌──────────┐
    │  PG DB  │        │ MySQL DB│        │ MSSQL DB │  ...
    └─────────┘        └─────────┘        └──────────┘
```

**Gateway nodes are stateless.** All state lives in The Brain:
- Scale horizontally by adding nodes behind a load balancer
- Any node can handle any request
- Node failure doesn't lose state
- Rolling deployments with zero downtime

**Crawler workers are separate processes.** They run on their own schedule and don't affect query latency.

---

## Technology Stack

| Component | Technology | Why |
|-----------|-----------|-----|
| Gateway nodes | Rust | High concurrency, low latency, strong typing, reuses pg-retest proxy code |
| PG wire protocol | Custom (from pg-retest proxy `protocol.rs`) | Already built, protocol-complete for proxying |
| HTTP API | `axum` | Tokio-native, lightweight, fast |
| MCP server | MCP SDK or custom | Standard protocol |
| Brain database | PostgreSQL + pgvector | Structured + vector data, extensions ecosystem |
| SQL parsing | `sqlparser` crate (multi-dialect) | Rust-native, supports PG/MySQL/MSSQL/Oracle |
| Schema crawling | `tokio-postgres`, `mysql_async`, `tiberius`, custom Oracle | Async native drivers per engine |
| LLM integration | `reqwest` → Claude API / OpenAI | HTTP-based, flexible provider choice |
| Embedding generation | Claude / OpenAI embeddings API | For semantic matching |
| Scheduling | `pg_cron` in Brain or built-in Tokio timers | Crawler schedules |
| Configuration | TOML + environment variables | Consistent with Rust ecosystem |
| Deployment | Docker / Kubernetes | Container-native |

---

## Relationship to pg-retest Proxy

The `pg-retest proxy` capture proxy is the foundation. Here's how each component evolves:

| pg-retest proxy | SQL Gateway |
|----------------|-------------|
| `protocol.rs` — PG message frame parser | Same, extended with response parsing for result capture |
| `pool.rs` — Session pooling | Extended with transaction pooling + multi-backend pools |
| `capture.rs` — Workload capture to .wkl | Becomes query observer writing to The Brain |
| `connection.rs` — Transparent relay | Add governance check between parse and forward |
| `listener.rs` — TCP accept | Add HTTP listener (axum) + MCP listener |
| `tls.rs` — TLS negotiation | Same, plus mTLS for agent authentication |
| (new) | `router.rs` — Semantic routing to multiple backends |
| (new) | `generator.rs` — Multi-dialect SQL generation |
| (new) | `governance.rs` — Resolution chain + access control |
| (new) | `brain.rs` — Brain connection pool + query interface |
| (new) | `crawlers/` — Schema, query observer, sampler, relationship mapper |

---

## Honest Limitations

### Latency
Every query goes through the gateway. Approved query path: ~5-20ms overhead. NL-to-SQL: 500ms-5s (LLM call). Cross-database federation: depends on slowest sub-query.

**Mitigation:** Encourage approved queries for common patterns. Cache routing decisions. Use fast LLMs.

### Crawler Freshness
Crawled knowledge is only as current as the last crawl. A table created between crawls won't be discoverable.

**Mitigation:** Configurable crawl frequency (down to 15 minutes). Event-based triggers where supported (PG NOTIFY, MySQL binlog). Schema change detection on query failure triggers re-crawl.

### NL-to-SQL Accuracy Across Dialects
Generating correct SQL for four database engines is harder than one. LLM accuracy varies by dialect.

**Mitigation:** Few-shot examples from observed queries. Dialect-specific AST validation catches syntax errors before execution. Approved query path bypasses generation entirely.

### Cross-Database Consistency
Federated queries read from multiple databases at different points in time. No cross-database transaction.

**Mitigation:** Return metadata showing when each sub-query executed. For strong consistency use cases, create approved queries against a single source.

### Complexity
More components, more failure modes, more operational overhead than a simple extension.

**Mitigation:** Phase 1 is just single-database gateway — same scope as an extension but as a service. Each phase adds capability incrementally.

---

## Implementation Roadmap

### Phase 1: Single-Database Gateway (MVP)
- Brain schema in PostgreSQL
- Schema crawler for PostgreSQL only
- Approved query registry with NL matching (pgvector)
- SQL whitelist with hash + pattern matching
- Agent authentication and role management
- PostgreSQL wire protocol API (`gateway.ask()`, `gateway.query()`)
- Basic access logging
- **Deliverable:** Drop-in replacement for direct PG agent access, with governance

### Phase 2: Multi-Database Support
- MySQL, SQL Server, Oracle crawlers
- Dialect-specific SQL generation
- Cross-database relationship mapping
- Routing logic (which database has the data?)
- HTTP REST API
- SQL validation per dialect

### Phase 3: Query Observer + Learning
- Query observer for PG (`pg_stat_statements`), MySQL (`performance_schema`), MSSQL (Query Store/DMVs)
- Observed query → approved query promotion suggestions
- Denied request clustering
- Agent behavior profiling + anomaly detection
- Learning dashboard views

### Phase 4: Data Sampler + Semantic Intelligence
- Data sampling across all connected databases
- PII detection by content (not just column name)
- Implicit relationship discovery
- Semantic entity auto-generation
- Enhanced routing using data profiles

### Phase 5: Cross-Database Federation
- Multi-source query decomposition
- Parallel sub-query execution
- In-gateway result merging
- Cross-database join support

### Phase 6: MCP + Ecosystem Integration
- MCP server with tools + resources
- Agent feedback loop
- Export/import for approved queries and access rules
- Integration with existing data catalogs (optional)
- Admin UI (web dashboard for Brain management)
