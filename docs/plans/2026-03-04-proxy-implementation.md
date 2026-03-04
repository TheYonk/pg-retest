# Capture Proxy Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a PG wire protocol proxy that captures workload by sitting between client apps and PostgreSQL, with session pooling and `.wkl` output.

**Architecture:** TCP listener accepts client connections, pairs each with a pooled server connection, relays bytes bidirectionally while parsing PG message frames to extract queries/timing/errors. Capture events are sent via async channel to a collector that builds a `WorkloadProfile` on shutdown.

**Tech Stack:** Rust, Tokio (TCP/async), `bytes` crate (buffer management), existing pg-retest profile/masking modules.

---

### Task 1: Project Setup — Dependencies, Module Skeleton, Refactor `assign_transaction_ids`

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Modify: `src/profile/mod.rs` — add `pub fn assign_transaction_ids`
- Modify: `src/capture/csv_log.rs` — call profile-level function instead of local one
- Create: `src/proxy/mod.rs` — empty module with re-exports
- Create: `src/proxy/protocol.rs` — empty
- Create: `src/proxy/pool.rs` — empty
- Create: `src/proxy/capture.rs` — empty
- Create: `src/proxy/connection.rs` — empty
- Create: `src/proxy/listener.rs` — empty

**Step 1: Add `bytes` dependency to Cargo.toml**

```toml
# Add after anyhow = "1"
bytes = "1"
```

**Step 2: Move `assign_transaction_ids` to `src/profile/mod.rs`**

Add this public function at the end of `src/profile/mod.rs` (before any `#[cfg(test)]` block):

```rust
/// Assign transaction IDs to queries within BEGIN/COMMIT|ROLLBACK blocks.
/// Used by both CSV log capture and proxy capture.
pub fn assign_transaction_ids(queries: &mut [Query], next_txn_id: &mut u64) {
    let mut current_txn: Option<u64> = None;

    for query in queries.iter_mut() {
        match query.kind {
            QueryKind::Begin => {
                let txn_id = *next_txn_id;
                *next_txn_id += 1;
                current_txn = Some(txn_id);
                query.transaction_id = Some(txn_id);
            }
            QueryKind::Commit | QueryKind::Rollback => {
                if let Some(txn_id) = current_txn {
                    query.transaction_id = Some(txn_id);
                }
                current_txn = None;
            }
            _ => {
                query.transaction_id = current_txn;
            }
        }
    }
}
```

**Step 3: Update `src/capture/csv_log.rs` to call the profile-level function**

Replace the local `assign_transaction_ids` function body with a delegation:

```rust
fn assign_transaction_ids(queries: &mut [Query], next_txn_id: &mut u64) {
    crate::profile::assign_transaction_ids(queries, next_txn_id);
}
```

Or simply remove the local function and change the call site in `build_profile` to:
```rust
crate::profile::assign_transaction_ids(&mut queries, &mut next_txn_id);
```

**Step 4: Add `pub mod proxy;` to `src/lib.rs`**

```rust
pub mod capture;
pub mod classify;
pub mod cli;
pub mod compare;
pub mod profile;
pub mod proxy;
pub mod replay;
```

**Step 5: Create proxy module files**

Create `src/proxy/mod.rs`:
```rust
pub mod protocol;
pub mod pool;
pub mod capture;
pub mod connection;
pub mod listener;
```

Create empty files: `src/proxy/protocol.rs`, `src/proxy/pool.rs`, `src/proxy/capture.rs`, `src/proxy/connection.rs`, `src/proxy/listener.rs` — each with just a comment:
```rust
// TODO: implementation in next task
```

**Step 6: Run tests to verify nothing broke**

Run: `cargo test`
Expected: All 67 existing tests pass.

**Step 7: Commit**

```bash
git add -A
git commit -m "refactor: move assign_transaction_ids to profile module, add proxy skeleton"
```

---

### Task 2: Protocol — PG Message Frame Parsing

**Files:**
- Create: `src/proxy/protocol.rs`

The PG wire protocol has a consistent message frame format: `[1 byte type][4 byte length (includes self)][payload]`. Startup messages are special — no type byte, just `[4 byte length][payload]`.

**Step 1: Write failing tests for message frame reading**

In `src/proxy/protocol.rs`, write the full module with tests first:

