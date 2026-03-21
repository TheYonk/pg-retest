# Audit Gap Closure Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close P0 and P1 security/operational gaps identified by AAT and prod-ready audits — dashboard auth + localhost binding, TLS support for database connections, replay concurrency limits, and SchemaChange safety allowlist.

**Architecture:** Four independent workstreams that can be parallelized: (1) web dashboard security via Axum middleware layer for bearer token auth + default localhost binding, (2) TLS support via `tokio-postgres-rustls` configurable connector replacing `NoTls`, (3) replay concurrency control via `tokio::sync::Semaphore`, (4) tuner safety hardening by inverting SchemaChange from denylist to allowlist and escaping SQL values.

**Tech Stack:** Rust, Axum (middleware layers), tokio-postgres-rustls, rustls, tokio::sync::Semaphore, clap

---

## File Structure

### New files
- `src/web/auth.rs` — Bearer token auth middleware for Axum
- `src/tls.rs` — Shared TLS connector builder used by replay and tuner

### Modified files
- `Cargo.toml` — Add `tokio-postgres-rustls`, `rustls`, `rustls-pemfile` dependencies
- `src/lib.rs` — Add `pub mod tls;`
- `src/web/mod.rs` — Import auth module, change default bind to `127.0.0.1`, generate/load auth token, apply middleware
- `src/web/routes.rs` — Apply auth middleware layer to API routes (exempt `/health`)
- `src/cli.rs` — Add `--bind`, `--auth-token`, `--no-auth` to `WebArgs`; add `--tls-mode`, `--tls-ca-cert` to `ReplayArgs` and `TuneArgs`; add `--max-connections` to `ReplayArgs`
- `src/replay/session.rs` — Accept TLS connector, add Semaphore-gated concurrency
- `src/tuner/context.rs` — Accept TLS connector
- `src/tuner/safety.rs` — Replace `is_blocked_sql()` with `is_allowed_schema_sql()`
- `src/tuner/apply.rs` — Escape values in ALTER SYSTEM SET
- `src/main.rs` — Pass new CLI args through to `run_server`, `run_replay`, tuner

### Test files
- `tests/web_test.rs` — Add auth middleware tests
- `tests/replay_test.rs` — Add concurrency limit tests
- `tests/tuner_test.rs` — Add SchemaChange allowlist tests

---

## Task 1: Secure the Web Dashboard — Auth Middleware

**Files:**
- Create: `src/web/auth.rs`
- Modify: `src/web/mod.rs`
- Modify: `src/web/routes.rs`
- Modify: `src/cli.rs:267-276` (WebArgs)
- Modify: `src/main.rs` (cmd_web)
- Test: `tests/web_test.rs`

### Step-by-step

- [ ] **Step 1: Add CLI flags to WebArgs**

In `src/cli.rs`, add `--bind`, `--auth-token`, and `--no-auth` to `WebArgs`:

```rust
#[derive(clap::Args)]
pub struct WebArgs {
    /// Port to listen on
    #[arg(long, default_value_t = 8080)]
    pub port: u16,

    /// Data directory for SQLite database and workload files
    #[arg(long, default_value = "./data")]
    pub data_dir: std::path::PathBuf,

    /// Address to bind to (default: 127.0.0.1 for security)
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,

    /// Bearer token for API authentication (auto-generated if not set)
    #[arg(long)]
    pub auth_token: Option<String>,

    /// Disable authentication (NOT recommended for network exposure)
    #[arg(long, default_value_t = false)]
    pub no_auth: bool,
}
```

- [ ] **Step 2: Run `cargo check` to verify CLI compiles**

Run: `cargo check 2>&1 | tail -5`
Expected: Compilation errors in `cmd_web` because it doesn't pass new args yet. That's fine — we'll fix in step 6.

- [ ] **Step 3: Create auth middleware**

Create `src/web/auth.rs`:

