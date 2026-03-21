# Web Dashboard Guide

## Starting the Dashboard

Launch the web dashboard with the `web` subcommand:

```bash
pg-retest web --port 8080 --data-dir ./data
```

The dashboard will be available at `http://localhost:8080`. The `--data-dir` directory is created automatically and stores the SQLite database and workload files.

| Flag         | Default  | Description                                        |
|--------------|----------|----------------------------------------------------|
| `--port`     | `8080`   | HTTP port to listen on                             |
| `--data-dir` | `./data` | Directory for SQLite database and workload files   |


## Security

### Bind Address

By default, the web server binds to `127.0.0.1` (localhost only). External clients cannot connect unless you explicitly bind to a network interface:

```bash
# Listen on all interfaces (required for Docker, remote access)
pg-retest web --port 8080 --bind 0.0.0.0
```

### Authentication

A bearer token is auto-generated on startup and printed to stdout. All API calls (except `GET /health`) require the token in the `Authorization` header:

```
Authorization: Bearer <token>
```

Requests without a valid token receive a `401 Unauthorized` response.

| Flag | Description |
|------|-------------|
| `--no-auth` | Disable authentication entirely. Use for development or demo environments only. |
| `--auth-token <TOKEN>` | Set a specific bearer token instead of auto-generating one. Useful for CI/CD pipelines where the token must be known in advance. |

### Graceful Shutdown

On `SIGTERM` or `SIGINT`, the server drains active HTTP connections and cancels all background tasks (proxy, replay, pipeline) via their cancellation tokens before exiting.


## Overview

The pg-retest web dashboard is a single-page application built on **Axum** (Rust HTTP framework) with an **Alpine.js + Chart.js + Tailwind CSS** frontend. It provides a browser-based interface for every pg-retest operation: managing workload profiles, running the capture proxy, replaying workloads, comparing results, executing A/B tests, running CI/CD pipelines, and browsing historical runs.

All operations that run in the background (proxy, replay, pipeline, A/B tests) push real-time status updates to the browser via WebSocket.


## Pages

The dashboard contains 9 pages:

### 1. Dashboard

The landing page. Shows a summary of recent activity: total workloads, recent runs, active tasks, and quick-action links to common operations.

### 2. Workloads

Central workload management. Lists all imported workload profiles with metadata (source type, host, session/query counts, capture timestamp, classification). Provides actions to upload, import, inspect, classify, and delete workloads.

### 3. Proxy

Start and stop the PG wire protocol capture proxy directly from the browser. Shows live traffic as it flows through the proxy: session opens/closes, queries executed, queries-per-second, and active session count. All metrics stream over WebSocket in real time.

### 4. Replay

Configure and launch workload replays against a target PostgreSQL instance. Supports all replay options: read-only mode, speed multiplier, uniform scaling, per-category scaling (analytical, transactional, mixed, bulk), and stagger interval. Tracks per-session progress with a live progress bar.

### 5. A/B Testing

Configure two or more database variants (each with a label and connection string), then replay the same workload against each variant sequentially. The dashboard displays per-query regression detection, winner determination, and side-by-side latency comparison when the test completes.

### 6. Compare

View comparison reports between a source workload and its replay results. Shows per-query latency deltas, regression flags, throughput metrics, and error counts. Chart.js renders latency distribution and regression charts.

### 7. Pipeline

Validate and execute full CI/CD pipelines defined in TOML configuration. The UI provides a TOML editor, validation feedback, and live stage-by-stage progress as the pipeline runs (capture, provision, replay, compare, threshold evaluation, report generation).

### 8. History

Browse all historical runs with filtering by type (replay, ab, pipeline) and configurable limits. View trends over time for a given workload, including latency percentiles and throughput. Each run links to its full report.

### 9. Help

Reference documentation accessible within the dashboard. Covers subcommand usage, configuration format, and common workflows.


## Workload Management

### Upload a log file

Upload a raw PostgreSQL CSV log or MySQL slow query log file. The server runs capture automatically and produces a `.wkl` workload profile. Supports `pg-csv` and `mysql-slow` source types, with optional PII masking.

