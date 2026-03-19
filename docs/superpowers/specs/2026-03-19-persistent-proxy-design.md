# Persistent Proxy with Decoupled Capture Design

**Date:** 2026-03-19
**Status:** Approved

## Problem

The current proxy couples its lifecycle to capture — starting the proxy begins recording, stopping the proxy writes the `.wkl` file. For production use, this means redirecting traffic to the proxy and back requires downtime or careful DNS/LB changes each time you want to capture a workload. Additionally, the capture collector holds all queries in memory, which risks OOM on long-running or high-traffic captures.

## Solution

Decouple the proxy lifecycle from capture. The proxy runs persistently as a transparent relay between application and database. Capture is toggled on/off independently, producing a `.wkl` file each time capture stops. Queries are staged to SQLite during capture to bound memory usage. Multiple sequential captures are supported without restarting the proxy.

## 1. Persistent Proxy Mode

### New CLI Flags

```
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --persistent \
  --control-port 9091
```

- `--persistent` — Keeps the proxy running indefinitely. No `--output` required. No capture on start. The proxy acts as a pure relay until capture is explicitly started.
- `--control-port` — Port for the minimal HTTP control endpoint (default: 9091). Only used in standalone mode (not needed when running via web dashboard).
- Without `--persistent`, the proxy behaves exactly as today (start → capture → stop → write).

### Capture Lifecycle

```
Proxy starts (persistent) → listening, no capture active
  ↓
Start Capture (via web UI, CLI proxy-ctl, or control API)
  ↓
Queries written to SQLite staging table as they arrive
  ↓
Stop Capture (via web UI, CLI proxy-ctl, or control API)
  ↓
SQLite staging → build WorkloadProfile → write .wkl → clear staging
  ↓
Proxy still running, ready for next capture
```

Multiple sequential captures are supported. Each stop-capture produces an independent `.wkl` file.

## 2. SQLite Staging for Capture

### Motivation

The current capture collector accumulates all `CaptureEvent` data in memory via `Vec<CapturedQuery>` per session. For a persistent proxy running hours or days on a high-traffic database, this leads to unbounded memory growth and eventual OOM. SQLite staging writes queries to disk as they arrive, bounding memory usage to a small batch buffer.

### Staging Table Schema

```sql
CREATE TABLE IF NOT EXISTS capture_staging (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    capture_id TEXT NOT NULL,
    session_id INTEGER NOT NULL,
    user_name TEXT,
    database_name TEXT,
    sql TEXT NOT NULL,
    kind TEXT,
    start_offset_us INTEGER NOT NULL,
    duration_us INTEGER NOT NULL,
    is_error INTEGER NOT NULL DEFAULT 0,
    error_message TEXT,
    timestamp_us INTEGER NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_staging_capture ON capture_staging(capture_id);
```

- `capture_id` — UUID grouping queries to one capture session. Generated on start-capture.
- `timestamp_us` — Absolute microsecond timestamp for ordering.
- `start_offset_us` — Offset from capture start, used for replay timing.

### Data Flow

1. **Start capture** → Generate `capture_id` (UUID), record start timestamp.
2. **During capture** → The collector task receives `CaptureEvent::QueryComplete` events and batches inserts into SQLite. Batches flush every 100 queries or 500ms, whichever comes first. This amortizes SQLite write overhead while keeping memory bounded to one batch (~100 queries).
3. **Stop capture** → Read all rows for `capture_id` from `capture_staging`, build `WorkloadProfile` via existing `build_profile()` logic, write `.wkl` file, delete staging rows for that `capture_id`.
4. **Crash recovery** → On proxy restart in persistent mode, check for orphaned rows in `capture_staging`. Log a warning with the count and `capture_id`. The user can recover them via `proxy-ctl recover --proxy localhost:9091` or discard with `proxy-ctl discard --proxy localhost:9091`.

### Where the SQLite Database Lives

- **Web mode**: Reuses the existing `pg-retest.db` in `data_dir`. The `capture_staging` table is added to `init_db()`.
- **Standalone mode**: Creates `proxy-capture.db` in the current directory (or `--data-dir` if specified).