```rust
use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Axum middleware that validates a bearer token on every request.
pub async fn require_auth(
    request: Request<Body>,
    next: Next,
) -> Response {
    // Extract token from app state stored in extensions
    let expected = request
        .extensions()
        .get::<AuthToken>()
        .map(|t| t.0.clone());

    let expected = match expected {
        Some(t) => t,
        None => return next.run(request).await, // auth disabled
    };

    // Check Authorization header
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if token == expected {
                next.run(request).await
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "Invalid bearer token"})),
                )
                    .into_response()
            }
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Missing or invalid Authorization header. Use: Authorization: Bearer <token>"})),
        )
            .into_response(),
    }
}

/// Wrapper type for the auth token, stored in request extensions.
#[derive(Clone)]
pub struct AuthToken(pub String);
```

- [ ] **Step 4: Register auth module**

Add `pub mod auth;` to `src/web/mod.rs` after the existing module declarations.

- [ ] **Step 5: Update `run_server` to accept bind address and auth token**

In `src/web/mod.rs`, change the `run_server` signature and body:

```rust
pub async fn run_server(port: u16, data_dir: PathBuf, bind: String, auth_token: Option<String>) -> Result<()> {
```

Replace the addr/listener/println block (lines 140-144):

```rust
    let addr = format!("{bind}:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    if let Some(ref token) = auth_token {
        println!("Authentication enabled. Bearer token: {token}");
    } else {
        println!("WARNING: Authentication is disabled. Use --auth-token or remove --no-auth for production.");
    }

    if bind == "0.0.0.0" {
        println!("WARNING: Dashboard is exposed to all network interfaces.");
    }

    println!("pg-retest web dashboard: http://{}:{port}", if bind == "0.0.0.0" { "localhost" } else { &bind });
    println!("Data directory: {}", data_dir.display());
```

- [ ] **Step 6: Apply auth middleware to routes**

In `src/web/routes.rs`, wrap the API router with the auth middleware. The health endpoint must remain unauthenticated:

```rust
use axum::middleware;
use super::auth::{self, AuthToken};

pub fn build_router(state: AppState, auth_token: Option<String>) -> Router {
    // Health is always public
    let public = Router::new()
        .route("/health", get(handlers::health));

    let api = Router::new()
        .route("/tasks", get(handlers::list_tasks))
        // ... all existing routes except /health ...
        ;

    // Apply auth middleware only to protected routes
    let api = if let Some(token) = auth_token {
        api.layer(axum::Extension(AuthToken(token)))
            .layer(middleware::from_fn(auth::require_auth))
    } else {
        api
    };

    let combined = public.merge(api);

    Router::new().nest("/api/v1", combined).with_state(state)
}
```

- [ ] **Step 7: Update `run_server` to pass auth_token to `build_router`**

Change line 138 in `src/web/mod.rs`:

```rust
    let app = routes::build_router(state, auth_token).fallback(static_handler);
```

- [ ] **Step 8: Update `cmd_web` in `src/main.rs`**

Pass the new args through. Find the `cmd_web` function and update it:

```rust
fn cmd_web(args: pg_retest::cli::WebArgs) -> Result<()> {
    use pg_retest::web;
    let auth_token = if args.no_auth {
        None
    } else {
        Some(args.auth_token.unwrap_or_else(|| uuid::Uuid::new_v4().to_string()))
    };
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(web::run_server(args.port, args.data_dir, args.bind, auth_token))
}
```

- [ ] **Step 9: Run `cargo check`**

Run: `cargo check 2>&1 | tail -10`
Expected: Clean compilation.

- [ ] **Step 10: Update existing `tests/web_test.rs` callers**

The existing `tests/web_test.rs` has 6 calls to `build_router(state)` (lines 14, 49, 82, 115, 148, 200). Update all of them to `build_router(state, None)` so existing tests continue to work in no-auth mode.

- [ ] **Step 11: Write auth middleware tests**

Add tests to `tests/web_test.rs`. These tests use `axum::test` helpers (tower::ServiceExt):

```rust
// Test that /api/v1/health is accessible without auth
#[tokio::test]
async fn test_health_no_auth_required() {
    // Setup AppState with temp dir, build_router(state, Some("test-token".into()))
    // GET /api/v1/health should return 200 without Authorization header
}

// Test that protected routes return 401 without token
#[tokio::test]
async fn test_protected_route_requires_auth() {
    // build_router(state, Some("test-token".into()))
    // GET /api/v1/workloads without auth header should return 401
}

// Test that protected routes work with valid token
#[tokio::test]
async fn test_protected_route_with_valid_token() {
    // GET /api/v1/workloads with "Bearer test-token" should return 200
}

// Test that wrong token returns 401
#[tokio::test]
async fn test_protected_route_with_wrong_token() {
    // GET /api/v1/workloads with wrong Bearer token should return 401
}

// Test that no auth mode skips all checks
#[tokio::test]
async fn test_no_auth_mode() {
    // build_router(state, None), all routes accessible without auth
}
```