**Endpoint:** `POST /api/v1/workloads/upload` (multipart form with fields: `file`, `source_type`, `source_host`, `mask_values`)

### Import an existing .wkl file

Upload a previously captured `.wkl` file directly. The server reads profile metadata (sessions, queries, timestamps, classification) and registers it in the database.

**Endpoint:** `POST /api/v1/workloads/import` (multipart form with field: `file`)

### Inspect

View the full contents of a workload profile: all sessions, queries, timing metadata, transaction boundaries, and workload classification breakdown (Analytical, Transactional, Mixed, Bulk).

**Endpoint:** `GET /api/v1/workloads/{id}/inspect`

### Classify

Classification is computed automatically during inspect. Sessions are categorized based on their read/write ratio, average latency, and transaction count into one of four classes: Analytical, Transactional, Mixed, or Bulk.

### Delete

Removes the workload from the SQLite database and deletes the `.wkl` file from disk.

**Endpoint:** `DELETE /api/v1/workloads/{id}`


## Proxy

The proxy page provides full control over the PG wire protocol capture proxy:

- **Start proxy** -- Configure listen address, target PostgreSQL address, pool size, PII masking, and capture toggle. The proxy runs as a background task.
- **Stop proxy** -- Sends a cancellation signal for graceful shutdown. The captured workload is automatically saved and registered as a new workload in the database.
- **Live traffic view** -- WebSocket streams `ProxyQueryExecuted` events (rate-limited to 50/second) showing SQL previews and session IDs as queries flow through the proxy.
- **Session monitoring** -- See active session count, total query count, and QPS in real time via `ProxyStats` broadcasts every second. Individual session open/close events are shown with user and database information.
- **Toggle capture** -- Enable or disable workload capture without stopping the proxy (proxy-only mode).
- **Session list** -- View all proxy sessions (active and completed) with per-session query counts and timestamps.


## Replay

- **Start replay** -- Select a workload, provide a target connection string, and configure replay options (read-only, speed, scale, per-category scaling, stagger). The replay runs as a background task.
- **Per-session progress** -- As each session completes, the server broadcasts `ReplayProgress` with completed/total counts and percentage. The UI renders a live progress bar.
- **Cancel** -- Cancel a running replay by task ID. The cancellation token stops in-progress sessions gracefully.
- **Results** -- When replay completes, a comparison report is computed automatically. The `ReplayCompleted` event includes the run ID for immediate viewing.


## Compare

- **Compute comparison** -- Provide a workload ID and run ID to compute a side-by-side comparison report. The report includes per-query latency deltas, regression flags (based on a configurable threshold percentage), throughput metrics, and error counts.
- **View report** -- Retrieve a stored comparison report by run ID. Chart.js renders latency distributions and regression visualizations.
- **Charts** -- Latency histograms, per-query before/after scatter plots, and regression count summaries.


## Pipeline

- **Validate config** -- Submit TOML configuration for syntax and schema validation. The response indicates whether the config is valid and summarizes its sections (capture, provision, thresholds, variant count).
- **Run pipeline** -- Submit TOML configuration to execute the full pipeline: capture, provision, replay, compare, threshold evaluation, and report generation. Progress is broadcast via `PipelineStageChanged` events.
- **View results** -- Retrieve pipeline run results including the comparison report, threshold evaluation results, and exit code.


## A/B Testing

- **Configure variants** -- Define two or more variants, each with a label (e.g., "pg15-default", "pg16-tuned") and a PostgreSQL connection string.
- **Run test** -- The workload is replayed sequentially against each variant. As each variant completes, an `ABVariantCompleted` event is broadcast.
- **View results** -- The A/B report shows per-query regression detection via positional matching, winner determination by average latency, and threshold-based improvement/regression classification.


## History

- **Run list** -- All runs (replay, ab, pipeline) are stored in SQLite with timestamps, status, configuration, and results. Filter by `run_type` and limit result count.
- **Run details** -- Each run includes its full comparison report, threshold results (if applicable), and exit code.
- **Trends** -- View performance trends over time for a given workload: latency percentiles and throughput across multiple runs.
- **Stats** -- Aggregate statistics across all runs (counts by type, pass/fail rates).


