# pg-retest Capture Proxy Gateway — Design

**Date:** 2026-03-04
**Status:** Approved
**Scope:** Lightweight PG wire protocol proxy with session pooling and workload capture

## Problem

CSV log capture has three limitations:
1. Requires server-side logging configuration (`log_min_duration_statement`, `logging_collector`)
2. Prepared statement parameter values require parsing the `detail` field — sometimes missing
3. Timing accuracy is limited to PG's internal log timestamp (not network-level)

A protocol-level proxy captures workload without any server-side changes and gets precise timing from packet arrival.

## Architecture

```
Client Apps (1000s)         pg-retest proxy              PostgreSQL (or PgBouncer)
    |                          |                               |
    |--- TCP connect --------->| accept on --listen port       |
    |                          |--- connect to --target ------>|
    |                          |   (from pool or new conn)     |
    |                          |                               |
    |<== bidirectional relay ==>|<== bidirectional relay =====>|
    |    (msg frames parsed    |    (bytes forwarded           |
    |     for capture data)    |     verbatim)                 |
    |                          |                               |
    |--- Terminate ('X') ----->| return server conn to pool    |
    |                          |   (after reset query)         |
    |                          |                               |
                          On shutdown (Ctrl+C / --duration):
                          Build WorkloadProfile from all captured sessions
                          Write to .wkl file
```

### Core Principles

- **Forward verbatim, peek selectively.** All bytes pass through unchanged. Only message headers and specific message types are parsed for capture data.
- **Session pooling.** Server connections are pooled and reused across client sessions. A server connection is pinned to a client for the client's entire session (like PgBouncer session mode).
- **Capture off the hot path.** Relay tasks send capture events via async channel to a dedicated collector task. No disk I/O or heavy processing in the relay path.
- **Can coexist with PgBouncer.** Proxy sits in front of PgBouncer or directly in front of PG. Not a replacement for production-grade poolers — a complement with capture built in.

## Connection Pooling

### Session Pool Model

```
Pool State:
  idle_connections: Vec<ServerConn>    (available for assignment)
  active_connections: usize            (currently pinned to clients)
  waiting_clients: VecDeque<Waiter>    (queued when pool full)
  max_size: usize                      (--pool-size flag)

On client connect:
  1. If idle_connections is non-empty → assign one to client
  2. If active + idle < max_size → open new server connection, assign to client
  3. If at capacity → queue client (with --pool-timeout)

On client disconnect:
  1. Run reset query on server connection (DISCARD ALL or configurable)
  2. If idle + active < max_size → return to idle pool
  3. Else → close server connection

On pool timeout:
  Client receives error: "too many connections, pool exhausted"
```

### Configuration

| Flag | Default | Description |
|------|---------|-------------|
| `--pool-size` | 100 | Max server connections to target |
| `--pool-timeout` | 30s | How long a client waits for a pool slot |
| `--reset-query` | `DISCARD ALL` | SQL run on server conn before returning to pool |

## PG Wire Protocol Handling

### Message Frame Format

Every message after startup: `[1 byte type][4 byte length][payload]`

Startup messages (no type byte): `[4 byte length][payload]` — detected by absence of type byte on initial connection.

### What We Parse vs. Forward Opaquely

| Message | Type | Direction | Action |
|---------|------|-----------|--------|
| SSLRequest | (special) | C→P | Intercept. Handle per --tls mode. |
| StartupMessage | (special) | C→S | Forward. Extract user, database. |
| All auth messages | R,p | Both | Forward verbatim. |
| BackendKeyData | K | S→C | Forward. Record PID for cancel routing. |
| ReadyForQuery | Z | S→C | Forward. Record transaction state (I/T/E). |
| Query | Q | C→S | Forward. **Extract SQL text + timestamp.** |
| Parse | P | C→S | Forward. **Extract stmt name → SQL mapping.** |
| Bind | B | C→S | Forward. **Extract stmt name + parameter values.** |
| Execute | E | C→S | Forward. Record timestamp for latency. |
| CommandComplete | C | S→C | Forward. **Compute query latency.** |
| ErrorResponse | E | S→C | Forward. Record error details. |
| CopyIn/Out/Data | d,c,f,G,H,W | Both | Forward verbatim. No parsing. |
| NoticeResponse | N | S→C | Forward. Ignore for capture. |
| Terminate | X | C→S | Forward. Trigger session end. |
| CancelRequest | (special) | New conn | Route to server using PID mapping. |
| Everything else | * | Both | Forward verbatim. |

### Extended Query Protocol: Prepared Statement Tracking

The proxy maintains a per-connection `HashMap<String, String>` mapping statement names to SQL text:

```
Parse("stmt1", "SELECT * FROM users WHERE id = $1")
  → record: stmt_cache["stmt1"] = "SELECT * FROM users WHERE id = $1"

Bind("stmt1", params=["42"])
  → lookup: sql = stmt_cache["stmt1"]
  → capture event: QueryStart { sql with params inlined }

Execute("stmt1")
  → record timestamp for latency measurement

CommandComplete
  → capture event: QueryComplete { duration_us }
```

This gives us the complete SQL with actual parameter values — better than CSV log capture.

### SSL/TLS Modes

| Mode | Flag | Behavior |
|------|------|----------|
| **reject** | `--tls reject` (default) | Respond 'N' to SSLRequest. Client falls back to plaintext. |
| **terminate** | `--tls terminate --cert X --key Y` | Proxy terminates TLS. Connects to server in plaintext or with `--target-tls`. |
| **passthrough** | `--tls passthrough` | Forward SSL to server. **Capture disabled** — becomes L4 relay. Logs warning. |