```rust
use anyhow::{bail, Result};
use bytes::{Buf, BufMut, BytesMut};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// A parsed PG protocol message frame.
#[derive(Debug, Clone)]
pub struct PgMessage {
    /// Message type byte. 0 for startup messages (no type byte).
    pub msg_type: u8,
    /// Complete message bytes (including type byte and length).
    /// For startup messages, includes length + payload (no type byte).
    pub payload: BytesMut,
}

impl PgMessage {
    /// Total wire size of this message.
    pub fn wire_len(&self) -> usize {
        self.payload.len() + if self.msg_type != 0 { 1 } else { 0 }
    }

    /// Get the payload bytes (after type byte and length).
    pub fn body(&self) -> &[u8] {
        if self.msg_type != 0 {
            // Type byte is NOT in payload; payload = [4-byte len][body]
            &self.payload[4..]
        } else {
            // Startup: payload = [4-byte len][body]
            &self.payload[4..]
        }
    }
}

/// SSLRequest magic: length=8, code=80877103
const SSL_REQUEST_CODE: i32 = 80877103;
/// CancelRequest magic: length=16, code=80877102
const CANCEL_REQUEST_CODE: i32 = 80877102;
/// Protocol version 3.0: 196608
const PROTOCOL_VERSION_3: i32 = 196608;

/// Identifies what kind of startup-phase message this is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartupType {
    SslRequest,
    CancelRequest,
    StartupMessage,
    Unknown,
}

/// Read a single PG message from a stream (post-startup phase).
/// Returns None if the stream is closed (EOF).
pub async fn read_message<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Option<PgMessage>> {
    // Read type byte
    let msg_type = match read_byte(stream).await? {
        Some(b) => b,
        None => return Ok(None),
    };

    // Read 4-byte length
    let mut len_buf = [0u8; 4];
    stream.read_exact(&mut len_buf).await?;
    let len = i32::from_be_bytes(len_buf) as usize;

    if len < 4 {
        bail!("Invalid message length: {len}");
    }

    // Read remaining payload (length includes the 4 length bytes)
    let body_len = len - 4;
    let mut payload = BytesMut::with_capacity(len);
    payload.put_slice(&len_buf);
    if body_len > 0 {
        payload.resize(len, 0);
        stream.read_exact(&mut payload[4..]).await?;
    }

    Ok(Some(PgMessage { msg_type, payload }))
}

/// Read a startup-phase message (no type byte — just length + payload).
/// Used for the first message from a client (StartupMessage, SSLRequest, CancelRequest).
pub async fn read_startup_message<R: AsyncRead + Unpin>(
    stream: &mut R,
) -> Result<Option<PgMessage>> {
    // Read 4-byte length
    let mut len_buf = [0u8; 4];
    match stream.read_exact(&mut len_buf).await {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    let len = i32::from_be_bytes(len_buf) as usize;

    if len < 4 {
        bail!("Invalid startup message length: {len}");
    }

    let body_len = len - 4;
    let mut payload = BytesMut::with_capacity(len);
    payload.put_slice(&len_buf);
    if body_len > 0 {
        payload.resize(len, 0);
        stream.read_exact(&mut payload[4..]).await?;
    }

    Ok(Some(PgMessage {
        msg_type: 0,
        payload,
    }))
}

/// Write a PgMessage to a stream.
pub async fn write_message<W: AsyncWrite + Unpin>(
    stream: &mut W,
    msg: &PgMessage,
) -> Result<()> {
    if msg.msg_type != 0 {
        stream.write_all(&[msg.msg_type]).await?;
    }
    stream.write_all(&msg.payload).await?;
    Ok(())
}

/// Classify a startup-phase message by its protocol code.
pub fn classify_startup(msg: &PgMessage) -> StartupType {
    if msg.payload.len() < 8 {
        return StartupType::Unknown;
    }
    let code = i32::from_be_bytes([
        msg.payload[4],
        msg.payload[5],
        msg.payload[6],
        msg.payload[7],
    ]);
    match code {
        SSL_REQUEST_CODE => StartupType::SslRequest,
        CANCEL_REQUEST_CODE => StartupType::CancelRequest,
        PROTOCOL_VERSION_3 => StartupType::StartupMessage,
        _ => StartupType::Unknown,
    }
}

/// Extract user and database from a StartupMessage.
/// The body after the version field is a sequence of null-terminated key-value pairs.
pub fn parse_startup_params(msg: &PgMessage) -> (Option<String>, Option<String>) {
    let body = msg.body();
    if body.len() < 4 {
        return (None, None);
    }
    // Skip 4-byte version field
    let params = &body[4..];

    let mut user = None;
    let mut database = None;
    let mut iter = params.split(|&b| b == 0);

    loop {
        let key = match iter.next() {
            Some(k) if !k.is_empty() => k,
            _ => break,
        };
        let value = match iter.next() {
            Some(v) => v,
            None => break,
        };
        match key {
            b"user" => user = Some(String::from_utf8_lossy(value).into_owned()),
            b"database" => database = Some(String::from_utf8_lossy(value).into_owned()),
            _ => {}
        }
    }

    (user, database)
}

/// Read a single byte, returning None on EOF.
async fn read_byte<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Option<u8>> {
    let mut buf = [0u8; 1];
    match stream.read_exact(&mut buf).await {
        Ok(()) => Ok(Some(buf[0])),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(None),
        Err(e) => Err(e.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn make_message(msg_type: u8, body: &[u8]) -> Vec<u8> {
        let len = (body.len() + 4) as i32;
        let mut buf = Vec::new();
        buf.push(msg_type);
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(body);
        buf
    }

    #[tokio::test]
    async fn test_read_message_query() {
        let sql = b"SELECT 1\0";
        let wire = make_message(b'Q', sql);
        let mut cursor = Cursor::new(wire);
        let msg = read_message(&mut cursor).await.unwrap().unwrap();
        assert_eq!(msg.msg_type, b'Q');
        assert_eq!(msg.body(), sql);
    }

    #[tokio::test]
    async fn test_read_message_eof() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let msg = read_message(&mut cursor).await.unwrap();
        assert!(msg.is_none());
    }

    #[tokio::test]
    async fn test_read_startup_message_ssl_request() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&8i32.to_be_bytes()); // length = 8
        buf.extend_from_slice(&SSL_REQUEST_CODE.to_be_bytes());
        let mut cursor = Cursor::new(buf);
        let msg = read_startup_message(&mut cursor).await.unwrap().unwrap();
        assert_eq!(msg.msg_type, 0);
        assert_eq!(classify_startup(&msg), StartupType::SslRequest);
    }

    #[tokio::test]
    async fn test_read_startup_message_v3() {
        let mut buf = Vec::new();
        // length = 4 (len) + 4 (version) + key-value pairs + terminator
        let params = b"user\0app\0database\0mydb\0\0";
        let total_len = (4 + 4 + params.len()) as i32;
        buf.extend_from_slice(&total_len.to_be_bytes());
        buf.extend_from_slice(&PROTOCOL_VERSION_3.to_be_bytes());
        buf.extend_from_slice(params);
        let mut cursor = Cursor::new(buf);
        let msg = read_startup_message(&mut cursor).await.unwrap().unwrap();
        assert_eq!(classify_startup(&msg), StartupType::StartupMessage);
        let (user, db) = parse_startup_params(&msg);
        assert_eq!(user.as_deref(), Some("app"));
        assert_eq!(db.as_deref(), Some("mydb"));
    }

    #[tokio::test]
    async fn test_write_message_roundtrip() {
        let sql = b"SELECT 1\0";
        let wire = make_message(b'Q', sql);
        let mut cursor = Cursor::new(wire);
        let msg = read_message(&mut cursor).await.unwrap().unwrap();

        let mut output = Vec::new();
        write_message(&mut output, &msg).await.unwrap();

        // Re-read from output
        let mut cursor2 = Cursor::new(output);
        let msg2 = read_message(&mut cursor2).await.unwrap().unwrap();
        assert_eq!(msg2.msg_type, b'Q');
        assert_eq!(msg2.body(), sql);
    }

    #[tokio::test]
    async fn test_read_startup_eof() {
        let mut cursor = Cursor::new(Vec::<u8>::new());
        let msg = read_startup_message(&mut cursor).await.unwrap();
        assert!(msg.is_none());
    }
}
```

**Step 2: Run tests**

Run: `cargo test --lib proxy::protocol`
Expected: All 5 tests pass.

**Step 3: Commit**

```bash
git add src/proxy/protocol.rs
git commit -m "feat(proxy): PG wire protocol message frame parser"
```

---

### Task 3: Protocol — Content Extraction from Query/Parse/Bind/CommandComplete/Error/ReadyForQuery

**Files:**
- Modify: `src/proxy/protocol.rs` — add content extraction functions

**Step 1: Add extraction functions and tests**

Append to `src/proxy/protocol.rs` (before the `#[cfg(test)]` block):