## Real-Time Updates (WebSocket)

Connect to `ws://localhost:8080/api/v1/ws` to receive server-sent JSON messages. All messages include a `"type"` field for dispatch. The WebSocket is read-only from the server's perspective -- clients connect and receive broadcasts; the server drains any client-sent messages without processing them.

### Proxy Events

| Message Type           | Fields                                              | Description                                                  |
|------------------------|-----------------------------------------------------|--------------------------------------------------------------|
| `ProxyStarted`         | `task_id`                                           | Proxy background task has started                            |
| `ProxyStopped`         | `workload_id` (nullable)                            | Proxy has stopped; includes workload ID if capture was active |
| `ProxySessionOpened`   | `session_id`, `user`, `database`                    | A new client session connected through the proxy             |
| `ProxySessionClosed`   | `session_id`, `query_count`                         | A client session disconnected; includes total query count    |
| `ProxyQueryExecuted`   | `session_id`, `sql_preview`, `duration_us`          | A query was executed (rate-limited to 50/second)             |
| `ProxyStats`           | `active_sessions`, `total_queries`, `qps`           | Aggregate proxy statistics, broadcast every 1 second         |

### Replay Events

| Message Type           | Fields                                              | Description                                                  |
|------------------------|-----------------------------------------------------|--------------------------------------------------------------|
| `ReplayProgress`       | `task_id`, `completed`, `total`, `pct`              | Per-session replay progress update                           |
| `ReplayCompleted`      | `task_id`, `run_id`                                 | Replay finished successfully; run ID links to results        |
| `ReplayFailed`         | `task_id`, `error`                                  | Replay failed with an error message                          |

### Pipeline Events

| Message Type              | Fields                                           | Description                                                  |
|---------------------------|--------------------------------------------------|--------------------------------------------------------------|
| `PipelineStageChanged`    | `task_id`, `stage`                               | Pipeline advanced to a new stage (e.g., "starting", "capture", "replay") |
| `PipelineCompleted`       | `task_id`, `exit_code`                           | Pipeline finished; exit code indicates pass/fail             |

### A/B Events

| Message Type           | Fields                                              | Description                                                  |
|------------------------|-----------------------------------------------------|--------------------------------------------------------------|
| `ABVariantCompleted`   | `task_id`, `label`                                  | One variant's replay finished; label identifies which one    |
| `ABCompleted`          | `task_id`, `run_id`                                 | All variants completed; run ID links to the A/B report       |

### General Events

| Message Type           | Fields                                              | Description                                                  |
|------------------------|-----------------------------------------------------|--------------------------------------------------------------|
| `TaskStatusChanged`    | `task_id`, `status`                                 | Generic task status update (running, completed, failed, cancelled) |
| `Error`                | `message`                                           | Server-side error message                                    |


## API Reference

All endpoints are nested under `/api/v1/`.

### Health and Tasks

| Method | Path           | Description                                    |
|--------|----------------|------------------------------------------------|
| GET    | `/health`      | Health check; returns status, version, name    |
| GET    | `/tasks`       | List all active and recent background tasks    |

### WebSocket

| Method | Path   | Description                                           |
|--------|--------|-------------------------------------------------------|
| GET    | `/ws`  | WebSocket upgrade endpoint for real-time event stream |

### Workloads

| Method | Path                        | Description                                                  |
|--------|-----------------------------|--------------------------------------------------------------|
| GET    | `/workloads`                | List all registered workload profiles                        |
| POST   | `/workloads/upload`         | Upload a log file, run capture, and register the workload    |
| POST   | `/workloads/import`         | Import an existing `.wkl` file and register it               |
| GET    | `/workloads/{id}`           | Get metadata for a single workload                           |
| DELETE | `/workloads/{id}`           | Delete a workload (removes DB record and `.wkl` file)        |
| GET    | `/workloads/{id}/inspect`   | Inspect full profile contents with classification            |

### Proxy

