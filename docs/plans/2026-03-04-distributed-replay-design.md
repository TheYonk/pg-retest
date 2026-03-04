# Distributed Replay Runners — Design

**Date:** 2026-03-04
**Status:** Draft (future feature)
**Scope:** Multi-node replay execution for large-scale load testing

## Problem

A single `pg-retest replay` process hits limits at high scale:
- Memory: holding 100K+ sessions in memory
- CPU: Tokio runtime on one machine can only drive so many concurrent connections
- Network: single source IP may hit connection limits or network bandwidth ceiling
- Realism: real production traffic comes from many hosts

## Architecture

```
                         ┌────────────────────────┐
                         │     Control Node        │
                         │  pg-retest orchestrate  │
                         │                         │
                         │  - Splits workload      │
                         │  - Assigns to runners   │
                         │  - Collects results     │
                         │  - Produces report      │
                         └───────────┬────────────┘
                                     │ gRPC or HTTP
                    ┌────────────────┼────────────────┐
                    │                │                 │
              ┌─────▼─────┐  ┌──────▼────┐  ┌────────▼───┐
              │  Runner 1  │  │  Runner 2 │  │  Runner 3  │
              │  (host A)  │  │  (host B) │  │  (host C)  │
              │            │  │           │  │            │
              │ Sessions   │  │ Sessions  │  │ Sessions   │
              │ 1-100      │  │ 101-200   │  │ 201-300    │
              └─────┬──────┘  └─────┬─────┘  └─────┬──────┘
                    │               │               │
                    └───────────────┼───────────────┘
                                    │
                              ┌─────▼─────┐
                              │ PostgreSQL │
                              │  (target)  │
                              └───────────┘
```

## Components

### Runner (Worker Node)

A lightweight agent that registers with the control node and executes assigned sessions.

```bash
# Start a runner on each worker host
pg-retest runner \
  --control http://control-node:9090 \
  --name "runner-east-1" \
  --target "host=db.example.com dbname=mydb user=replay" \
  --max-sessions 200
```

**Runner lifecycle:**
1. Starts up, connects to control node via HTTP/gRPC
2. Registers: sends name, capacity (max sessions), target connection string
3. Waits for work assignment
4. Receives session slice (subset of workload profile)
5. Executes replay using existing `run_replay()` engine
6. Streams results back to control node
7. Reports completion, waits for next assignment or shutdown

**Runner registration message:**
```json
{
    "name": "runner-east-1",
    "host": "10.0.1.50",
    "capacity_sessions": 200,
    "target": "host=db.example.com dbname=mydb user=replay",
    "status": "ready"
}
```

### Control Node (Orchestrator)

Coordinates the distributed replay.

```bash
pg-retest orchestrate \
  --workload workload.wkl \
  --listen 0.0.0.0:9090 \
  --output results.wkl \
  --scale 10 \
  --stagger-ms 500 \
  --speed 1.0 \
  --read-only \
  --min-runners 3 \
  --wait-for-runners 60s
```

**Control node lifecycle:**
1. Loads workload profile
2. Applies scaling (--scale N) to generate full session list
3. Starts HTTP/gRPC server, waits for runners to register
4. Once `--min-runners` registered (or `--wait-for-runners` timeout):
   - Splits sessions across runners (round-robin or by capacity)
   - Sends each runner its session slice + replay config (speed, mode)
   - Signals coordinated start (all runners begin at same wall-clock time)
5. Collects results as runners complete
6. Merges all results into single report
7. Writes combined output + produces comparison report

### Session Distribution Strategy

```
Workload: 300 sessions (after scaling)
Runners:  3 registered (capacity: 200, 200, 100)

Strategy 1: Round-robin (simple)
  Runner 1: sessions 1, 4, 7, 10, ...  (100 sessions)
  Runner 2: sessions 2, 5, 8, 11, ...  (100 sessions)
  Runner 3: sessions 3, 6, 9, 12, ...  (100 sessions)

Strategy 2: Capacity-weighted (better)
  Runner 1: sessions 1-120     (120 sessions, 60% of capacity)
  Runner 2: sessions 121-240   (120 sessions, 60% of capacity)
  Runner 3: sessions 241-300   (60 sessions, 60% of capacity)

Strategy 3: Classification-aware (best)
  Split by workload class so each runner gets a mix:
  Runner 1: 40 analytical + 60 transactional
  Runner 2: 40 analytical + 60 transactional
  Runner 3: 20 analytical + 40 transactional + 40 bulk
```

### Timing Synchronization

All runners must start at the same wall-clock time for realistic timing:

```
Control node:
  1. Assign sessions to all runners (includes session data)
  2. Send "prepare" signal → runners load sessions into memory
  3. All runners ACK "prepared"
  4. Send "start at T=<wall_clock>" where T is NOW + 2 seconds
  5. All runners wait until T, then begin replay simultaneously
```

Stagger offsets (`--stagger-ms`) are baked into the session data before distribution — each runner's sessions already have the correct `start_offset_us` values.

### Result Collection

Runners stream results back as sessions complete (not all at once at the end):

```
Runner → Control:
  SessionComplete {
      runner_name: "runner-east-1",
      session_id: 42,
      query_results: [...],  // Vec<QueryResult>
      elapsed_us: 15000000,
  }
```

Control node merges into a single `Vec<ReplayResults>` compatible with existing `compare` pipeline.

## Communication Protocol

### Option A: HTTP + JSON (simpler)

```
POST /api/v1/register          Runner → Control: register
POST /api/v1/heartbeat         Runner → Control: still alive
GET  /api/v1/assignment        Runner ← Control: get session slice
POST /api/v1/ready             Runner → Control: sessions loaded
GET  /api/v1/start-signal      Runner ← Control: wait for start time (long-poll)
POST /api/v1/session-complete  Runner → Control: stream results
POST /api/v1/done              Runner → Control: all sessions finished
```

### Option B: gRPC + Protobuf (more efficient for streaming)

```protobuf
service ReplayControl {
    rpc Register(RegisterRequest) returns (RegisterResponse);
    rpc GetAssignment(AssignmentRequest) returns (stream SessionSlice);
    rpc ReportReady(ReadyRequest) returns (StartSignal);
    rpc StreamResults(stream SessionResult) returns (Ack);
}
```

### Recommendation: HTTP for Phase 1

HTTP/JSON is simpler, debuggable with curl, and sufficient for the orchestration pattern (low message frequency, results streamed per-session not per-query). Move to gRPC only if serialization overhead becomes a bottleneck.

## Fault Tolerance

| Failure | Handling |
|---------|---------|
| Runner disconnects mid-replay | Control marks its sessions as failed. Report includes partial results. Optionally reassign to another runner. |
| Runner never registers | Control waits up to `--wait-for-runners`, then proceeds with available runners (if >= `--min-runners`). |
| Control node crashes | Runners detect heartbeat timeout, stop replay, discard results. Must re-run. |
| Network partition | Runners continue replay but can't report. Results lost. Must re-run. |
| Target DB overloaded | Runners report high error rates. Control can signal early stop if `--max-error-rate` exceeded. |

Phase 1 prioritizes simplicity: failures stop the test, operator re-runs. Phase 2 adds reassignment and checkpointing.

## CLI Integration

```bash
# Full distributed replay workflow:

# 1. On worker hosts (can be in Docker/K8s):
pg-retest runner --control http://orchestrator:9090 --name worker-1 \
  --target "host=db port=5432 dbname=test user=replay"

pg-retest runner --control http://orchestrator:9090 --name worker-2 \
  --target "host=db port=5432 dbname=test user=replay"

# 2. On control node:
pg-retest orchestrate \
  --workload production.wkl \
  --listen 0.0.0.0:9090 \
  --output distributed-results.wkl \
  --scale 20 \
  --stagger-ms 200 \
  --read-only \
  --min-runners 2

# 3. Compare as usual:
pg-retest compare \
  --source production.wkl \
  --replay distributed-results.wkl \
  --json report.json \
  --fail-on-regression
```

## Module Structure

```
src/
  distributed/
    mod.rs           — Public API, config structs
    control.rs       — Orchestrator: HTTP server, session splitting, result merging
    runner.rs        — Runner agent: registration, replay execution, result streaming
    protocol.rs      — Shared message types (RegisterRequest, SessionSlice, etc.)
    sync.rs          — Timing synchronization (coordinated start)
```

## New Dependencies

```toml
# For HTTP communication between control and runners
axum = { version = "0.7", optional = true }       # Control node HTTP server
reqwest = { version = "0.12", optional = true }    # Runner HTTP client

[features]
distributed = ["axum", "reqwest"]
```

Behind a feature flag — most users won't need distributed replay.

## Relationship to SQL Gateway

The distributed runner architecture maps directly to the SQL Gateway's deployment model:
- **Control node** → Gateway orchestrator (manages the cluster)
- **Runner nodes** → Gateway worker nodes (handle agent requests)
- **Registration protocol** → Gateway node discovery
- **Session distribution** → Request routing across gateway nodes
- **Result merging** → Response aggregation from federated queries

Building distributed replay first proves the coordination patterns needed for the gateway cluster.