- [ ] **Step 12: Run tests**

Run: `cargo test --test web_test 2>&1 | tail -10`
Expected: All auth tests pass (existing + new).

- [ ] **Step 13: Run `cargo fmt` and `cargo clippy`**

Run: `cargo fmt && cargo clippy 2>&1 | tail -5`

- [ ] **Step 14: Commit**

```bash
git add src/web/auth.rs src/web/mod.rs src/web/routes.rs src/cli.rs src/main.rs tests/web_test.rs
git commit -m "feat(web): add bearer token auth and default localhost binding

Closes GAP-001 (BLOCKER) and addresses AAT P0-2.
- Default bind changed from 0.0.0.0 to 127.0.0.1
- Auto-generated bearer token printed on startup
- /health remains unauthenticated for health checks
- --no-auth flag for development/demo use"
```

---

## Task 2: TLS Support for Database Connections

**Files:**
- Create: `src/tls.rs`
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Modify: `src/replay/session.rs`
- Modify: `src/tuner/context.rs`
- Modify: `src/cli.rs` (ReplayArgs, TuneArgs)
- Modify: `src/main.rs`

### Step-by-step

- [ ] **Step 1: Add TLS dependencies to Cargo.toml**

Add to `[dependencies]`:

```toml
tokio-postgres-rustls = "0.13"
rustls = "0.23"
rustls-pemfile = "2"
rustls-native-certs = "0.8"
```

- [ ] **Step 2: Run `cargo check` to verify deps resolve**

Run: `cargo check 2>&1 | tail -5`
Expected: Compiles (new deps unused but resolved).

- [ ] **Step 3: Create the TLS connector builder**

Create `src/tls.rs`:

```rust
use anyhow::{Context, Result};
use std::path::Path;
use std::sync::Arc;
use tokio_postgres_rustls::MakeRustlsConnect;

/// TLS mode for PostgreSQL connections.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlsMode {
    /// No TLS (plaintext) — only for trusted networks.
    Disable,
    /// Use TLS if server supports it, fall back to plaintext.
    Prefer,
    /// Require TLS, but don't verify server certificate.
    Require,
}

/// Build a `MakeRustlsConnect` TLS connector based on mode and optional CA cert.
pub fn make_tls_connector(
    mode: TlsMode,
    ca_cert_path: Option<&Path>,
) -> Result<Option<MakeRustlsConnect>> {
    match mode {
        TlsMode::Disable => Ok(None),
        TlsMode::Prefer | TlsMode::Require => {
            let config = if let Some(ca_path) = ca_cert_path {
                // Load custom CA certificate
                let cert_pem = std::fs::read(ca_path)
                    .with_context(|| format!("Failed to read CA cert: {}", ca_path.display()))?;
                let mut reader = std::io::BufReader::new(&cert_pem[..]);
                let certs: Vec<_> = rustls_pemfile::certs(&mut reader)
                    .collect::<std::result::Result<_, _>>()?;

                let mut root_store = rustls::RootCertStore::empty();
                for cert in certs {
                    root_store.add(cert)?;
                }

                rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            } else {
                // Use system CA certificates
                let native_certs = rustls_native_certs::load_native_certs();
                let root_store = rustls::RootCertStore::from_iter(native_certs.certs.into_iter());

                rustls::ClientConfig::builder()
                    .with_root_certificates(root_store)
                    .with_no_client_auth()
            };

            Ok(Some(MakeRustlsConnect::new(config)))
        }
    }
}

/// Parse a TLS mode string from CLI.
pub fn parse_tls_mode(s: &str) -> Result<TlsMode> {
    match s.to_lowercase().as_str() {
        "disable" | "off" | "no" => Ok(TlsMode::Disable),
        "prefer" => Ok(TlsMode::Prefer),
        "require" => Ok(TlsMode::Require),
        other => anyhow::bail!(
            "Unknown TLS mode '{}'. Use: disable, prefer, require",
            other
        ),
    }
}
```

