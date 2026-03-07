pub mod capture;
pub mod connection;
pub mod listener;
pub mod pool;
pub mod protocol;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::info;

use self::capture::{build_profile, run_collector, CaptureEvent};
use self::pool::SessionPool;
use crate::profile::io;
use crate::profile::WorkloadProfile;

/// Configuration for the proxy server.
pub struct ProxyConfig {
    pub listen_addr: String,
    pub target_addr: String,
    pub output: PathBuf,
    pub pool_size: usize,
    pub pool_timeout_secs: u64,
    pub mask_values: bool,
    pub no_capture: bool,
    pub duration: Option<std::time::Duration>,
}

/// Run the proxy server (CLI mode — signal-based shutdown).
pub async fn run_proxy(config: ProxyConfig) -> Result<()> {
    let listener = TcpListener::bind(&config.listen_addr).await?;
    let pool = Arc::new(SessionPool::new(
        config.target_addr.clone(),
        config.pool_size,
        config.pool_timeout_secs,
    ));

    let (capture_tx, capture_rx) = mpsc::unbounded_channel();

    // Spawn capture collector
    let collector_handle = tokio::spawn(async move { run_collector(capture_rx).await });

    // Spawn listener (no metrics channel in CLI mode)
    let pool_clone = pool.clone();
    let no_capture = config.no_capture;
    let listener_handle = tokio::spawn(async move {
        listener::run_listener(listener, pool_clone, capture_tx, no_capture, None).await
    });

    // Wait for shutdown signal or duration
    match config.duration {
        Some(dur) => {
            info!("Proxy will run for {:?}", dur);
            tokio::time::sleep(dur).await;
            info!("Duration elapsed, shutting down...");
        }
        None => {
            info!("Press Ctrl+C to stop and save captured workload");
            #[cfg(unix)]
            {
                use tokio::signal::unix::{signal, SignalKind};
                let mut sigterm = signal(SignalKind::terminate())?;
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => {},
                    _ = sigterm.recv() => {},
                }
            }
            #[cfg(not(unix))]
            {
                tokio::signal::ctrl_c().await?;
            }
            info!("Shutdown signal received...");
        }
    }

    // Abort the listener to stop accepting new connections
    listener_handle.abort();

    // Give active connections a moment to finish
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Wait for collector to finish (channel senders will be dropped)
    let captured = collector_handle.await?;

    if config.no_capture {
        info!("Proxy stopped (capture was disabled)");
        return Ok(());
    }

    // Build and write profile
    let source_host = config.target_addr.clone();
    let profile = build_profile(captured, &source_host, config.mask_values);

    info!(
        "Captured {} queries across {} sessions",
        profile.metadata.total_queries, profile.metadata.total_sessions
    );

    io::write_profile(&config.output, &profile)?;
    info!("Wrote workload profile to {}", config.output.display());

    Ok(())
}

/// Run the proxy server in managed mode (web UI — CancellationToken + metrics channel).
///
/// Returns `Some(WorkloadProfile)` if capture was enabled, `None` otherwise.
pub async fn run_proxy_managed(
    config: ProxyConfig,
    cancel_token: CancellationToken,
    metrics_tx: mpsc::UnboundedSender<CaptureEvent>,
) -> Result<Option<WorkloadProfile>> {
    let listener = TcpListener::bind(&config.listen_addr).await?;
    let pool = Arc::new(SessionPool::new(
        config.target_addr.clone(),
        config.pool_size,
        config.pool_timeout_secs,
    ));

    let (capture_tx, capture_rx) = mpsc::unbounded_channel();

    // Spawn capture collector
    let collector_handle = tokio::spawn(async move { run_collector(capture_rx).await });

    // Spawn listener with metrics channel
    let pool_clone = pool.clone();
    let no_capture = config.no_capture;
    let listener_handle = tokio::spawn(async move {
        listener::run_listener(
            listener,
            pool_clone,
            capture_tx,
            no_capture,
            Some(metrics_tx),
        )
        .await
    });

    // Wait for cancellation or duration
    match config.duration {
        Some(dur) => {
            tokio::select! {
                _ = tokio::time::sleep(dur) => {
                    info!("Proxy duration elapsed, shutting down...");
                }
                _ = cancel_token.cancelled() => {
                    info!("Proxy cancelled via web UI");
                }
            }
        }
        None => {
            cancel_token.cancelled().await;
            info!("Proxy cancelled via web UI");
        }
    }

    // Abort the listener to stop accepting new connections
    listener_handle.abort();

    // Give active connections a moment to finish
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Wait for collector to finish (channel senders will be dropped)
    let captured = collector_handle.await?;

    if config.no_capture {
        info!("Proxy stopped (capture was disabled)");
        return Ok(None);
    }

    // Build profile
    let source_host = config.target_addr.clone();
    let profile = build_profile(captured, &source_host, config.mask_values);

    info!(
        "Captured {} queries across {} sessions",
        profile.metadata.total_queries, profile.metadata.total_sessions
    );

    // Write profile to disk
    io::write_profile(&config.output, &profile)?;
    info!("Wrote workload profile to {}", config.output.display());

    Ok(Some(profile))
}