```rust
// ── Content extraction from specific message types ──────────────────

/// Extract SQL text from a Query ('Q') message.
/// Body is: SQL string followed by null terminator.
pub fn extract_query_sql(msg: &PgMessage) -> Option<String> {
    if msg.msg_type != b'Q' {
        return None;
    }
    let body = msg.body();
    // Strip trailing null terminator
    let sql = if body.last() == Some(&0) {
        &body[..body.len() - 1]
    } else {
        body
    };
    Some(String::from_utf8_lossy(sql).into_owned())
}

/// Parsed Parse ('P') message: statement name + SQL text.
pub struct ParseMessage {
    pub statement_name: String,
    pub sql: String,
}

/// Extract statement name and SQL from a Parse ('P') message.
/// Body: name (null-terminated) + query (null-terminated) + param count (i16) + param OIDs.
pub fn extract_parse(msg: &PgMessage) -> Option<ParseMessage> {
    if msg.msg_type != b'P' {
        return None;
    }
    let body = msg.body();
    let name_end = body.iter().position(|&b| b == 0)?;
    let name = String::from_utf8_lossy(&body[..name_end]).into_owned();
    let rest = &body[name_end + 1..];
    let sql_end = rest.iter().position(|&b| b == 0)?;
    let sql = String::from_utf8_lossy(&rest[..sql_end]).into_owned();
    Some(ParseMessage {
        statement_name: name,
        sql,
    })
}

/// Parsed Bind ('B') message: portal name, statement name, parameter values.
pub struct BindMessage {
    pub portal_name: String,
    pub statement_name: String,
    pub parameters: Vec<Option<Vec<u8>>>,
}

/// Extract portal name, statement name, and parameters from a Bind ('B') message.
/// Body: portal (null-term) + stmt (null-term) + format_count (i16) + formats
///       + param_count (i16) + params (len + data each, -1 for NULL).
pub fn extract_bind(msg: &PgMessage) -> Option<BindMessage> {
    if msg.msg_type != b'B' {
        return None;
    }
    let body = msg.body();
    let mut pos = 0;

    // Portal name (null-terminated)
    let portal_end = body[pos..].iter().position(|&b| b == 0)?;
    let portal_name = String::from_utf8_lossy(&body[pos..pos + portal_end]).into_owned();
    pos += portal_end + 1;

    // Statement name (null-terminated)
    let stmt_end = body[pos..].iter().position(|&b| b == 0)?;
    let statement_name = String::from_utf8_lossy(&body[pos..pos + stmt_end]).into_owned();
    pos += stmt_end + 1;

    // Format codes count (i16) + skip format codes
    if pos + 2 > body.len() {
        return None;
    }
    let format_count = i16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2 + format_count * 2; // Each format code is 2 bytes

    // Parameter count (i16)
    if pos + 2 > body.len() {
        return None;
    }
    let param_count = i16::from_be_bytes([body[pos], body[pos + 1]]) as usize;
    pos += 2;

    let mut parameters = Vec::with_capacity(param_count);
    for _ in 0..param_count {
        if pos + 4 > body.len() {
            break;
        }
        let param_len = i32::from_be_bytes([body[pos], body[pos + 1], body[pos + 2], body[pos + 3]]);
        pos += 4;
        if param_len == -1 {
            parameters.push(None); // NULL
        } else {
            let len = param_len as usize;
            if pos + len > body.len() {
                break;
            }
            parameters.push(Some(body[pos..pos + len].to_vec()));
            pos += len;
        }
    }

    Some(BindMessage {
        portal_name,
        statement_name,
        parameters,
    })
}

/// Extract the command tag from a CommandComplete ('C') message.
/// Body: tag string (null-terminated), e.g. "SELECT 5", "INSERT 0 1".
pub fn extract_command_complete(msg: &PgMessage) -> Option<String> {
    if msg.msg_type != b'C' {
        return None;
    }
    let body = msg.body();
    let end = body.iter().position(|&b| b == 0).unwrap_or(body.len());
    Some(String::from_utf8_lossy(&body[..end]).into_owned())
}

/// Extract the transaction state from a ReadyForQuery ('Z') message.
/// Body: single byte — 'I' (idle), 'T' (in transaction), 'E' (failed transaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TxnState {
    Idle,
    InTransaction,
    Failed,
}

pub fn extract_ready_for_query(msg: &PgMessage) -> Option<TxnState> {
    if msg.msg_type != b'Z' {
        return None;
    }
    let body = msg.body();
    if body.is_empty() {
        return None;
    }
    match body[0] {
        b'I' => Some(TxnState::Idle),
        b'T' => Some(TxnState::InTransaction),
        b'E' => Some(TxnState::Failed),
        _ => None,
    }
}

/// Extract error message from an ErrorResponse ('E') message.
/// Body: sequence of (type byte + null-terminated string) pairs, terminated by 0.
/// We extract the 'M' (message) field.
pub fn extract_error_message(msg: &PgMessage) -> Option<String> {
    if msg.msg_type != b'E' {
        return None;
    }
    let body = msg.body();
    let mut pos = 0;
    while pos < body.len() {
        let field_type = body[pos];
        pos += 1;
        if field_type == 0 {
            break; // End of fields
        }
        let end = body[pos..].iter().position(|&b| b == 0)?;
        let value = &body[pos..pos + end];
        pos += end + 1;
        if field_type == b'M' {
            return Some(String::from_utf8_lossy(value).into_owned());
        }
    }
    None
}

/// Extract the PID and secret key from a BackendKeyData ('K') message.
pub fn extract_backend_key_data(msg: &PgMessage) -> Option<(i32, i32)> {
    if msg.msg_type != b'K' {
        return None;
    }
    let body = msg.body();
    if body.len() < 8 {
        return None;
    }
    let pid = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
    let secret = i32::from_be_bytes([body[4], body[5], body[6], body[7]]);
    Some((pid, secret))
}

/// Format parameter values from a Bind message as strings for capture.
/// Text parameters are converted to strings; binary params are shown as hex.
/// NULL parameters become the string "NULL".
pub fn format_bind_params(params: &[Option<Vec<u8>>]) -> Vec<String> {
    params
        .iter()
        .map(|p| match p {
            None => "NULL".to_string(),
            Some(bytes) => match std::str::from_utf8(bytes) {
                Ok(s) => format!("'{}'", s.replace('\'', "''")),
                Err(_) => format!("'\\x{}'", hex::encode(bytes)),
            },
        })
        .collect()
}

/// Inline bind parameters into a SQL template, replacing $1, $2, etc.
pub fn inline_bind_params(sql: &str, params: &[String]) -> String {
    let mut result = sql.to_string();
    // Replace in reverse order ($10 before $1)
    for (i, value) in params.iter().enumerate().rev() {
        let placeholder = format!("${}", i + 1);
        result = result.replace(&placeholder, value);
    }
    result
}
```

Note: `format_bind_params` uses `hex::encode` which would need the `hex` crate. Let's avoid that dependency and just use a simpler approach:

Replace the `format_bind_params` hex fallback:
```rust
                Err(_) => format!("'<binary {} bytes>'", bytes.len()),
```

**Step 2: Add tests for content extraction**

Add these tests to the existing `#[cfg(test)] mod tests` block:

```rust
    #[test]
    fn test_extract_query_sql() {
        let body = b"SELECT * FROM users\0";
        let msg = PgMessage {
            msg_type: b'Q',
            payload: {
                let len = (body.len() + 4) as i32;
                let mut p = BytesMut::new();
                p.put_slice(&len.to_be_bytes());
                p.put_slice(body);
                p
            },
        };
        assert_eq!(
            extract_query_sql(&msg).unwrap(),
            "SELECT * FROM users"
        );
    }

    #[test]
    fn test_extract_parse() {
        // Parse message body: name\0 + sql\0 + param_count(i16) + oids
        let mut body = Vec::new();
        body.extend_from_slice(b"stmt1\0");
        body.extend_from_slice(b"SELECT * FROM t WHERE id = $1\0");
        body.extend_from_slice(&0i16.to_be_bytes()); // 0 param types
        let msg = PgMessage {
            msg_type: b'P',
            payload: {
                let len = (body.len() + 4) as i32;
                let mut p = BytesMut::new();
                p.put_slice(&len.to_be_bytes());
                p.put_slice(&body);
                p
            },
        };
        let parsed = extract_parse(&msg).unwrap();
        assert_eq!(parsed.statement_name, "stmt1");
        assert_eq!(parsed.sql, "SELECT * FROM t WHERE id = $1");
    }

    #[test]
    fn test_extract_bind() {
        let mut body = Vec::new();
        body.extend_from_slice(b"\0");           // portal name (empty)
        body.extend_from_slice(b"stmt1\0");      // statement name
        body.extend_from_slice(&0i16.to_be_bytes()); // 0 format codes
        body.extend_from_slice(&2i16.to_be_bytes()); // 2 parameters
        // Param 1: "42" (len=2)
        body.extend_from_slice(&2i32.to_be_bytes());
        body.extend_from_slice(b"42");
        // Param 2: NULL (len=-1)
        body.extend_from_slice(&(-1i32).to_be_bytes());
        let msg = PgMessage {
            msg_type: b'B',
            payload: {
                let len = (body.len() + 4) as i32;
                let mut p = BytesMut::new();
                p.put_slice(&len.to_be_bytes());
                p.put_slice(&body);
                p
            },
        };
        let bind = extract_bind(&msg).unwrap();
        assert_eq!(bind.statement_name, "stmt1");
        assert_eq!(bind.parameters.len(), 2);
        assert_eq!(bind.parameters[0].as_deref(), Some(b"42".as_slice()));
        assert!(bind.parameters[1].is_none()); // NULL
    }

    #[test]
    fn test_extract_command_complete() {
        let body = b"SELECT 5\0";
        let msg = PgMessage {
            msg_type: b'C',
            payload: {
                let len = (body.len() + 4) as i32;
                let mut p = BytesMut::new();
                p.put_slice(&len.to_be_bytes());
                p.put_slice(body);
                p
            },
        };
        assert_eq!(
            extract_command_complete(&msg).unwrap(),
            "SELECT 5"
        );
    }

    #[test]
    fn test_extract_ready_for_query() {
        let msg = PgMessage {
            msg_type: b'Z',
            payload: {
                let mut p = BytesMut::new();
                p.put_slice(&5i32.to_be_bytes()); // length = 5 (4 + 1 byte body)
                p.put_u8(b'I');
                p
            },
        };
        assert_eq!(
            extract_ready_for_query(&msg).unwrap(),
            TxnState::Idle
        );
    }

    #[test]
    fn test_extract_error_message() {
        let mut body = Vec::new();
        body.push(b'S'); // Severity
        body.extend_from_slice(b"ERROR\0");
        body.push(b'M'); // Message
        body.extend_from_slice(b"relation \"foo\" does not exist\0");
        body.push(0); // terminator
        let msg = PgMessage {
            msg_type: b'E',
            payload: {
                let len = (body.len() + 4) as i32;
                let mut p = BytesMut::new();
                p.put_slice(&len.to_be_bytes());
                p.put_slice(&body);
                p
            },
        };
        assert_eq!(
            extract_error_message(&msg).unwrap(),
            "relation \"foo\" does not exist"
        );
    }

    #[test]
    fn test_inline_bind_params() {
        let sql = "SELECT * FROM users WHERE id = $1 AND name = $2";
        let params = vec!["42".to_string(), "'alice'".to_string()];
        let result = inline_bind_params(sql, &params);
        assert_eq!(result, "SELECT * FROM users WHERE id = 42 AND name = 'alice'");
    }

    #[test]
    fn test_format_bind_params() {
        let params = vec![
            Some(b"hello".to_vec()),
            None,
            Some(b"42".to_vec()),
        ];
        let formatted = format_bind_params(&params);
        assert_eq!(formatted[0], "'hello'");
        assert_eq!(formatted[1], "NULL");
        assert_eq!(formatted[2], "'42'");
    }
```

**Step 3: Run tests**

Run: `cargo test --lib proxy::protocol`
Expected: All 13 tests pass (5 from Task 2 + 8 new).

**Step 4: Commit**

```bash
git add src/proxy/protocol.rs
git commit -m "feat(proxy): PG message content extraction (Query/Parse/Bind/CommandComplete/Error/ReadyForQuery)"
```

---

### Task 4: Capture — CaptureEvent + CaptureCollector

**Files:**
- Create: `src/proxy/capture.rs`

The collector receives events from relay tasks via an async channel and builds a `WorkloadProfile` on shutdown.

**Step 1: Write the capture module**