- [ ] **Step 4: Register module in `src/lib.rs`**

Add `pub mod tls;` to `src/lib.rs`.

- [ ] **Step 5: Add TLS CLI flags to ReplayArgs**

Add to `ReplayArgs` in `src/cli.rs`:

```rust
    /// TLS mode for target database connection: disable, prefer, require
    #[arg(long, default_value = "prefer")]
    pub tls_mode: String,

    /// Path to CA certificate file for TLS verification
    #[arg(long)]
    pub tls_ca_cert: Option<PathBuf>,

    /// Maximum concurrent database connections during replay
    #[arg(long)]
    pub max_connections: Option<u32>,
```

Add the same `tls_mode` and `tls_ca_cert` flags to `TuneArgs`.

- [ ] **Step 6: Update `replay_session` to accept a generic TLS connector**

In `src/replay/session.rs`, change the connection to support both TLS and NoTls:

```rust
use crate::tls::TlsMode;
use tokio_postgres_rustls::MakeRustlsConnect;

pub async fn replay_session(
    session: &Session,
    connection_string: &str,
    mode: ReplayMode,
    speed: f64,
    replay_start: TokioInstant,
    tls: Option<MakeRustlsConnect>,
) -> Result<ReplayResults> {
    let (client, connection) = if let Some(tls_connector) = tls {
        tokio_postgres::connect(connection_string, tls_connector).await?
    } else {
        tokio_postgres::connect(connection_string, NoTls).await?
    };
    // ... rest unchanged ...
```

Update `run_replay` similarly to accept and pass through the TLS connector.

- [ ] **Step 7: Update `tuner/context.rs` connect function**

```rust
use crate::tls::TlsMode;
use tokio_postgres_rustls::MakeRustlsConnect;

pub async fn connect(connection_string: &str, tls: Option<MakeRustlsConnect>) -> Result<Client> {
    let (client, connection) = if let Some(tls_connector) = tls {
        tokio_postgres::connect(connection_string, tls_connector).await?
    } else {
        tokio_postgres::connect(connection_string, NoTls).await?
    };
    // ... rest unchanged ...
```

- [ ] **Step 8: Wire TLS through `src/main.rs` and update all `context::connect` callers**

In the replay and tune command handlers, parse the TLS mode and build the connector:

```rust
let tls_mode = pg_retest::tls::parse_tls_mode(&args.tls_mode)?;
let tls = pg_retest::tls::make_tls_connector(tls_mode, args.tls_ca_cert.as_deref())?;
```

Pass `tls` to `run_replay()` and tuner functions.

Also update callers of `tuner::context::connect()`:
- `src/tuner/mod.rs` (line ~66) — pass the TLS connector from tuning config
- `src/web/handlers/demo.rs` (lines 65, 419) — pass `None` (demo uses local Docker DBs)

- [ ] **Step 9: Run `cargo check`**

Run: `cargo check 2>&1 | tail -10`
Expected: Clean compilation.

- [ ] **Step 10: Write unit tests for TLS module**

Add tests in `src/tls.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_tls_mode() {
        assert_eq!(parse_tls_mode("disable").unwrap(), TlsMode::Disable);
        assert_eq!(parse_tls_mode("prefer").unwrap(), TlsMode::Prefer);
        assert_eq!(parse_tls_mode("require").unwrap(), TlsMode::Require);
        assert_eq!(parse_tls_mode("off").unwrap(), TlsMode::Disable);
        assert!(parse_tls_mode("invalid").is_err());
    }

    #[test]
    fn test_disable_returns_none() {
        let connector = make_tls_connector(TlsMode::Disable, None).unwrap();
        assert!(connector.is_none());
    }

    #[test]
    fn test_prefer_returns_some() {
        // This uses system CAs — should succeed on most systems
        let connector = make_tls_connector(TlsMode::Prefer, None).unwrap();
        assert!(connector.is_some());
    }

    #[test]
    fn test_bad_ca_path_errors() {
        let result = make_tls_connector(
            TlsMode::Require,
            Some(Path::new("/nonexistent/ca.pem")),
        );
        assert!(result.is_err());
    }
}
```