### Memory Budget

- Collector task: at most one batch (~100 queries) in memory before flushing.
- Session metadata: user, database, connection mapping — a few KB per active connection.
- Profile building on stop: reads from SQLite in chunks, builds profile incrementally. Peak memory during finalization is proportional to the number of unique sessions (metadata), not total query count.

## 3. Control Plane

### Standalone Control Endpoint

When running with `--persistent` (without the web server), the proxy starts a minimal Axum HTTP server on `--control-port`:

```
GET  /status              → { running, capturing, capture_id, active_sessions, total_queries, uptime_secs }
POST /start-capture       → { capture_id, started_at }
POST /stop-capture        → { capture_id, workload_path, total_queries, total_sessions }
```

Three endpoints. No static files, no WebSocket. JSON request/response only.

### proxy-ctl CLI Subcommand

New subcommand for controlling a running proxy from a separate terminal:

```bash
pg-retest proxy-ctl status --proxy localhost:9091
pg-retest proxy-ctl start-capture --proxy localhost:9091
pg-retest proxy-ctl stop-capture --proxy localhost:9091 --output workload.wkl
pg-retest proxy-ctl recover --proxy localhost:9091
pg-retest proxy-ctl discard --proxy localhost:9091
```

`--proxy` accepts `host:port`. The subcommand auto-detects whether the target is a standalone control port or the web dashboard by trying `GET /api/v1/health` first. If it responds, use web API paths (`/api/v1/proxy/*`); otherwise use standalone paths (`/status`, `/start-capture`, `/stop-capture`).

### Web Dashboard Integration

The existing Proxy page changes:

- **Start Proxy** button: in persistent mode, the proxy stays running after capture stops.
- **Capture toggle**: becomes a distinct Start Capture / Stop Capture button pair, separate from the proxy lifecycle.
- **Capture state indicator**: `Idle` / `Capturing` / `Finalizing` badge next to proxy status.
- **Capture history**: List of previous captures from this proxy session with timestamps, query counts, and links to the workload in the Workloads page.
- **Stop Proxy**: becomes a secondary action with confirmation dialog ("This will disconnect all active clients. Continue?").

### Existing Web API Changes

| Endpoint | Current Behavior | New Behavior |
|----------|-----------------|-------------|
| `POST /proxy/start` | Starts proxy + capture | Starts proxy. If persistent, no auto-capture. |
| `POST /proxy/stop` | Stops proxy (cancels task) | Stops proxy entirely (confirmation required in UI) |
| `POST /proxy/toggle-capture` | **Stub — returns `{ "toggled": true }` and does nothing** (must be built from scratch) | Starts or stops a capture cycle. On stop: flush SQLite → build `.wkl` → register workload → clear staging |
| `GET /proxy/status` | Returns running/task_id/stats | Adds `capturing: bool`, `capture_id: Option<String>`, `capture_history: Vec<CaptureRecord>` |

## 4. Backward Compatibility

### No Breaking Changes

Without `--persistent`, the proxy behaves identically to today:

```bash
# Still works exactly as before
pg-retest proxy --listen 0.0.0.0:5433 --target db:5432 --output workload.wkl
```

`--persistent` is opt-in. The existing web dashboard proxy flow (Start → capture → Stop → get workload) continues to work. The enhancement adds the ability to keep the proxy running and cycle captures independently.

### Output File Naming

- **Legacy mode** (`--output workload.wkl`): Writes to the specified path, as today.
- **Persistent mode** (no `--output`): Auto-generates timestamped filenames: `capture-2026-03-19T14-00-00.wkl` in the data directory.
- **proxy-ctl** (`--output`): Overrides the auto-generated name for the current capture.

### What Does NOT Change

- Wire protocol relay (`connection.rs`, `protocol.rs`) — untouched
- Session pool (`pool.rs`) — untouched
- PII masking — still applied at profile-build time
- Transaction ID assignment — still applied at profile-build time
- `.wkl` format — identical MessagePack v2
- Existing WebSocket message types — `ProxyStarted`, `ProxyStopped`, `ProxyQueryExecuted`, `ProxyStats`, `ProxySessionOpened`, `ProxySessionClosed` all continue to work

