pub mod capture;
pub mod connection;
pub mod control;
pub mod listener;
pub mod pool;
pub mod protocol;
pub mod staging;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tokio::sync::RwLock;
use tokio_util::sync::CancellationToken;
use tracing::info;
use uuid::Uuid;

use self::capture::{
    build_profile, build_profile_from_staging, run_collector, run_staging_collector, CaptureEvent,
};
use self::control::{build_control_router, CaptureCommand, ControlState};
use self::pool::SessionPool;
use self::staging::StagingDb;
use crate::profile::io;
use crate::profile::WorkloadProfile;

/// Configuration for the proxy server.
pub struct ProxyConfig {
    pub listen_addr: String,
    pub target_addr: String,
    pub output: Option<PathBuf>,
    pub pool_size: usize,
    pub pool_timeout_secs: u64,
    pub mask_values: bool,
    pub no_capture: bool,
    pub duration: Option<std::time::Duration>,
    pub persistent: bool,
    pub control_port: Option<u16>,
}

/// Run the proxy server (CLI mode — signal-based shutdown).
pub async fn run_proxy(config: ProxyConfig) -> Result<()> {
    if config.persistent {
        return run_proxy_persistent(config).await;
    }

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
    let no_capture = Arc::new(AtomicBool::new(config.no_capture));
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

    if let Some(ref output) = config.output {
        io::write_profile(output, &profile)?;
        info!("Wrote workload profile to {}", output.display());
    } else {
        info!("No output path specified — skipping profile write");
    }

    Ok(())
}