```rust
use std::collections::HashMap;
use std::time::Instant;

use chrono::Utc;
use tokio::sync::mpsc;
use tracing::debug;

use crate::capture::masking::mask_sql_literals;
use crate::profile::{self, Metadata, Query, QueryKind, Session, WorkloadProfile};

/// Events sent from relay tasks to the capture collector.
#[derive(Debug)]
pub enum CaptureEvent {
    SessionStart {
        session_id: u64,
        user: String,
        database: String,
        timestamp: Instant,
    },
    QueryStart {
        session_id: u64,
        sql: String,
        timestamp: Instant,
    },
    QueryComplete {
        session_id: u64,
        timestamp: Instant,
    },
    QueryError {
        session_id: u64,
        message: String,
        timestamp: Instant,
    },
    SessionEnd {
        session_id: u64,
    },
}

/// Per-session state tracked by the collector.
struct SessionState {
    user: String,
    database: String,
    session_start: Instant,
    queries: Vec<CapturedQuery>,
    pending_sql: Option<(String, Instant)>,
}

struct CapturedQuery {
    sql: String,
    start_offset_us: u64,
    duration_us: u64,
    error: bool,
}

/// Runs the capture collector loop, consuming events until the channel closes.
/// Returns the captured sessions (to be built into a WorkloadProfile).
pub async fn run_collector(
    mut rx: mpsc::UnboundedReceiver<CaptureEvent>,
) -> Vec<(u64, SessionState)> {
    let mut sessions: HashMap<u64, SessionState> = HashMap::new();

    while let Some(event) = rx.recv().await {
        match event {
            CaptureEvent::SessionStart {
                session_id,
                user,
                database,
                timestamp,
            } => {
                sessions.insert(
                    session_id,
                    SessionState {
                        user,
                        database,
                        session_start: timestamp,
                        queries: Vec::new(),
                        pending_sql: None,
                    },
                );
                debug!("Capture: session {session_id} started");
            }
            CaptureEvent::QueryStart {
                session_id,
                sql,
                timestamp,
            } => {
                if let Some(state) = sessions.get_mut(&session_id) {
                    state.pending_sql = Some((sql, timestamp));
                }
            }
            CaptureEvent::QueryComplete {
                session_id,
                timestamp,
            } => {
                if let Some(state) = sessions.get_mut(&session_id) {
                    if let Some((sql, start)) = state.pending_sql.take() {
                        let offset = start.duration_since(state.session_start);
                        let duration = timestamp.duration_since(start);
                        state.queries.push(CapturedQuery {
                            sql,
                            start_offset_us: offset.as_micros() as u64,
                            duration_us: duration.as_micros() as u64,
                            error: false,
                        });
                    }
                }
            }
            CaptureEvent::QueryError {
                session_id,
                message,
                timestamp,
            } => {
                if let Some(state) = sessions.get_mut(&session_id) {
                    if let Some((sql, start)) = state.pending_sql.take() {
                        let offset = start.duration_since(state.session_start);
                        let duration = timestamp.duration_since(start);
                        debug!("Capture: query error in session {session_id}: {message}");
                        state.queries.push(CapturedQuery {
                            sql,
                            start_offset_us: offset.as_micros() as u64,
                            duration_us: duration.as_micros() as u64,
                            error: false, // Still record the query — it executed
                        });
                    }
                }
            }
            CaptureEvent::SessionEnd { session_id } => {
                debug!("Capture: session {session_id} ended");
            }
        }
    }

    sessions.into_iter().collect()
}

/// Build a WorkloadProfile from captured session data.
pub fn build_profile(
    captured: Vec<(u64, SessionState)>,
    source_host: &str,
    mask_values: bool,
) -> WorkloadProfile {
    let mut sessions = Vec::new();
    let mut total_queries: u64 = 0;
    let mut next_txn_id: u64 = 1;
    let mut capture_duration_us: u64 = 0;

    for (session_id, state) in captured {
        let mut queries: Vec<Query> = state
            .queries
            .into_iter()
            .map(|cq| {
                let sql = if mask_values {
                    mask_sql_literals(&cq.sql)
                } else {
                    cq.sql
                };
                Query {
                    kind: QueryKind::from_sql(&sql),
                    sql,
                    start_offset_us: cq.start_offset_us,
                    duration_us: cq.duration_us,
                    transaction_id: None,
                }
            })
            .collect();

        profile::assign_transaction_ids(&mut queries, &mut next_txn_id);

        if let Some(last) = queries.last() {
            let end = last.start_offset_us + last.duration_us;
            if end > capture_duration_us {
                capture_duration_us = end;
            }
        }

        total_queries += queries.len() as u64;

        sessions.push(Session {
            id: session_id,
            user: state.user,
            database: state.database,
            queries,
        });
    }

    sessions.sort_by_key(|s| s.id);

    WorkloadProfile {
        version: 2,
        captured_at: Utc::now(),
        source_host: source_host.to_string(),
        pg_version: "unknown".to_string(),
        capture_method: "proxy".to_string(),
        sessions: sessions.clone(),
        metadata: Metadata {
            total_queries,
            total_sessions: sessions.len() as u64,
            capture_duration_us,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_collector_basic_session() {
        let (tx, rx) = mpsc::unbounded_channel();
        let now = Instant::now();

        tx.send(CaptureEvent::SessionStart {
            session_id: 1,
            user: "app".into(),
            database: "mydb".into(),
            timestamp: now,
        })
        .unwrap();

        tx.send(CaptureEvent::QueryStart {
            session_id: 1,
            sql: "SELECT 1".into(),
            timestamp: now + std::time::Duration::from_micros(100),
        })
        .unwrap();

        tx.send(CaptureEvent::QueryComplete {
            session_id: 1,
            timestamp: now + std::time::Duration::from_micros(600),
        })
        .unwrap();

        tx.send(CaptureEvent::SessionEnd { session_id: 1 }).unwrap();

        drop(tx); // Close channel

        let captured = run_collector(rx).await;
        assert_eq!(captured.len(), 1);

        let (id, state) = &captured[0];
        assert_eq!(*id, 1);
        assert_eq!(state.user, "app");
        assert_eq!(state.queries.len(), 1);
        assert_eq!(state.queries[0].sql, "SELECT 1");
        assert!(state.queries[0].duration_us >= 400); // 600 - 100 = 500, allow some slack
    }

    #[test]
    fn test_build_profile_with_transactions() {
        let now = Instant::now();
        let state = SessionState {
            user: "app".into(),
            database: "db".into(),
            session_start: now,
            queries: vec![
                CapturedQuery {
                    sql: "BEGIN".into(),
                    start_offset_us: 0,
                    duration_us: 10,
                    error: false,
                },
                CapturedQuery {
                    sql: "INSERT INTO t VALUES (1)".into(),
                    start_offset_us: 100,
                    duration_us: 500,
                    error: false,
                },
                CapturedQuery {
                    sql: "COMMIT".into(),
                    start_offset_us: 700,
                    duration_us: 20,
                    error: false,
                },
            ],
            pending_sql: None,
        };

        let profile = build_profile(vec![(1, state)], "test-host", false);
        assert_eq!(profile.capture_method, "proxy");
        assert_eq!(profile.sessions.len(), 1);
        assert_eq!(profile.sessions[0].queries.len(), 3);
        assert_eq!(profile.sessions[0].queries[0].kind, QueryKind::Begin);
        assert_eq!(
            profile.sessions[0].queries[0].transaction_id,
            Some(1)
        );
        assert_eq!(
            profile.sessions[0].queries[1].transaction_id,
            Some(1)
        );
        assert_eq!(
            profile.sessions[0].queries[2].transaction_id,
            Some(1)
        );
    }

    #[test]
    fn test_build_profile_with_masking() {
        let now = Instant::now();
        let state = SessionState {
            user: "app".into(),
            database: "db".into(),
            session_start: now,
            queries: vec![CapturedQuery {
                sql: "SELECT * FROM users WHERE email = 'alice@corp.com'".into(),
                start_offset_us: 0,
                duration_us: 100,
                error: false,
            }],
            pending_sql: None,
        };

        let profile = build_profile(vec![(1, state)], "test", true);
        assert!(profile.sessions[0].queries[0].sql.contains("$S"));
        assert!(!profile.sessions[0].queries[0].sql.contains("alice"));
    }
}
```

**Step 2: Run tests**

Run: `cargo test --lib proxy::capture`
Expected: All 3 tests pass.

**Step 3: Commit**

```bash
git add src/proxy/capture.rs
git commit -m "feat(proxy): capture event collector and profile builder"
```

---

### Task 5: Pool — Session Connection Pool

**Files:**
- Create: `src/proxy/pool.rs`

**Step 1: Write the pool module**