TLS termination requires `tokio-rustls`, gated behind `tls` cargo feature flag.

## Capture Data Flow

```
Client→Server relay task                    Server→Client relay task
        |                                           |
   [parse msg frame]                          [parse msg frame]
   [if Query/Parse/Bind/Execute:              [if CommandComplete/Error/ReadyForQuery:
    extract capture data]                      extract capture data]
        |                                           |
        +--------> CaptureEvent channel <-----------+
                          |
                          v
                  CaptureCollector task
                  ├── tracks per-session state
                  ├── builds Query structs (sql, timing, kind, txn_id)
                  ├── assigns transaction IDs (BEGIN/COMMIT tracking)
                  └── on shutdown: build WorkloadProfile, write .wkl
```

### CaptureEvent Types

```rust
enum CaptureEvent {
    SessionStart { session_id: u64, user: String, database: String, timestamp: Instant },
    PreparedStatement { session_id: u64, name: String, sql: String },
    QueryStart { session_id: u64, sql: String, params: Option<Vec<String>>, timestamp: Instant },
    QueryComplete { session_id: u64, command_tag: String, timestamp: Instant },
    QueryError { session_id: u64, message: String, timestamp: Instant },
    TransactionState { session_id: u64, state: TxnState },
    SessionEnd { session_id: u64, timestamp: Instant },
}
```

### Profile Generation

On shutdown (Ctrl+C, SIGTERM, or `--duration` elapsed):
1. Drain remaining capture events
2. For each session: convert captured queries to `Query` structs with relative `start_offset_us`
3. Assign `QueryKind` via `QueryKind::from_sql()`
4. Assign `transaction_id` via `assign_transaction_ids()`
5. Apply `--mask-values` if requested
6. Build `WorkloadProfile` with `capture_method: "proxy"`
7. Write to `--output` path via `profile::io::write_profile()`

Output is identical to CSV log capture — same `.wkl` format, same replay compatibility.

## CLI Interface

```bash
# Basic: capture proxy on port 5433, forward to local PG
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --output workload.wkl

# With duration limit and pooling config
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --output workload.wkl \
  --duration 60s \
  --pool-size 50

# With TLS termination
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --output workload.wkl \
  --tls terminate --cert server.crt --key server.key

# With PII masking
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --output workload.wkl \
  --mask-values

# Passthrough mode (no capture, just pooling)
pg-retest proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5432 \
  --no-capture
```

## Module Structure

```
src/
  proxy/
    mod.rs          — ProxyConfig, ProxyServer::run(), public API
    listener.rs     — TCP accept loop, SSL negotiation, spawn connections
    connection.rs   — Per-connection state machine (startup → relay → shutdown)
    protocol.rs     — PG message frame reader/writer, selective message parsing
    pool.rs         — SessionPool: idle/active tracking, waiters, reset query
    capture.rs      — CaptureCollector: event channel consumer, profile builder
    tls.rs          — TLS mode handling (reject/terminate/passthrough)
```

All modules are under `src/proxy/` and exported via `pub mod proxy` in `lib.rs`.

## New Dependencies

```toml
# Required
bytes = "1"                  # Efficient byte buffer management for protocol parsing

# Optional (behind feature flag)
tokio-rustls = { version = "0.26", optional = true }
rustls-pemfile = { version = "2", optional = true }

[features]
default = []
tls = ["tokio-rustls", "rustls-pemfile"]
```

## Performance Target

- **Per-query overhead:** <200us (message frame parse + channel send)
- **Connection overhead:** <1ms (pool checkout + startup forwarding)
- **Memory per connection:** ~8KB (read/write buffers + stmt cache)
- **Throughput:** 50K+ queries/sec on a single core (message forwarding is I/O-bound, not CPU-bound)

Reference: PgBouncer adds <100us overhead per query with C + libevent. Our Tokio-based proxy should be in the same ballpark.

## Future: SQL Gateway Evolution

This proxy becomes the transport layer for the full SQL Gateway:

| Proxy feature | Gateway evolution |
|--------------|-------------------|
| Forward queries verbatim | Add governance validation before forwarding |
| Single --target server | Add semantic routing to multiple backends |
| PG auth passthrough | Add agent authentication (API key, mTLS) |
| .wkl file output | Add real-time query observer writing to The Brain |
| Session pooling | Add transaction pooling with prepared stmt tracking |
| PG wire protocol only | Add HTTP REST + MCP server interfaces |

The proxy module stays — governance and routing plug in between protocol parsing and forwarding.

## Build Order

1. **protocol.rs** — Message frame reader (type + length + payload), selective parsers for Query/Parse/Bind/Execute/CommandComplete/ErrorResponse/ReadyForQuery
2. **pool.rs** — Session pool (idle list, max size, checkout/checkin, reset query)
3. **capture.rs** — CaptureEvent enum, CaptureCollector (channel consumer → WorkloadProfile builder)
4. **connection.rs** — Per-connection state machine (startup passthrough, bidirectional relay with capture)
5. **listener.rs** — TCP accept loop, startup message detection, SSL negotiation
6. **tls.rs** — TLS reject (default), terminate and passthrough behind feature flag
7. **mod.rs** — ProxyConfig, ProxyServer, CLI wiring
8. **Integration** — Add `proxy` subcommand to CLI, wire into main.rs, integration tests

## Testing Strategy

- **Unit tests:** Message frame parsing, parameter extraction, pool checkout/checkin logic
- **Integration tests:** Spin up proxy against a mock TCP server (no real PG needed for protocol tests)
- **End-to-end test:** Proxy in front of Docker PG container, run queries through proxy, verify .wkl output matches expected workload