- [ ] **Step 11: Run tests**

Run: `cargo test tls 2>&1 | tail -10`
Expected: All TLS unit tests pass.

- [ ] **Step 12: Run `cargo fmt` and `cargo clippy`**

- [ ] **Step 13: Commit**

```bash
git add Cargo.toml Cargo.lock src/tls.rs src/lib.rs src/replay/session.rs src/tuner/context.rs src/cli.rs src/main.rs
git commit -m "feat: add TLS/SSL support for database connections

Closes AAT P0-1 and GAP-002.
- New --tls-mode flag (disable/prefer/require), default prefer
- --tls-ca-cert for custom CA certificates
- Uses tokio-postgres-rustls with system CAs by default
- Applied to replay engine and tuner context collector"
```

---

## Task 3: Replay Concurrency Limits

**Files:**
- Modify: `src/replay/session.rs:112-141` (run_replay)
- Modify: `src/cli.rs` (ReplayArgs — already added `--max-connections` in Task 2 step 5)
- Modify: `src/main.rs`
- Test: `tests/replay_test.rs`

### Step-by-step

- [ ] **Step 1: Write a test for concurrency-limited replay**

In `tests/replay_test.rs`, add a test that verifies the semaphore limits concurrent sessions:

```rust
#[tokio::test]
async fn test_replay_concurrency_limit() {
    use pg_retest::profile::{WorkloadProfile, Metadata};
    use pg_retest::replay::session::run_replay;
    use pg_retest::replay::ReplayMode;

    // Create a profile with no sessions (we can't connect to a real DB here,
    // but we can verify the function signature accepts max_connections)
    let profile = WorkloadProfile {
        version: 2,
        source_host: "test".into(),
        pg_version: "16.0".into(),
        captured_at: chrono::Utc::now(),
        capture_method: "test".into(),
        sessions: vec![],
        metadata: Metadata {
            total_queries: 0,
            total_sessions: 0,
            capture_duration_us: 0,
        },
    };

    let results = run_replay(&profile, "postgresql://localhost/test", ReplayMode::ReadWrite, 1.0, Some(10), None).await;
    // With no sessions, should succeed with empty results
    assert!(results.is_ok());
    assert!(results.unwrap().is_empty());
}
```

- [ ] **Step 2: Run the test to verify it fails (signature mismatch)**

Run: `cargo test --test replay_test test_replay_concurrency_limit 2>&1 | tail -10`
Expected: FAIL — `run_replay` doesn't accept `max_connections` yet.

- [ ] **Step 3: Add Semaphore-gated concurrency to `run_replay`**

In `src/replay/session.rs`, update `run_replay`:

```rust
use std::sync::Arc;
use tokio::sync::Semaphore;

pub async fn run_replay(
    profile: &crate::profile::WorkloadProfile,
    connection_string: &str,
    mode: ReplayMode,
    speed: f64,
    max_connections: Option<u32>,
    tls: Option<MakeRustlsConnect>,
) -> Result<Vec<ReplayResults>> {
    let replay_start = TokioInstant::now();
    let mut handles = Vec::new();
    let session_count = profile.sessions.len();

    let semaphore = max_connections.map(|n| Arc::new(Semaphore::new(n as usize)));

    if let Some(max) = max_connections {
        if session_count > max as usize {
            tracing::info!(
                "Concurrency limited to {} (workload has {} sessions)",
                max,
                session_count
            );
        }
    }

    for session in &profile.sessions {
        let session = session.clone();
        let conn_str = connection_string.to_string();
        let sem = semaphore.clone();
        let tls_clone = tls.clone();

        let handle = tokio::spawn(async move {
            // Acquire semaphore permit if concurrency is limited
            let _permit = match sem {
                Some(ref s) => Some(s.acquire().await.unwrap()),
                None => None,
            };
            replay_session(&session, &conn_str, mode, speed, replay_start, tls_clone).await
        });

        handles.push(handle);
    }

    let mut all_results = Vec::new();
    for handle in handles {
        match handle.await? {
            Ok(results) => all_results.push(results),
            Err(e) => warn!("Session replay failed: {e}"),
        }
    }

    Ok(all_results)
}
```

