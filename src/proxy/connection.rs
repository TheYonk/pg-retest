use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::io::{AsyncWriteExt, BufReader, BufWriter, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::capture::CaptureEvent;
use super::pool::SessionPool;
use super::protocol::{
    self, extract_bind, extract_error_message, extract_parse, extract_query_sql,
    format_bind_params, inline_bind_params, StartupType,
};

/// Handle a single client connection through its full lifecycle.
pub async fn handle_connection(
    client_stream: TcpStream,
    pool: Arc<SessionPool>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    no_capture: Arc<AtomicBool>,
    metrics_tx: Option<mpsc::UnboundedSender<CaptureEvent>>,
) {
    if let Err(e) = handle_connection_inner(
        client_stream,
        pool,
        session_id,
        capture_tx,
        no_capture,
        metrics_tx,
    )
    .await
    {
        debug!("Session {session_id} ended: {e}");
    }
}

async fn handle_connection_inner(
    mut client_stream: TcpStream,
    pool: Arc<SessionPool>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    no_capture: Arc<AtomicBool>,
    metrics_tx: Option<mpsc::UnboundedSender<CaptureEvent>>,
) -> Result<()> {
    // ── Phase 1: Startup ────────────────────────────────────────────
    // Read the first message from client (no type byte — startup message)
    let startup_msg = match protocol::read_startup_message(&mut client_stream).await? {
        Some(msg) => msg,
        None => return Ok(()), // Client disconnected immediately
    };

    // Handle SSLRequest
    let startup_msg = match protocol::classify_startup(&startup_msg) {
        StartupType::SslRequest => {
            // Reject SSL — respond with 'N'
            client_stream.write_all(b"N").await?;
            // Client should now send actual StartupMessage
            match protocol::read_startup_message(&mut client_stream).await? {
                Some(msg) => msg,
                None => return Ok(()),
            }
        }
        StartupType::CancelRequest => {
            debug!("Session {session_id}: cancel request (not yet supported)");
            return Ok(());
        }
        StartupType::StartupMessage => startup_msg,
        StartupType::Unknown => {
            warn!("Session {session_id}: unknown startup message");
            return Ok(());
        }
    };

    // Extract user/database from startup
    let (user, database) = protocol::parse_startup_params(&startup_msg);
    let user = user.unwrap_or_else(|| "unknown".to_string());
    let database = database.unwrap_or_else(|| user.clone());

    debug!("Session {session_id}: startup user={user} database={database}");

    // ── Phase 2: Get server connection from pool ────────────────────
    let server_conn = pool.checkout().await?;
    let mut server_stream = server_conn.stream;

    // Forward startup message to server
    protocol::write_message(&mut server_stream, &startup_msg).await?;

    // ── Phase 3: Auth passthrough ───────────────────────────────────
    let auth_complete = relay_auth(&mut client_stream, &mut server_stream).await?;
    if !auth_complete {
        pool.discard().await;
        return Ok(());
    }

    // Send capture session start
    if !no_capture.load(Ordering::Relaxed) {
        let event = CaptureEvent::SessionStart {
            session_id,
            user,
            database,
            timestamp: Instant::now(),
        };
        if let Some(ref mtx) = metrics_tx {
            let _ = mtx.send(event.clone());
        }
        let _ = capture_tx.send(event);
    }

    // ── Phase 4: Bidirectional relay with capture ───────────────────
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, server_write) = tokio::io::split(server_stream);
    // Buffer I/O to batch small protocol reads/writes
    let client_read = BufReader::new(client_read);
    let client_write = BufWriter::new(client_write);
    let server_read = BufReader::new(server_read);
    let server_write = BufWriter::new(server_write);

    let capture_tx2 = capture_tx.clone();
    let metrics_tx2 = metrics_tx.clone();

    // Shared state for prepared statement tracking
    let stmt_cache: Arc<tokio::sync::Mutex<HashMap<String, String>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));

    // Client → Server relay
    let c2s = tokio::spawn({
        let capture_tx = capture_tx.clone();
        let metrics_tx = metrics_tx.clone();
        let stmt_cache = stmt_cache.clone();
        let no_capture = no_capture.clone();
        async move {
            relay_client_to_server(
                client_read,
                server_write,
                session_id,
                capture_tx,
                stmt_cache,
                no_capture,
                metrics_tx,
            )
            .await
        }
    });

    // Server → Client relay
    let s2c = tokio::spawn(async move {
        relay_server_to_client(
            server_read,
            client_write,
            session_id,
            capture_tx2,
            no_capture,
            metrics_tx2,
        )
        .await
    });

    // Wait for either direction to finish (one side disconnected)
    tokio::select! {
        result = c2s => {
            if let Ok(Err(e)) = result {
                debug!("Session {session_id}: c2s error: {e}");
            }
        }
        result = s2c => {
            if let Ok(Err(e)) = result {
                debug!("Session {session_id}: s2c error: {e}");
            }
        }
    }

    // Session complete — discard the server connection
    // (connection may be in unknown state after one side disconnected)
    pool.discard().await;

    Ok(())
}