```rust
use std::collections::VecDeque;

use anyhow::{bail, Result};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, Notify};
use tokio::time::{timeout, Duration};
use tracing::debug;

/// A pooled server connection.
pub struct ServerConn {
    pub stream: TcpStream,
    pub id: u64,
}

/// Session-mode connection pool.
/// Each client gets a dedicated server connection for the entire session.
pub struct SessionPool {
    target: String,
    max_size: usize,
    pool_timeout: Duration,
    reset_query: String,
    inner: Mutex<PoolInner>,
    notify: Notify,
}

struct PoolInner {
    idle: VecDeque<ServerConn>,
    active_count: usize,
    next_id: u64,
}

impl SessionPool {
    pub fn new(target: String, max_size: usize, pool_timeout_secs: u64, reset_query: String) -> Self {
        Self {
            target,
            max_size,
            pool_timeout: Duration::from_secs(pool_timeout_secs),
            reset_query,
            inner: Mutex::new(PoolInner {
                idle: VecDeque::new(),
                active_count: 0,
                next_id: 1,
            }),
            notify: Notify::new(),
        }
    }

    /// Checkout a server connection from the pool.
    /// Returns an idle connection or opens a new one if under the limit.
    /// Waits up to pool_timeout if at capacity.
    pub async fn checkout(&self) -> Result<ServerConn> {
        let deadline = tokio::time::Instant::now() + self.pool_timeout;

        loop {
            {
                let mut inner = self.inner.lock().await;

                // Try to grab an idle connection
                if let Some(conn) = inner.idle.pop_front() {
                    inner.active_count += 1;
                    debug!("Pool: checkout idle conn {} (active={}, idle={})",
                        conn.id, inner.active_count, inner.idle.len());
                    return Ok(conn);
                }

                // Try to open a new connection
                let total = inner.active_count + inner.idle.len();
                if total < self.max_size {
                    let id = inner.next_id;
                    inner.next_id += 1;
                    inner.active_count += 1;
                    debug!("Pool: opening new conn {id} to {} (active={}, idle={})",
                        self.target, inner.active_count, inner.idle.len());
                    drop(inner); // Release lock before connecting

                    let stream = TcpStream::connect(&self.target).await?;
                    return Ok(ServerConn { stream, id });
                }
            }

            // At capacity — wait for a connection to be returned
            let remaining = deadline - tokio::time::Instant::now();
            if remaining.is_zero() {
                bail!(
                    "Connection pool exhausted (max_size={}). Timed out waiting for a connection.",
                    self.max_size
                );
            }

            match timeout(remaining, self.notify.notified()).await {
                Ok(()) => continue, // A connection was returned, try again
                Err(_) => bail!(
                    "Connection pool exhausted (max_size={}). Timed out waiting for a connection.",
                    self.max_size
                ),
            }
        }
    }

    /// Return a server connection to the pool.
    pub async fn checkin(&self, conn: ServerConn) {
        let mut inner = self.inner.lock().await;
        inner.active_count = inner.active_count.saturating_sub(1);
        inner.idle.push_back(conn);
        debug!("Pool: checkin conn {} (active={}, idle={})",
            conn.id, inner.active_count, inner.idle.len());
        drop(inner);
        self.notify.notify_one();
    }

    /// Discard a server connection (don't return to pool).
    pub async fn discard(&self) {
        let mut inner = self.inner.lock().await;
        inner.active_count = inner.active_count.saturating_sub(1);
        drop(inner);
        self.notify.notify_one();
    }

    /// Get the reset query to run before returning a connection to the pool.
    pub fn reset_query(&self) -> &str {
        &self.reset_query
    }

    /// Get current pool stats.
    pub async fn stats(&self) -> (usize, usize) {
        let inner = self.inner.lock().await;
        (inner.active_count, inner.idle.len())
    }
}
```

Note: The `checkin` function has a subtle issue — it moves `conn` and then uses `conn.id` after the move. Fix by capturing the id first:

```rust
    pub async fn checkin(&self, conn: ServerConn) {
        let id = conn.id;
        let mut inner = self.inner.lock().await;
        inner.active_count = inner.active_count.saturating_sub(1);
        inner.idle.push_back(conn);
        debug!("Pool: checkin conn {id} (active={}, idle={})",
            inner.active_count, inner.idle.len());
        drop(inner);
        self.notify.notify_one();
    }
```

**Step 2: Run compile check**

Run: `cargo check`
Expected: Compiles without errors. (Pool requires real TCP for full testing — unit tests would need a mock, so we test it during integration.)

**Step 3: Commit**

```bash
git add src/proxy/pool.rs
git commit -m "feat(proxy): session connection pool with checkout/checkin/timeout"
```

---

### Task 6: Connection — Per-Connection Bidirectional Relay with Capture

**Files:**
- Create: `src/proxy/connection.rs`

This is the core logic: per-connection state machine that handles startup, then bidirectional relay with capture.

**Step 1: Write the connection module**