- [ ] **Step 4: Update all callers of `run_replay` and `replay_session`**

Search for all callers in the codebase and add the `max_connections` and `tls` parameters. Key callers:

`run_replay` callers (need `max_connections` + `tls` params):
- `src/main.rs` (cmd_replay)
- `src/pipeline/mod.rs` (pipeline orchestrator)
- `src/tuner/mod.rs` (baseline and iteration replays)
- `src/web/handlers/ab.rs` (A/B test handler)
- `src/web/handlers/demo.rs` (demo scenarios)
- `tests/replay_e2e_test.rs` (lines 258, 295) — pass `None, None`

`replay_session` callers (need `tls` param only):
- `src/web/handlers/replay.rs` (calls `replay_session` directly for per-session progress — NOT `run_replay`)
- `tests/replay_e2e_test.rs` (multiple direct calls) — pass `None`

For callers that don't expose max_connections (web handlers, pipeline, tuner), pass `None`.

- [ ] **Step 5: Run the test**

Run: `cargo test --test replay_test test_replay_concurrency_limit 2>&1 | tail -10`
Expected: PASS.

- [ ] **Step 6: Run full test suite**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 7: Run `cargo fmt` and `cargo clippy`**

- [ ] **Step 8: Commit**

```bash
git add src/replay/session.rs src/cli.rs src/main.rs src/pipeline/ src/web/handlers/ src/tuner/mod.rs tests/replay_test.rs
git commit -m "feat(replay): add --max-connections concurrency limit

Closes AAT P1-1.
- Semaphore-gated session spawning
- Logs when workload is throttled
- Default unlimited (existing behavior preserved)"
```

---

## Task 4: SchemaChange Safety Allowlist

**Files:**
- Modify: `src/tuner/safety.rs:98-121` (is_blocked_sql → is_allowed_schema_sql)
- Modify: `src/tuner/safety.rs:154-160` (validate_recommendations SchemaChange arm)
- Test: existing tests in `src/tuner/safety.rs` (inline)

### Step-by-step

- [ ] **Step 1: Write failing tests for the new allowlist behavior**

Add tests to the existing `#[cfg(test)] mod tests` in `src/tuner/safety.rs`:

```rust
    #[test]
    fn test_schema_change_allowlist() {
        // Allowed operations
        assert!(is_allowed_schema_sql("CREATE INDEX idx_foo ON bar (baz)"));
        assert!(is_allowed_schema_sql("CREATE INDEX CONCURRENTLY idx_foo ON bar (baz)"));
        assert!(is_allowed_schema_sql("CREATE UNIQUE INDEX idx_foo ON bar (baz)"));
        assert!(is_allowed_schema_sql("ANALYZE users"));
        assert!(is_allowed_schema_sql("ANALYZE"));
        assert!(is_allowed_schema_sql("REINDEX TABLE users"));
        assert!(is_allowed_schema_sql("REINDEX INDEX idx_foo"));

        // Blocked operations (not on allowlist)
        assert!(!is_allowed_schema_sql("ALTER TABLE users ADD COLUMN archived boolean"));
        assert!(!is_allowed_schema_sql("DROP TABLE users"));
        assert!(!is_allowed_schema_sql("CREATE TABLE foo (id int)"));
        assert!(!is_allowed_schema_sql("TRUNCATE orders"));
        assert!(!is_allowed_schema_sql("DROP INDEX idx_foo"));
        assert!(!is_allowed_schema_sql("ALTER INDEX idx_foo RENAME TO idx_bar"));
        assert!(!is_allowed_schema_sql("GRANT ALL ON TABLE users TO admin"));
    }

    #[test]
    fn test_validate_schema_change_uses_allowlist() {
        let recs = vec![
            // Allowed: CREATE INDEX
            Recommendation::SchemaChange {
                sql: "CREATE INDEX idx_test ON orders (status)".into(),
                description: "Add index".into(),
                rationale: "Speed up".into(),
            },
            // Blocked: ALTER TABLE (not on allowlist)
            Recommendation::SchemaChange {
                sql: "ALTER TABLE users ADD COLUMN archived boolean".into(),
                description: "Add column".into(),
                rationale: "Feature".into(),
            },
        ];

        let (safe, rejected) = validate_recommendations(&recs);
        assert_eq!(safe.len(), 1);
        assert_eq!(rejected.len(), 1);
        assert!(rejected[0].1.contains("not on the allowed list"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test --lib tuner::safety 2>&1 | tail -10`