/// Relay auth messages between client and server until ReadyForQuery.
/// Returns true if auth succeeded, false if the connection was lost.
async fn relay_auth(client: &mut TcpStream, server: &mut TcpStream) -> Result<bool> {
    loop {
        // Read server response
        let msg = match protocol::read_message(server).await? {
            Some(m) => m,
            None => return Ok(false),
        };

        let is_ready = msg.msg_type == b'Z';
        let is_auth_request = msg.msg_type == b'R';

        // Forward to client
        protocol::write_message(client, &msg).await?;

        if is_ready {
            return Ok(true); // Auth complete, ready for queries
        }

        // If server sent an auth request, check if client needs to respond
        if is_auth_request {
            let body = msg.body();
            if body.len() >= 4 {
                let auth_type = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                match auth_type {
                    // AuthenticationOk — done with auth, server sends params next
                    0 => continue,
                    // AuthenticationSASLFinal — server verification, no client response
                    12 => continue,
                    // All others expect a client response:
                    // 3=Cleartext, 5=MD5, 7=GSSAPI, 8=SSPI,
                    // 10=SASL, 11=SASLContinue
                    _ => {
                        if let Some(client_msg) = protocol::read_message(client).await? {
                            protocol::write_message(server, &client_msg).await?;
                        } else {
                            return Ok(false);
                        }
                    }
                }
            }
        }
    }
}

/// Relay messages from client to server, extracting capture data.
async fn relay_client_to_server(
    mut client: BufReader<ReadHalf<TcpStream>>,
    mut server: BufWriter<WriteHalf<TcpStream>>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    stmt_cache: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    no_capture: Arc<AtomicBool>,
    metrics_tx: Option<mpsc::UnboundedSender<CaptureEvent>>,
) -> Result<()> {
    loop {
        let msg = match protocol::read_message(&mut client).await? {
            Some(m) => m,
            None => break, // Client disconnected
        };

        if !no_capture.load(Ordering::Relaxed) {
            match msg.msg_type {
                b'Q' => {
                    // Simple query
                    if let Some(sql) = extract_query_sql(&msg) {
                        let event = CaptureEvent::QueryStart {
                            session_id,
                            sql,
                            timestamp: Instant::now(),
                        };
                        if let Some(ref mtx) = metrics_tx {
                            let _ = mtx.send(event.clone());
                        }
                        let _ = capture_tx.send(event);
                    }
                }
                b'P' => {
                    // Parse (prepared statement) — cache name→SQL mapping
                    if let Some(parsed) = extract_parse(&msg) {
                        let mut cache = stmt_cache.lock().await;
                        cache.insert(parsed.statement_name, parsed.sql);
                    }
                }
                b'B' => {
                    // Bind — resolve stmt name to SQL, inline params
                    if let Some(bind) = extract_bind(&msg) {
                        let cache = stmt_cache.lock().await;
                        if let Some(sql_template) = cache.get(&bind.statement_name) {
                            let params = format_bind_params(&bind.parameters);
                            let sql = inline_bind_params(sql_template, &params);
                            let event = CaptureEvent::QueryStart {
                                session_id,
                                sql,
                                timestamp: Instant::now(),
                            };
                            if let Some(ref mtx) = metrics_tx {
                                let _ = mtx.send(event.clone());
                            }
                            let _ = capture_tx.send(event);
                        }
                    }
                }
                b'X' => {
                    // Terminate
                    protocol::write_message(&mut server, &msg).await?;
                    server.flush().await?;
                    let event = CaptureEvent::SessionEnd { session_id };
                    if let Some(ref mtx) = metrics_tx {
                        let _ = mtx.send(event.clone());
                    }
                    let _ = capture_tx.send(event);
                    break;
                }
                _ => {}
            }
        } else if msg.msg_type == b'X' {
            protocol::write_message(&mut server, &msg).await?;
            server.flush().await?;
            break;
        }

        protocol::write_message(&mut server, &msg).await?;
        server.flush().await?;
    }
    Ok(())
}

/// Relay messages from server to client, extracting capture data.
async fn relay_server_to_client(
    mut server: BufReader<ReadHalf<TcpStream>>,
    mut client: BufWriter<WriteHalf<TcpStream>>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    no_capture: Arc<AtomicBool>,
    metrics_tx: Option<mpsc::UnboundedSender<CaptureEvent>>,
) -> Result<()> {
    loop {
        let msg = match protocol::read_message(&mut server).await? {
            Some(m) => m,
            None => break, // Server disconnected
        };

        if !no_capture.load(Ordering::Relaxed) {
            match msg.msg_type {
                b'C' => {
                    // CommandComplete — query finished
                    let event = CaptureEvent::QueryComplete {
                        session_id,
                        timestamp: Instant::now(),
                    };
                    if let Some(ref mtx) = metrics_tx {
                        let _ = mtx.send(event.clone());
                    }
                    let _ = capture_tx.send(event);
                }
                b'E' => {
                    // ErrorResponse
                    if let Some(err_msg) = extract_error_message(&msg) {
                        let event = CaptureEvent::QueryError {
                            session_id,
                            message: err_msg,
                            timestamp: Instant::now(),
                        };
                        if let Some(ref mtx) = metrics_tx {
                            let _ = mtx.send(event.clone());
                        }
                        let _ = capture_tx.send(event);
                    }
                }
                _ => {}
            }
        }

        protocol::write_message(&mut client, &msg).await?;
        // Flush after ReadyForQuery so client sees results promptly
        if msg.msg_type == b'Z' {
            client.flush().await?;
        }
    }
    Ok(())
}