```rust
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::Result;
use tokio::io::{AsyncWriteExt, ReadHalf, WriteHalf};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::capture::CaptureEvent;
use super::pool::{ServerConn, SessionPool};
use super::protocol::{
    self, PgMessage, StartupType, TxnState,
    extract_backend_key_data, extract_bind, extract_command_complete,
    extract_error_message, extract_parse, extract_query_sql,
    extract_ready_for_query, format_bind_params, inline_bind_params,
};

/// Handle a single client connection through its full lifecycle.
pub async fn handle_connection(
    client_stream: TcpStream,
    pool: Arc<SessionPool>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    no_capture: bool,
) {
    if let Err(e) = handle_connection_inner(
        client_stream, pool, session_id, capture_tx, no_capture
    ).await {
        debug!("Session {session_id} ended: {e}");
    }
}

async fn handle_connection_inner(
    mut client_stream: TcpStream,
    pool: Arc<SessionPool>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    no_capture: bool,
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
            client_stream.write_all(&[b'N']).await?;
            // Client should now send actual StartupMessage
            match protocol::read_startup_message(&mut client_stream).await? {
                Some(msg) => msg,
                None => return Ok(()),
            }
        }
        StartupType::CancelRequest => {
            // TODO: route cancel request to correct backend via PID mapping
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
    let conn_id = server_conn.id;

    // Forward startup message to server
    protocol::write_message(&mut server_stream, &startup_msg).await?;

    // ── Phase 3: Auth passthrough ───────────────────────────────────
    // Relay messages between client and server until we see ReadyForQuery
    let auth_complete = relay_auth(&mut client_stream, &mut server_stream).await?;
    if !auth_complete {
        pool.discard().await;
        return Ok(());
    }

    // Send capture session start
    if !no_capture {
        let _ = capture_tx.send(CaptureEvent::SessionStart {
            session_id,
            user,
            database,
            timestamp: Instant::now(),
        });
    }

    // ── Phase 4: Bidirectional relay with capture ───────────────────
    let (client_read, client_write) = tokio::io::split(client_stream);
    let (server_read, server_write) = tokio::io::split(server_stream);

    let capture_tx2 = capture_tx.clone();
    let no_capture2 = no_capture;

    // Shared state for prepared statement tracking
    let stmt_cache: Arc<tokio::sync::Mutex<HashMap<String, String>>> =
        Arc::new(tokio::sync::Mutex::new(HashMap::new()));
    let stmt_cache2 = stmt_cache.clone();

    // Client → Server relay
    let c2s = tokio::spawn(async move {
        relay_client_to_server(
            client_read, server_write, session_id, capture_tx, stmt_cache, no_capture
        ).await
    });

    // Server → Client relay
    let s2c = tokio::spawn(async move {
        relay_server_to_client(
            server_read, client_write, session_id, capture_tx2, no_capture2
        ).await
    });

    // Wait for either direction to finish (one side disconnected)
    tokio::select! {
        result = c2s => {
            if let Err(e) = result {
                debug!("Session {session_id}: c2s task error: {e}");
            }
        }
        result = s2c => {
            if let Err(e) = result {
                debug!("Session {session_id}: s2c task error: {e}");
            }
        }
    }

    // Session complete — discard the server connection (session mode:
    // we'd need to send DISCARD ALL before reuse, but the connection
    // may be in an unknown state after one side disconnected)
    pool.discard().await;

    Ok(())
}

/// Relay auth messages between client and server until ReadyForQuery.
/// Returns true if auth succeeded, false if the connection was lost.
async fn relay_auth(
    client: &mut TcpStream,
    server: &mut TcpStream,
) -> Result<bool> {
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

        // If server sent an auth request, client needs to respond
        if is_auth_request {
            // Check if it's AuthenticationOk (body = 0i32)
            let body = msg.body();
            if body.len() >= 4 {
                let auth_type = i32::from_be_bytes([body[0], body[1], body[2], body[3]]);
                if auth_type == 0 {
                    // AuthenticationOk — server will send more messages, keep reading
                    continue;
                }
            }
            // Server wants auth data from client — relay client response
            if let Some(client_msg) = protocol::read_message(client).await? {
                protocol::write_message(server, &client_msg).await?;
            } else {
                return Ok(false);
            }
        }
    }
}

/// Relay messages from client to server, extracting capture data.
async fn relay_client_to_server(
    mut client: ReadHalf<TcpStream>,
    mut server: WriteHalf<TcpStream>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    stmt_cache: Arc<tokio::sync::Mutex<HashMap<String, String>>>,
    no_capture: bool,
) -> Result<()> {
    loop {
        let msg = match protocol::read_message(&mut client).await? {
            Some(m) => m,
            None => break, // Client disconnected
        };

        if !no_capture {
            match msg.msg_type {
                b'Q' => {
                    // Simple query
                    if let Some(sql) = extract_query_sql(&msg) {
                        let _ = capture_tx.send(CaptureEvent::QueryStart {
                            session_id,
                            sql,
                            timestamp: Instant::now(),
                        });
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
                            let _ = capture_tx.send(CaptureEvent::QueryStart {
                                session_id,
                                sql,
                                timestamp: Instant::now(),
                            });
                        }
                    }
                }
                b'X' => {
                    // Terminate
                    protocol::write_message(&mut server, &msg).await?;
                    let _ = capture_tx.send(CaptureEvent::SessionEnd { session_id });
                    break;
                }
                _ => {}
            }
        } else if msg.msg_type == b'X' {
            protocol::write_message(&mut server, &msg).await?;
            break;
        }

        protocol::write_message(&mut server, &msg).await?;
    }
    Ok(())
}

/// Relay messages from server to client, extracting capture data.
async fn relay_server_to_client(
    mut server: ReadHalf<TcpStream>,
    mut client: WriteHalf<TcpStream>,
    session_id: u64,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    no_capture: bool,
) -> Result<()> {
    loop {
        let msg = match protocol::read_message(&mut server).await? {
            Some(m) => m,
            None => break, // Server disconnected
        };

        if !no_capture {
            match msg.msg_type {
                b'C' => {
                    // CommandComplete — query finished
                    let _ = capture_tx.send(CaptureEvent::QueryComplete {
                        session_id,
                        timestamp: Instant::now(),
                    });
                }
                b'E' => {
                    // ErrorResponse
                    if let Some(err_msg) = extract_error_message(&msg) {
                        let _ = capture_tx.send(CaptureEvent::QueryError {
                            session_id,
                            message: err_msg,
                            timestamp: Instant::now(),
                        });
                    }
                }
                _ => {}
            }
        }

        protocol::write_message(&mut client, &msg).await?;
    }
    Ok(())
}
```

**Step 2: Compile check**

Run: `cargo check`
Expected: Compiles. (Full testing requires TCP connections — tested in integration task.)

**Step 3: Commit**

```bash
git add src/proxy/connection.rs
git commit -m "feat(proxy): per-connection relay with startup/auth passthrough and capture"
```

---

### Task 7: Listener — TCP Accept Loop + ProxyServer

**Files:**
- Modify: `src/proxy/mod.rs` — ProxyConfig, ProxyServer, run()
- Create: `src/proxy/listener.rs` — TCP accept loop

**Step 1: Write the listener**

`src/proxy/listener.rs`:
```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::Result;
use tokio::net::TcpListener;
use tokio::sync::mpsc;
use tracing::info;

use super::capture::CaptureEvent;
use super::connection::handle_connection;
use super::pool::SessionPool;

/// Run the TCP accept loop.
pub async fn run_listener(
    listener: TcpListener,
    pool: Arc<SessionPool>,
    capture_tx: mpsc::UnboundedSender<CaptureEvent>,
    no_capture: bool,
) -> Result<()> {
    let session_counter = AtomicU64::new(1);
    let addr = listener.local_addr()?;
    info!("Proxy listening on {addr}");

    loop {
        let (client_stream, peer_addr) = listener.accept().await?;
        let session_id = session_counter.fetch_add(1, Ordering::Relaxed);
        let pool = pool.clone();
        let capture_tx = capture_tx.clone();

        info!("Session {session_id}: accepted connection from {peer_addr}");

        tokio::spawn(async move {
            handle_connection(client_stream, pool, session_id, capture_tx, no_capture).await;
        });
    }
}
```

**Step 2: Write the ProxyServer in mod.rs**