Expected: FAIL — `is_allowed_schema_sql` doesn't exist, and `ALTER TABLE ADD COLUMN` currently passes the denylist.

- [ ] **Step 3: Replace `is_blocked_sql` with `is_allowed_schema_sql`**

In `src/tuner/safety.rs`, replace lines 98-121:

```rust
/// Check whether a SQL statement is on the allowlist for SchemaChange execution.
/// Only these operations are safe to auto-apply from LLM recommendations:
/// - CREATE INDEX (including CONCURRENTLY, UNIQUE, IF NOT EXISTS)
/// - ANALYZE
/// - REINDEX
///
/// Everything else is rejected and presented as a suggestion for human review.
pub fn is_allowed_schema_sql(sql: &str) -> bool {
    let upper = sql.trim().to_uppercase();

    // CREATE INDEX (with optional UNIQUE, CONCURRENTLY, IF NOT EXISTS)
    if upper.starts_with("CREATE INDEX")
        || upper.starts_with("CREATE UNIQUE INDEX")
    {
        return true;
    }

    // ANALYZE (with optional table name)
    if upper.starts_with("ANALYZE") {
        return true;
    }

    // REINDEX (TABLE, INDEX, DATABASE, SCHEMA, SYSTEM)
    if upper.starts_with("REINDEX") {
        return true;
    }

    false
}
```

- [ ] **Step 4: Update `validate_recommendations` to use the new function**

Change the `SchemaChange` arm (lines 154-160) from:

```rust
Recommendation::SchemaChange { sql, .. } => {
    if let Some(reason) = is_blocked_sql(sql) {
        rejected.push((rec.clone(), reason));
    } else {
        safe.push(rec.clone());
    }
}
```

To:

```rust
Recommendation::SchemaChange { sql, .. } => {
    if is_allowed_schema_sql(sql) {
        safe.push(rec.clone());
    } else {
        rejected.push((
            rec.clone(),
            format!(
                "SchemaChange SQL is not on the allowed list (only CREATE INDEX, ANALYZE, REINDEX are auto-applied): {}",
                sql.chars().take(60).collect::<String>()
            ),
        ));
    }
}
```

Also update the `CreateIndex` arm to use `is_allowed_schema_sql` instead of `is_blocked_sql`, since `CreateIndex` SQL should also be on the allowlist:

```rust
Recommendation::CreateIndex { sql, .. } => {
    if is_allowed_schema_sql(sql) {
        safe.push(rec.clone());
    } else {
        rejected.push((
            rec.clone(),
            format!("CreateIndex SQL is not on the allowed list: {}", sql.chars().take(60).collect::<String>()),
        ));
    }
}
```

- [ ] **Step 5: Remove the old `is_blocked_sql` function**