## 5. Implementation Scope

### Files to Modify

| File | Change |
|------|--------|
| `src/proxy/mod.rs` | Add `persistent` and `control_port` to `ProxyConfig`. Change `output: PathBuf` to `output: Option<PathBuf>` (persistent mode has no default output). Update `run_proxy()` to loop on captures when persistent. Start control endpoint. Refactor `run_proxy_managed()` to support multi-capture lifecycle (see architectural note below). |
| `src/proxy/capture.rs` | Replace in-memory `Vec` collector with SQLite-backed staging. Add batched insert logic. Add `build_profile_from_staging()`. Add `is_error` and `error_message` fields to `CapturedQuery` struct so error metadata survives staging round-trip. |
| `src/cli.rs` | Add `--persistent`, `--control-port` to `ProxyArgs`. Change `output` from `PathBuf` to `Option<PathBuf>` (no default in persistent mode). Add `ProxyCtl` subcommand with `status`, `start-capture`, `stop-capture`, `recover`, `discard`. |
| `src/main.rs` | Wire `ProxyCtl` subcommand dispatch. Update `run_proxy` call site for `output: Option<PathBuf>`. |
| `src/web/db.rs` | Add `capture_staging` table to `init_db()`. |
| `src/web/handlers/proxy.rs` | **Fully implement** `toggle-capture` (currently a no-op stub). Wire it to start/stop capture cycles with SQLite flush + `.wkl` write on stop. Add capture history to status. Update start/stop semantics. |
| `src/web/static/js/pages/proxy.js` | Update UI for capture toggle, state indicator, capture history, stop confirmation. |

### New Files

| File | Purpose |
|------|---------|
| `src/proxy/control.rs` | Minimal Axum HTTP control endpoint (3 routes) for standalone persistent mode. |
| `src/proxy/staging.rs` | SQLite staging: batched insert, read-back, cleanup, crash recovery. |

### Architectural Notes

**`run_proxy_managed` multi-capture lifecycle:**
The current `run_proxy_managed()` returns `Result<Option<WorkloadProfile>>` when the proxy stops — tying profile delivery to proxy shutdown. In persistent mode, the proxy must stay alive across multiple capture cycles. The solution: `run_proxy_managed()` no longer returns a profile on completion. Instead, capture stop is handled via a channel. The web handler sends a `CaptureCommand::Stop { output_path }` message through an `mpsc` channel to the proxy task, which triggers SQLite flush → profile build → `.wkl` write → workload registration. The profile (or a summary) is sent back through a `oneshot` channel so the handler can respond to the HTTP request. The `CancellationToken` continues to control proxy shutdown (distinct from capture stop).

**`proxy-ctl` auto-detection:**
The `proxy-ctl` subcommand detects whether the target is a web dashboard or standalone control port by trying `GET /api/v1/health` (which exists in the web server at `src/web/handlers/mod.rs:17`). If it responds with `{"status":"ok"}`, use web API paths; otherwise use standalone control paths. No new health endpoint is needed — the existing one serves this purpose.

**SQLite connection management in `staging.rs`:**
`staging.rs` accepts an `Arc<Mutex<rusqlite::Connection>>` injected from outside. In web mode, this is the existing `state.db` connection (shared with other web handlers). In standalone mode, `run_proxy()` opens a new `rusqlite::Connection` to `proxy-capture.db`, wraps it in `Arc<Mutex<_>>`, and passes it to the staging module. This keeps `staging.rs` agnostic to its context and avoids duplicating connection management logic.

### Test Coverage

- Unit tests for SQLite staging (insert, read, cleanup, recovery)
- Unit tests for batched collector (flush on count, flush on timeout)
- Integration test: persistent proxy with sequential captures
- Integration test: proxy-ctl commands against control endpoint
- Existing proxy tests continue to pass (backward compat)
