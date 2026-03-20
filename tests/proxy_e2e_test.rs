//! End-to-end proxy tests against a real PostgreSQL instance.
//!
//! These tests require a PostgreSQL server running on localhost:5441
//! with database `pg_retest_e2e` accessible by user `sales_demo_app`.
//!
//! The proxy listens on ephemeral ports to avoid conflicts.

use pg_retest::proxy::capture::CaptureEvent;
use pg_retest::proxy::{run_proxy_managed, ProxyConfig};
use std::path::PathBuf;
use tempfile::TempDir;
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};
use tokio_postgres::NoTls;
use tokio_util::sync::CancellationToken;

const TARGET_ADDR: &str = "localhost:5441";
const CONN_STR: &str =
    "host=localhost port=5441 dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123";

/// Check if the test database is reachable.
async fn require_pg() -> bool {
    match tokio_postgres::connect(CONN_STR, NoTls).await {
        Ok((client, conn)) => {
            tokio::spawn(async move {
                let _ = conn.await;
            });
            let _ = client.simple_query("SELECT 1").await;
            true
        }
        Err(_) => {
            eprintln!("SKIP: PostgreSQL not available at localhost:5441");
            false
        }
    }
}

/// Find an available ephemeral port for the proxy to listen on.
async fn find_free_port() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Helper: start proxy on an ephemeral port, return (cancel_token, metrics_rx, join_handle, port, output_path)
async fn start_proxy(
    tmp_dir: &TempDir,
    mask_values: bool,
) -> (
    CancellationToken,
    mpsc::UnboundedReceiver<CaptureEvent>,
    tokio::task::JoinHandle<anyhow::Result<Option<pg_retest::profile::WorkloadProfile>>>,
    u16,
    PathBuf,
) {
    let port = find_free_port().await;
    let output_path = tmp_dir.path().join("proxy_capture.wkl");

    let config = ProxyConfig {
        listen_addr: format!("127.0.0.1:{}", port),
        target_addr: TARGET_ADDR.to_string(),
        output: Some(output_path.clone()),
        pool_size: 10,
        pool_timeout_secs: 5,
        mask_values,
        no_capture: false,
        duration: None,
        persistent: false,
        control_port: None,
    };

    let cancel_token = CancellationToken::new();
    let (metrics_tx, metrics_rx) = mpsc::unbounded_channel();

    let token_clone = cancel_token.clone();
    let handle =
        tokio::spawn(async move { run_proxy_managed(config, token_clone, metrics_tx).await });

    // Wait for proxy to be ready (accept connections)
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .is_ok()
        {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    (cancel_token, metrics_rx, handle, port, output_path)
}

// ─── Basic proxy relay: queries work through proxy ──────────────────

#[tokio::test]
async fn test_proxy_basic_relay() {
    if !require_pg().await {
        return;
    }

    let tmp_dir = TempDir::new().unwrap();
    let (cancel_token, _metrics_rx, handle, port, _output) = start_proxy(&tmp_dir, false).await;

    // Connect THROUGH the proxy
    let proxy_conn_str = format!(
        "host=localhost port={} dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123",
        port
    );

    let (client, conn) = tokio_postgres::connect(&proxy_conn_str, NoTls)
        .await
        .expect("Should connect through proxy");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // Run queries through the proxy
    let rows = client
        .query("SELECT count(*) FROM test_orders", &[])
        .await
        .expect("SELECT through proxy should work");
    let count: i64 = rows[0].get(0);
    assert!(count >= 3, "Should see test_orders data through proxy");

    // Run a few more queries
    client
        .simple_query("SELECT product, quantity FROM test_orders ORDER BY id")
        .await
        .expect("Second query through proxy should work");

    client
        .simple_query("SELECT 42 AS answer")
        .await
        .expect("Simple query through proxy should work");

    // Disconnect client, then stop proxy
    drop(client);
    sleep(Duration::from_millis(200)).await;
    cancel_token.cancel();

    let result = handle.await.expect("Proxy task should complete");
    let profile = result
        .expect("Proxy should return Ok")
        .expect("Should have captured profile");

    // Verify capture produced a valid profile
    assert!(
        profile.metadata.total_sessions >= 1,
        "Should have at least 1 session"
    );
    assert!(
        profile.metadata.total_queries >= 3,
        "Should have captured at least 3 queries"
    );
}

// ─── Proxy captures correct SQL text ────────────────────────────────

#[tokio::test]
async fn test_proxy_capture_sql_content() {
    if !require_pg().await {
        return;
    }

    let tmp_dir = TempDir::new().unwrap();
    let (cancel_token, _metrics_rx, handle, port, output) = start_proxy(&tmp_dir, false).await;

    let proxy_conn_str = format!(
        "host=localhost port={} dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123",
        port
    );

    let (client, conn) = tokio_postgres::connect(&proxy_conn_str, NoTls)
        .await
        .expect("Should connect through proxy");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // Run specific queries we can look for in the capture
    client
        .simple_query("SELECT 'proxy_capture_test_marker'")
        .await
        .unwrap();
    client.simple_query("SELECT 123 + 456").await.unwrap();

    drop(client);
    sleep(Duration::from_millis(200)).await;
    cancel_token.cancel();

    let result = handle.await.expect("Proxy task should complete");
    let profile = result.expect("Should be Ok").expect("Should have profile");

    // Find our marker queries in the captured workload
    let all_sqls: Vec<&str> = profile
        .sessions
        .iter()
        .flat_map(|s| s.queries.iter().map(|q| q.sql.as_str()))
        .collect();

    assert!(
        all_sqls
            .iter()
            .any(|sql| sql.contains("proxy_capture_test_marker")),
        "Should capture the marker query. Captured SQLs: {:?}",
        all_sqls
    );
    assert!(
        all_sqls.iter().any(|sql| sql.contains("123 + 456")),
        "Should capture the math query. Captured SQLs: {:?}",
        all_sqls
    );

    // Verify the .wkl file was written to disk
    assert!(
        output.exists(),
        "Profile should be written to disk at {:?}",
        output
    );

    // Verify we can read it back
    let loaded =
        pg_retest::profile::io::read_profile(&output).expect("Should read back the .wkl file");
    assert_eq!(
        loaded.metadata.total_sessions,
        profile.metadata.total_sessions
    );
}

// ─── Proxy with PII masking ─────────────────────────────────────────

#[tokio::test]
async fn test_proxy_capture_with_masking() {
    if !require_pg().await {
        return;
    }

    let tmp_dir = TempDir::new().unwrap();
    let (cancel_token, _metrics_rx, handle, port, _output) = start_proxy(&tmp_dir, true).await;

    let proxy_conn_str = format!(
        "host=localhost port={} dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123",
        port
    );

    let (client, conn) = tokio_postgres::connect(&proxy_conn_str, NoTls)
        .await
        .expect("Should connect through proxy");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    // Queries with literals that should be masked
    client
        .simple_query("SELECT * FROM test_orders WHERE product = 'widget' AND quantity = 10")
        .await
        .unwrap();

    drop(client);
    sleep(Duration::from_millis(200)).await;
    cancel_token.cancel();

    let result = handle.await.expect("Proxy task should complete");
    let profile = result.expect("Should be Ok").expect("Should have profile");

    // Find our query — literals should be masked
    let all_sqls: Vec<&str> = profile
        .sessions
        .iter()
        .flat_map(|s| s.queries.iter().map(|q| q.sql.as_str()))
        .collect();

    let masked_query = all_sqls
        .iter()
        .find(|sql| sql.contains("test_orders") && sql.contains("product"))
        .expect("Should find the test_orders query in capture");

    // String literal 'widget' should be replaced with $S
    assert!(
        !masked_query.contains("widget"),
        "String literal should be masked: {}",
        masked_query
    );
    assert!(
        masked_query.contains("$S"),
        "Should contain $S placeholder: {}",
        masked_query
    );
    // Numeric literal 10 should be replaced with $N
    assert!(
        masked_query.contains("$N"),
        "Should contain $N placeholder: {}",
        masked_query
    );
}

// ─── Multiple concurrent clients through proxy ──────────────────────

#[tokio::test]
async fn test_proxy_concurrent_clients() {
    if !require_pg().await {
        return;
    }

    let tmp_dir = TempDir::new().unwrap();
    let (cancel_token, _metrics_rx, handle, port, _output) = start_proxy(&tmp_dir, false).await;

    let proxy_conn_str = format!(
        "host=localhost port={} dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123",
        port
    );

    // Spawn 5 concurrent clients
    let mut client_handles = Vec::new();
    for i in 0..5 {
        let conn_str = proxy_conn_str.clone();
        let h = tokio::spawn(async move {
            let (client, conn) = tokio_postgres::connect(&conn_str, NoTls)
                .await
                .expect("Concurrent client should connect");
            tokio::spawn(async move {
                let _ = conn.await;
            });

            // Each client runs a few queries
            for j in 0..3 {
                let sql = format!("SELECT {} AS client_{}_query_{}", i * 100 + j, i, j);
                client
                    .simple_query(&sql)
                    .await
                    .unwrap_or_else(|e| panic!("Client {} query {} failed: {}", i, j, e));
            }
            drop(client);
        });
        client_handles.push(h);
    }

    // Wait for all clients to complete
    for h in client_handles {
        h.await.expect("Client task should complete");
    }

    sleep(Duration::from_millis(300)).await;
    cancel_token.cancel();

    let result = handle.await.expect("Proxy task should complete");
    let profile = result.expect("Should be Ok").expect("Should have profile");

    // Should have captured sessions from all 5 clients
    assert!(
        profile.metadata.total_sessions >= 5,
        "Should have at least 5 sessions (one per client), got {}",
        profile.metadata.total_sessions
    );
    assert!(
        profile.metadata.total_queries >= 15,
        "Should have at least 15 queries (5 clients x 3 queries), got {}",
        profile.metadata.total_queries
    );
}

// ─── Proxy no-capture mode ──────────────────────────────────────────

#[tokio::test]
async fn test_proxy_no_capture_mode() {
    if !require_pg().await {
        return;
    }

    let tmp_dir = TempDir::new().unwrap();
    let port = find_free_port().await;
    let output_path = tmp_dir.path().join("no_capture.wkl");

    let config = ProxyConfig {
        listen_addr: format!("127.0.0.1:{}", port),
        target_addr: TARGET_ADDR.to_string(),
        output: Some(output_path.clone()),
        pool_size: 10,
        pool_timeout_secs: 5,
        mask_values: false,
        no_capture: true,
        duration: None,
        persistent: false,
        control_port: None,
    };

    let cancel_token = CancellationToken::new();
    let (metrics_tx, _metrics_rx) = mpsc::unbounded_channel();

    let token_clone = cancel_token.clone();
    let handle =
        tokio::spawn(async move { run_proxy_managed(config, token_clone, metrics_tx).await });

    // Wait for proxy to be ready
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .is_ok()
        {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    // Connect and run a query — should work as a pass-through
    let proxy_conn_str = format!(
        "host=localhost port={} dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123",
        port
    );

    let (client, conn) = tokio_postgres::connect(&proxy_conn_str, NoTls)
        .await
        .expect("Should connect through proxy in no-capture mode");
    tokio::spawn(async move {
        let _ = conn.await;
    });

    let rows = client
        .query("SELECT 1 AS val", &[])
        .await
        .expect("Query should work");
    let val: i32 = rows[0].get(0);
    assert_eq!(val, 1);

    drop(client);
    sleep(Duration::from_millis(200)).await;
    cancel_token.cancel();

    let result = handle.await.expect("Proxy task should complete");
    let profile = result.expect("Should be Ok");
    assert!(profile.is_none(), "No-capture mode should return None");
}

// ─── Proxy with duration auto-shutdown ──────────────────────────────

#[tokio::test]
async fn test_proxy_duration_shutdown() {
    if !require_pg().await {
        return;
    }

    let tmp_dir = TempDir::new().unwrap();
    let port = find_free_port().await;
    let output_path = tmp_dir.path().join("duration.wkl");

    let config = ProxyConfig {
        listen_addr: format!("127.0.0.1:{}", port),
        target_addr: TARGET_ADDR.to_string(),
        output: Some(output_path.clone()),
        pool_size: 10,
        pool_timeout_secs: 5,
        mask_values: false,
        no_capture: false,
        duration: Some(std::time::Duration::from_secs(1)),
        persistent: false,
        control_port: None,
    };

    let cancel_token = CancellationToken::new();
    let (metrics_tx, _metrics_rx) = mpsc::unbounded_channel();

    let token_clone = cancel_token.clone();
    let handle =
        tokio::spawn(async move { run_proxy_managed(config, token_clone, metrics_tx).await });

    // Wait for proxy to be ready
    for _ in 0..50 {
        if tokio::net::TcpStream::connect(format!("127.0.0.1:{}", port))
            .await
            .is_ok()
        {
            break;
        }
        sleep(Duration::from_millis(50)).await;
    }

    // Run a query quickly
    let proxy_conn_str = format!(
        "host=localhost port={} dbname=pg_retest_e2e user=sales_demo_app password=salesdemo123",
        port
    );

    let (client, conn) = tokio_postgres::connect(&proxy_conn_str, NoTls)
        .await
        .expect("Should connect");
    tokio::spawn(async move {
        let _ = conn.await;
    });
    client.simple_query("SELECT 'duration_test'").await.unwrap();
    drop(client);

    // Proxy should auto-shutdown after 1 second
    let start = std::time::Instant::now();
    let result = handle.await.expect("Proxy task should complete");
    let elapsed = start.elapsed();

    assert!(
        elapsed < Duration::from_secs(5),
        "Proxy should shut down promptly, took {:?}",
        elapsed
    );
    let profile = result.expect("Should be Ok").expect("Should have profile");
    assert!(
        profile.metadata.total_queries >= 1,
        "Should have captured at least 1 query"
    );
}