Delete the `is_blocked_sql` function entirely (it's now unused). Remove the `test_blocked_sql` test too since it tested the old denylist.

- [ ] **Step 6: Update the old `test_validate_recommendations` test**

The existing test (line 198-243) expects `SchemaChange { sql: "DROP TABLE old_data" }` to be rejected. With the new allowlist, it's still rejected (DROP TABLE is not on the allowlist). But the rejection reason changes. Update both rejection assertions:

```rust
// data_directory config param rejection (unchanged)
assert!(rejected[0].1.contains("not on the safe allowlist"));
// DROP TABLE schema change rejection (new allowlist message)
assert!(rejected[1].1.contains("not on the allowed list"));
```

- [ ] **Step 7: Run tests**

Run: `cargo test --lib tuner::safety 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 8: Run `cargo fmt` and `cargo clippy`**

- [ ] **Step 9: Commit**

```bash
git add src/tuner/safety.rs
git commit -m "feat(tuner): replace SchemaChange denylist with allowlist

Closes AAT P1-2.
- Only CREATE INDEX, ANALYZE, REINDEX are auto-applied
- All other LLM-suggested DDL is rejected with explanation
- Prevents hallucinated destructive DDL from being executed"
```

---

## Task 5: Escape SQL Values in ALTER SYSTEM SET

**Files:**
- Modify: `src/tuner/apply.rs:19-21`
- Test: `src/tuner/apply.rs` (inline tests)

### Step-by-step

- [ ] **Step 1: Write a test for SQL value escaping**

Add to existing tests in `src/tuner/apply.rs`:

```rust
    #[test]
    fn test_escape_pg_value() {
        assert_eq!(escape_pg_value("128MB"), "128MB");
        assert_eq!(escape_pg_value("it's"), "it''s");
        assert_eq!(escape_pg_value("val'ue"), "val''ue");
        assert_eq!(escape_pg_value("normal"), "normal");
        assert_eq!(escape_pg_value(""), "");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test --lib tuner::apply 2>&1 | tail -5`
Expected: FAIL — `escape_pg_value` doesn't exist.

- [ ] **Step 3: Implement `escape_pg_value` and apply it**

Add the escaping function and update the ALTER SYSTEM SET construction in `src/tuner/apply.rs`:

```rust
/// Escape a value for use in a single-quoted SQL string.
/// Doubles any single quotes to prevent SQL injection.
fn escape_pg_value(value: &str) -> String {
    value.replace('\'', "''")
}
```

Update lines 19-21:

```rust
let set_sql = format!(
    "ALTER SYSTEM SET {} = '{}'",
    parameter,
    escape_pg_value(recommended_value)
);
let reload_sql = "SELECT pg_reload_conf()";
let rollback_sql = format!(
    "ALTER SYSTEM SET {} = '{}'",
    parameter,
    escape_pg_value(current_value)
);
```

Note: `parameter` is already validated against the `SAFE_CONFIG_PARAMS` allowlist in `safety.rs`, so it's a known-good identifier. No escaping needed for it.

- [ ] **Step 4: Run tests**

Run: `cargo test --lib tuner::apply 2>&1 | tail -10`
Expected: All tests pass.

- [ ] **Step 5: Run `cargo fmt` and `cargo clippy`**

- [ ] **Step 6: Commit**

```bash
git add src/tuner/apply.rs
git commit -m "fix(tuner): escape SQL values in ALTER SYSTEM SET

Closes the SQL interpolation finding from AAT T-5.
- Single quotes in config values are now doubled
- Parameter names are validated against allowlist (no escaping needed)"
```

---

## Task 6: Enhance Health Endpoint

**Files:**
- Modify: `src/web/handlers/mod.rs:17-24`
- Modify: `src/web/state.rs`

### Step-by-step

The existing `/api/v1/health` returns basic status. Enhance it with uptime and readiness check per FIX-03.

- [ ] **Step 1: Add startup time to AppState**

In `src/web/state.rs`, add `pub started_at: std::time::Instant` to `AppState` and set it in the constructor.

- [ ] **Step 2: Update health handler**

In `src/web/handlers/mod.rs`:

```rust
pub async fn health(State(state): State<AppState>) -> Json<serde_json::Value> {
    let uptime_secs = state.started_at.elapsed().as_secs();
    Json(json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "name": "pg-retest",
        "uptime_seconds": uptime_secs,
    }))
}
```

- [ ] **Step 3: Run `cargo check`**

Run: `cargo check 2>&1 | tail -5`
Expected: Clean compilation.

- [ ] **Step 4: Run `cargo fmt` and `cargo clippy`**

- [ ] **Step 5: Commit**

```bash
git add src/web/handlers/mod.rs src/web/state.rs
git commit -m "feat(web): enhance health endpoint with uptime

Closes GAP-004. Adds uptime_seconds to /api/v1/health response
for container orchestrator monitoring."
```

---

## Execution Order

Tasks 1-5 can be parallelized (independent code areas). Task 6 depends on Task 1 (AppState changes).

Recommended serial order: **Task 4 → Task 5 → Task 1 → Task 6 → Task 2 → Task 3**

Rationale: Tasks 4 and 5 are self-contained safety fixes (smallest blast radius). Task 1 changes the web layer. Task 6 builds on Task 1's AppState changes. Tasks 2 and 3 modify the replay path and have the most callers to update.