`src/proxy/mod.rs`:
```rust
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
use tracing::info;

use self::capture::{build_profile, run_collector};
use self::pool::SessionPool;
use crate::profile::io;

/// Configuration for the proxy server.
pub struct ProxyConfig {
    pub listen_addr: String,
    pub target_addr: String,
    pub output: PathBuf,
    pub pool_size: usize,
    pub pool_timeout_secs: u64,
    pub reset_query: String,
    pub mask_values: bool,
    pub no_capture: bool,
    pub duration: Option<std::time::Duration>,
}

/// Run the proxy server.
pub async fn run_proxy(config: ProxyConfig) -> Result<()> {
    let listener = TcpListener::bind(&config.listen_addr).await?;
    let pool = Arc::new(SessionPool::new(
        config.target_addr.clone(),
        config.pool_size,
        config.pool_timeout_secs,
        config.reset_query.clone(),
    ));

    let (capture_tx, capture_rx) = mpsc::unbounded_channel();

    // Spawn capture collector
    let collector_handle = tokio::spawn(async move {
        run_collector(capture_rx).await
    });

    // Spawn listener
    let pool_clone = pool.clone();
    let no_capture = config.no_capture;
    let listener_handle = tokio::spawn(async move {
        listener::run_listener(listener, pool_clone, capture_tx, no_capture).await
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
            tokio::signal::ctrl_c().await?;
            info!("Shutdown signal received...");
        }
    }

    // Abort the listener to stop accepting new connections
    listener_handle.abort();

    // Give active connections a moment to finish
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    // Drop all remaining capture_tx senders by aborting — collector will finish
    // The collector_handle will complete when all senders are dropped

    // Wait for collector to finish
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
```

**Step 3: Compile check**

Run: `cargo check`
Expected: Compiles.

**Step 4: Commit**

```bash
git add src/proxy/mod.rs src/proxy/listener.rs
git commit -m "feat(proxy): TCP listener and ProxyServer with graceful shutdown"
```

---

### Task 8: CLI Integration — Add `proxy` Subcommand

**Files:**
- Modify: `src/cli.rs` — add `ProxyArgs` and `Proxy` variant
- Modify: `src/main.rs` — add `cmd_proxy` handler

**Step 1: Add ProxyArgs to cli.rs**

Add to the `Commands` enum:
```rust
    /// Run a capture proxy between clients and PostgreSQL
    Proxy(ProxyArgs),
```

Add the args struct:
```rust
#[derive(clap::Args)]
pub struct ProxyArgs {
    /// Address to listen on (e.g., 0.0.0.0:5433)
    #[arg(long, default_value = "0.0.0.0:5433")]
    pub listen: String,

    /// Target PostgreSQL address (e.g., localhost:5432)
    #[arg(long)]
    pub target: String,

    /// Output workload profile path (.wkl)
    #[arg(short, long, default_value = "workload.wkl")]
    pub output: PathBuf,

    /// Maximum server connections in the pool
    #[arg(long, default_value_t = 100)]
    pub pool_size: usize,

    /// Timeout waiting for a pool connection (seconds)
    #[arg(long, default_value_t = 30)]
    pub pool_timeout: u64,

    /// SQL to run when returning a connection to the pool
    #[arg(long, default_value = "DISCARD ALL")]
    pub reset_query: String,

    /// Mask string and numeric literals in captured SQL (PII protection)
    #[arg(long, default_value_t = false)]
    pub mask_values: bool,

    /// Disable workload capture (proxy-only mode)
    #[arg(long, default_value_t = false)]
    pub no_capture: bool,

    /// Capture duration (e.g., 60s, 5m). If not set, runs until Ctrl+C.
    #[arg(long)]
    pub duration: Option<String>,
}
```

**Step 2: Add cmd_proxy to main.rs**

Add to the match statement:
```rust
        Commands::Proxy(args) => cmd_proxy(args),
```

Add the handler function:
```rust
fn cmd_proxy(args: pg_retest::cli::ProxyArgs) -> Result<()> {
    use pg_retest::proxy::{run_proxy, ProxyConfig};

    let duration = args.duration.as_deref().map(parse_duration).transpose()?;

    let config = ProxyConfig {
        listen_addr: args.listen,
        target_addr: args.target,
        output: args.output,
        pool_size: args.pool_size,
        pool_timeout_secs: args.pool_timeout,
        reset_query: args.reset_query,
        mask_values: args.mask_values,
        no_capture: args.no_capture,
        duration,
    };

    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(run_proxy(config))
}

fn parse_duration(s: &str) -> Result<std::time::Duration> {
    let s = s.trim();
    if let Some(secs) = s.strip_suffix('s') {
        Ok(std::time::Duration::from_secs(secs.parse()?))
    } else if let Some(mins) = s.strip_suffix('m') {
        Ok(std::time::Duration::from_secs(mins.parse::<u64>()? * 60))
    } else {
        // Assume seconds
        Ok(std::time::Duration::from_secs(s.parse()?))
    }
}
```

**Step 3: Build and verify CLI help**

Run: `cargo build`
Run: `cargo run -- proxy --help`
Expected: Shows proxy subcommand help with all flags.

**Step 4: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat(proxy): add proxy subcommand to CLI"
```

---

### Task 9: End-to-End Test — Proxy Against Docker PostgreSQL

**Files:**
- None created — this is a manual smoke test

**Step 1: Run all unit tests**

Run: `cargo test`
Expected: All tests pass (67 existing + ~18 new proxy tests).

**Step 2: Run clippy**

Run: `cargo clippy`
Expected: Zero warnings.

**Step 3: Run fmt**

Run: `cargo fmt`

**Step 4: Start proxy against Docker container**

```bash
cargo run --release -- proxy \
  --listen 0.0.0.0:5433 \
  --target localhost:5441 \
  --output /tmp/proxy-workload.wkl \
  --duration 30s
```

**Step 5: In another terminal, run queries through the proxy**

```bash
# Direct connection through proxy
psql "host=localhost port=5433 dbname=postgres user=sales_demo_app password=salesdemo123" \
  -c "SELECT count(*) FROM pg_stat_activity"

# Or point the web app at port 5433 and generate traffic
for i in $(seq 1 10); do
  curl -s http://localhost:3000/api/products > /dev/null &
done
wait
```

**Step 6: Wait for proxy to save profile, then inspect**

```bash
cargo run --release -- inspect /tmp/proxy-workload.wkl
```

Expected: Shows captured queries with `capture_method: "proxy"`.

**Step 7: Replay read-only at 100x speed**

```bash
cargo run --release -- replay \
  --workload /tmp/proxy-workload.wkl \
  --target "host=localhost port=5441 dbname=postgres user=sales_demo_app password=salesdemo123" \
  --read-only --speed 100.0 \
  --output /tmp/proxy-replay.wkl
```

**Step 8: Compare**

```bash
cargo run --release -- compare \
  --source /tmp/proxy-workload.wkl \
  --replay /tmp/proxy-replay.wkl
```

**Step 9: Final commit**

```bash
cargo fmt
git add -A
git commit -m "feat(proxy): capture proxy with session pooling and PG wire protocol support"
```

---

## Build Dependency Graph

```
Task 1: Setup (deps, skeleton, refactor)
   ↓
Task 2: protocol.rs — message frames
   ↓
Task 3: protocol.rs — content extraction
   ↓
Task 4: capture.rs — events + collector    Task 5: pool.rs — session pool
   ↓                                          ↓
Task 6: connection.rs ─────────────────────────┘
   ↓
Task 7: listener.rs + mod.rs (ProxyServer)
   ↓
Task 8: CLI integration
   ↓
Task 9: End-to-end test
```

Tasks 4 and 5 are independent and can be done in parallel.
