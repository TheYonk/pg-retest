# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**pg-retest** (working title for EDB Database Testing Kit / EDTK) is a tool for capturing, replaying, and scaling PostgreSQL database workloads. It enables users to validate performance across configuration changes, server migrations, and scaling scenarios.

### Core Capabilities (by milestone)

1. **Capture & Replay** — Capture SQL workload from a PG server (per-connection thread profiling), replay it against a backup database, produce side-by-side performance comparison. Support read-write and read-only (strip DML) modes.
2. **Scaled Benchmark** — Classify captured workload into categories (analytical, transactional, etc.), scale each independently to simulate increased traffic for capacity planning.
3. **CI/CD Integration** — Automate the capture/replay/compare cycle as a pipeline step with pass/fail thresholds.
4. **Cross-Database Capture** — Capture from Oracle, MySQL, MariaDB, SQL Server and transform into PG-compatible workload for replay.
5. **AI-Assisted Tuning** — Use AI to recommend config, schema, and query changes; test iterations and produce comparison reports.

### Key Design Constraints

- Workload capture must have minimal impact on production systems.
- Transactions change data, which changes query plans. For accurate 1:1 replay, restore from a point-in-time backup before replay.
- Two distinct modes are needed: **true replay** (exact 1:1 reproduction) and **simulated benchmark** (scaled workload generation).
- PII may appear in captured queries — the tool must support filtering/masking.
- Thread simulation fidelity degrades at high scale; benchmark mode accepts this tradeoff.

## Architecture

```
┌─────────────┐    ┌──────────────┐    ┌──────────────┐    ┌────────────┐
│   Capture    │───>│   Workload   │───>│    Replay     │───>│  Reporter  │
│   Agent      │    │   Profile    │    │    Engine     │    │            │
└─────────────┘    │   (storage)  │    └──────────────┘    └────────────┘
                   └──────────────┘
```

- **Capture Agent** — Connects to PG (via `pg_stat_activity` polling, log parsing, or proxy) to record per-connection SQL streams with timing metadata.
- **Workload Profile** — Serialized representation of captured workload: queries, connection/thread mapping, timing, dependencies, transaction boundaries.
- **Replay Engine** — Reads a workload profile and replays it against a target PG instance, preserving connection parallelism and timing. Supports replay modes (exact, read-only, scaled).
- **Reporter** — Compares source vs. replay metrics and produces a performance comparison report (per-query latency, throughput, errors, regressions).

## Build & Development

- **Language:** Rust (2021 edition)
- **Build:** `cargo build` (debug) / `cargo build --release`
- **Test all:** `cargo test`
- **Test single file:** `cargo test --test profile_io_test`
- **Test single function:** `cargo test --test profile_io_test test_profile_roundtrip_messagepack`
- **Test lib unit tests:** `cargo test --lib capture::csv_log`
- **Lint:** `cargo clippy`
- **Format:** `cargo fmt`
- **Run:** `cargo run -- <subcommand> [args]`
- **Verbose logging:** `RUST_LOG=debug cargo run -- -v <subcommand>`

### Crate Structure

The project is both a library (`src/lib.rs`) and binary (`src/main.rs`). Integration tests in `tests/` import from the library crate via `use pg_retest::...`. The binary crate handles CLI dispatch only.

Key modules:
- `capture::csv_log` — PG CSV log parser (pluggable backend via `CaptureSource` pattern)
- `profile` — Core data types (`WorkloadProfile`, `Session`, `Query`) + MessagePack I/O
- `replay::session` — Async per-session replay engine (Tokio + tokio-postgres)
- `compare` — Performance comparison logic + terminal/JSON reporting
- `cli` — Clap derive-based CLI argument structs

## Milestone Status

- **M1: Capture & Replay** — Complete. CSV log capture, async replay, comparison reports.
- **M2: Scaled Benchmark** — Not started. Workload classification + scaled replay.
- **M3: CI/CD Integration** — Not started. Automation + pass/fail thresholds.
- **M4: Cross-Database Capture** — Not started. Oracle/MySQL/MariaDB capture + transform.
- **M5: AI-Assisted Tuning** — Not started.

## Gotchas

- All `pub mod` declarations go in `src/lib.rs`, not `src/main.rs` — integration tests import from the library crate.
- PG CSV log timestamps (`2024-03-08 10:00:00.100 UTC`) are not RFC 3339 — the parser has a fallback via `NaiveDateTime`.
- Capture backends are pluggable: implement parsing in `src/capture/`, the profile format and replay engine don't change.
- Always run `cargo fmt` after writing code — the formatter's output may differ from hand-written style.
- `.wkl` files are MessagePack binary. Use `pg-retest inspect file.wkl` to view as JSON.

## Conventions

- Target PostgreSQL as the primary replay destination for all milestones.
- Workload profiles should be a portable, version-stamped format (not tied to a specific PG version).
- Capture and replay must be decoupled — capture produces a profile file; replay consumes it. They should never require simultaneous access to source and target.
- Connection-level parallelism in replay is critical for realistic results; avoid serializing inherently parallel workloads.
- Configuration changes and server differences are the variables under test — the tool itself should introduce minimal overhead or variance.