| Method | Path                    | Description                                            |
|--------|-------------------------|--------------------------------------------------------|
| GET    | `/proxy/status`         | Get current proxy status (running, addresses, counters)|
| POST   | `/proxy/start`          | Start the capture proxy as a background task           |
| POST   | `/proxy/stop`           | Stop the running proxy gracefully                      |
| POST   | `/proxy/toggle-capture` | Toggle capture mode on/off without stopping the proxy  |
| GET    | `/proxy/sessions`       | List proxy sessions (active and completed)             |

### Replay

| Method | Path                    | Description                                            |
|--------|-------------------------|--------------------------------------------------------|
| POST   | `/replay/start`         | Start a workload replay as a background task           |
| GET    | `/replay/{id}`          | Get replay run status and results                      |
| POST   | `/replay/{id}/cancel`   | Cancel a running replay                                |

### Compare

| Method | Path                    | Description                                            |
|--------|-------------------------|--------------------------------------------------------|
| POST   | `/compare`              | Compute a comparison report for a workload and run     |
| GET    | `/compare/{run_id}`     | Retrieve a stored comparison report                    |

### A/B Testing

| Method | Path                    | Description                                            |
|--------|-------------------------|--------------------------------------------------------|
| POST   | `/ab/start`             | Start an A/B test with multiple variants               |
| GET    | `/ab/{id}`              | Get A/B test status and report                         |

### Pipeline

| Method | Path                    | Description                                            |
|--------|-------------------------|--------------------------------------------------------|
| POST   | `/pipeline/start`       | Start a full CI/CD pipeline from TOML config           |
| POST   | `/pipeline/validate`    | Validate a TOML pipeline configuration                 |
| GET    | `/pipeline/{id}`        | Get pipeline run status and results                    |

### Runs (History)

| Method | Path                    | Query Parameters                    | Description                                   |
|--------|-------------------------|-------------------------------------|-----------------------------------------------|
| GET    | `/runs`                 | `run_type`, `limit`                 | List runs with optional type filter and limit  |
| GET    | `/runs/stats`           |                                     | Aggregate run statistics                       |
| GET    | `/runs/trends`          | `workload_id`, `limit`              | Performance trends over time for a workload    |
| GET    | `/runs/{id}`            |                                     | Get run details with report and thresholds     |


## Data Storage

### SQLite Database

The SQLite database is stored at `{data_dir}/pg-retest.db` and tracks metadata only:

- **workloads** -- ID, name, file path, source type/host, capture timestamp, session/query counts, classification
- **runs** -- ID, type (replay/ab/pipeline), status, workload ID, config, timestamps, target connection, replay mode, speed, scale, results path, report JSON, exit code, error message
- **proxy_sessions** -- Task ID, session ID, user, database, query count, start/end timestamps
- **threshold_results** -- Run ID, threshold name, passed/failed, actual value, threshold limit

### Workload Files on Disk

`.wkl` files (MessagePack binary format) are the source of truth for workload data. They are stored in `{data_dir}/workloads/` and are referenced by file path in the SQLite database. Replay results are stored in `{data_dir}/results/`.

The SQLite database can be regenerated from the `.wkl` files on disk -- it serves as a fast metadata index, not the canonical data store.


## Frontend Stack

The frontend is a single-page application with no build step required:

- **Alpine.js** -- Lightweight JavaScript framework for reactivity and DOM binding. Provides `x-data`, `x-on`, `x-show`, and other directives for dynamic UI without a compile step.
- **Chart.js** -- JavaScript charting library for latency histograms, throughput graphs, regression scatter plots, and trend lines.
- **Tailwind CSS** -- Utility-first CSS framework for styling. The dashboard uses a dark industrial theme.
- **JetBrains Mono** -- Monospace font used for data display (query text, metrics, JSON).
- **DM Sans** -- Sans-serif font used for UI text (labels, headings, navigation).

All frontend dependencies are loaded via CDN -- there is no `node_modules`, no `package.json`, and no JavaScript build pipeline. The HTML, CSS, and JS files are embedded into the Rust binary at compile time via the `rust-embed` crate from `src/web/static/`. Modifying frontend files requires recompilation of the Rust binary.