/// Run the proxy in persistent mode (stays running, capture controlled via HTTP).
async fn run_proxy_persistent(config: ProxyConfig) -> Result<()> {
    let listener = TcpListener::bind(&config.listen_addr).await?;
    let pool = Arc::new(SessionPool::new(
        config.target_addr.clone(),
        config.pool_size,
        config.pool_timeout_secs,
    ));

    // Shared no_capture flag — starts as true (no capture until start-capture is called)
    let no_capture = Arc::new(AtomicBool::new(true));

    // Channel for capture commands from the control endpoint
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<CaptureCommand>();

    // Set up control state
    let control_port = config.control_port.unwrap_or(6544);
    let control_state = Arc::new(RwLock::new(ControlState {
        running: true,
        capturing: false,
        capture_id: None,
        active_sessions: 0,
        total_queries: 0,
        started_at: Instant::now(),
        capture_cmd_tx: Some(cmd_tx),
    }));

    // Start the control HTTP endpoint
    let control_router = build_control_router(control_state.clone());
    let control_addr = format!("0.0.0.0:{control_port}");
    let control_listener = TcpListener::bind(&control_addr).await?;
    info!("Persistent proxy control endpoint on http://{control_addr}");

    tokio::spawn(async move {
        if let Err(e) = axum::serve(control_listener, control_router).await {
            tracing::error!("Control endpoint error: {e}");
        }
    });

    // Open standalone staging DB
    let staging_db = StagingDb::open_standalone(&PathBuf::from("proxy-capture.db")).await?;

    // Spawn the listener — it runs for the entire lifetime of the proxy.
    // We need a capture_tx that stays alive. We'll swap the receiver on start/stop.
    let (persistent_capture_tx, persistent_capture_rx) = mpsc::unbounded_channel::<CaptureEvent>();

    let pool_clone = pool.clone();
    let no_capture_clone = no_capture.clone();
    let listener_handle = tokio::spawn(async move {
        listener::run_listener(
            listener,
            pool_clone,
            persistent_capture_tx,
            no_capture_clone,
            None,
        )
        .await
    });

    // We need to forward events from the persistent channel to the active staging collector.
    // Use an intermediary: a shared optional sender that the forwarder writes to.
    let active_staging_tx: Arc<tokio::sync::Mutex<Option<mpsc::UnboundedSender<CaptureEvent>>>> =
        Arc::new(tokio::sync::Mutex::new(None));

    // Spawn a forwarder task that reads from persistent_capture_rx and forwards
    // to the active staging collector (if any).
    let active_staging_tx_clone = active_staging_tx.clone();
    let forwarder_handle = tokio::spawn(async move {
        let mut rx = persistent_capture_rx;
        while let Some(event) = rx.recv().await {
            let guard = active_staging_tx_clone.lock().await;
            if let Some(ref tx) = *guard {
                let _ = tx.send(event);
            }
        }
    });

    // Track the active collector task
    let mut active_collector: Option<tokio::task::JoinHandle<()>> = None;
    let mut active_capture_id: Option<String> = None;

    info!(
        "Persistent proxy running on {} (target: {})",
        config.listen_addr, config.target_addr
    );
    info!("Use POST http://localhost:{control_port}/start-capture to begin capturing");

    // Main command loop — also listen for shutdown signals
    loop {
        let cmd = tokio::select! {
            cmd = cmd_rx.recv() => {
                match cmd {
                    Some(c) => c,
                    None => break, // Command channel closed
                }
            }
            _ = async {
                #[cfg(unix)]
                {
                    use tokio::signal::unix::{signal, SignalKind};
                    let mut sigterm = signal(SignalKind::terminate()).unwrap();
                    tokio::select! {
                        _ = tokio::signal::ctrl_c() => {},
                        _ = sigterm.recv() => {},
                    }
                }
                #[cfg(not(unix))]
                {
                    tokio::signal::ctrl_c().await.unwrap();
                }
            } => {
                info!("Shutdown signal received, stopping persistent proxy...");
                break;
            }
        };

        match cmd {
            CaptureCommand::Start { reply } => {
                if active_capture_id.is_some() {
                    let _ = reply.send(Err("Already capturing".to_string()));
                    continue;
                }

                let capture_id = Uuid::new_v4().to_string();
                info!("Starting capture session: {capture_id}");

                // Create a new staging collector channel
                let (staging_tx, staging_rx) = mpsc::unbounded_channel::<CaptureEvent>();

                // Set the forwarder to send to this new staging collector
                {
                    let mut guard = active_staging_tx.lock().await;
                    *guard = Some(staging_tx);
                }

                // Spawn staging collector
                let db_clone = staging_db.clone();
                let cap_id_clone = capture_id.clone();
                active_collector = Some(tokio::spawn(async move {
                    run_staging_collector(staging_rx, db_clone, cap_id_clone).await;
                }));

                // Enable capture
                no_capture.store(false, Ordering::Relaxed);

                // Update control state
                {
                    let mut state = control_state.write().await;
                    state.capturing = true;
                    state.capture_id = Some(capture_id.clone());
                }

                active_capture_id = Some(capture_id.clone());
                let _ = reply.send(Ok(capture_id));
            }
            CaptureCommand::Stop { output, reply } => {
                let capture_id = match active_capture_id.take() {
                    Some(id) => id,
                    None => {
                        let _ = reply.send(Err("Not currently capturing".to_string()));
                        continue;
                    }
                };

                info!("Stopping capture session: {capture_id}");

                // Disable capture (stop emitting events)
                no_capture.store(true, Ordering::Relaxed);

                // Close the staging collector channel by clearing the forwarder target
                {
                    let mut guard = active_staging_tx.lock().await;
                    *guard = None; // Drops the sender, closing the staging collector's channel
                }

                // Wait for the collector to finish processing
                if let Some(handle) = active_collector.take() {
                    let _ = handle.await;
                }

                // Read staged data and build profile
                let rows = match staging_db.read_capture(&capture_id).await {
                    Ok(r) => r,
                    Err(e) => {
                        let _ = reply.send(Err(format!("Failed to read staging data: {e}")));
                        continue;
                    }
                };

                let profile =
                    build_profile_from_staging(rows, &config.target_addr, config.mask_values);

                info!(
                    "Captured {} queries across {} sessions",
                    profile.metadata.total_queries, profile.metadata.total_sessions
                );

                // Determine output path
                let output_path = output.or_else(|| config.output.clone()).unwrap_or_else(|| {
                    let ts = chrono::Utc::now().format("%Y%m%d-%H%M%S");
                    PathBuf::from(format!("capture-{ts}.wkl"))
                });

                // Write profile
                if let Err(e) = io::write_profile(&output_path, &profile) {
                    let _ = reply.send(Err(format!("Failed to write profile: {e}")));
                    // Clean up staging data even on write failure
                    let _ = staging_db.clear_capture(&capture_id).await;
                    // Update control state
                    let mut state = control_state.write().await;
                    state.capturing = false;
                    state.capture_id = None;
                    continue;
                }

                info!("Wrote workload profile to {}", output_path.display());

                // Clear staging data
                let _ = staging_db.clear_capture(&capture_id).await;

                // Update control state
                {
                    let mut state = control_state.write().await;
                    state.capturing = false;
                    state.capture_id = None;
                }

                let _ = reply.send(Ok(serde_json::json!({
                    "ok": true,
                    "capture_id": capture_id,
                    "output": output_path.to_string_lossy(),
                    "total_sessions": profile.metadata.total_sessions,
                    "total_queries": profile.metadata.total_queries,
                })));
            }
        }
    }

    // Shutdown: if we were capturing, stop it
    if let Some(capture_id) = active_capture_id.take() {
        info!("Stopping active capture {capture_id} during shutdown...");
        no_capture.store(true, Ordering::Relaxed);
        {
            let mut guard = active_staging_tx.lock().await;
            *guard = None;
        }
        if let Some(handle) = active_collector.take() {
            let _ = handle.await;
        }
    }

    // Abort the listener
    listener_handle.abort();
    forwarder_handle.abort();

    // Give active connections a moment to finish
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    info!("Persistent proxy stopped");
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
    let no_capture = Arc::new(AtomicBool::new(config.no_capture));
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
    if let Some(ref output) = config.output {
        io::write_profile(output, &profile)?;
        info!("Wrote workload profile to {}", output.display());
    }

    Ok(Some(profile))
}
