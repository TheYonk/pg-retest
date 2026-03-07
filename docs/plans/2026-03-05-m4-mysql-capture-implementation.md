# M4: MySQL Capture + SQL Transform Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Capture workloads from MySQL slow query logs, transform SQL to PostgreSQL-compatible syntax, and produce standard `.wkl` profiles for replay against PostgreSQL.

**Architecture:** MySQL slow query log parser reads the multi-line log format into `LogEntry` structs, then reuses the existing profile-building pattern (session grouping, transaction IDs, metadata). A composable SQL transform pipeline converts MySQL-specific syntax (backtick quoting, `LIMIT offset,count`, `IFNULL`, `IF()`, etc.) to PG equivalents before storing in the profile. Untranslatable queries are marked as skipped. The `capture` CLI command gets a `--source-type` flag.

**Tech Stack:** `regex` crate for SQL transform rules (new dependency). No `sqlparser` — regex covers 80-90% of real MySQL queries and keeps complexity low.

---

## Context

### Codebase State
- Rust 2021 edition, Tokio async, clap derive CLI
- Existing subcommands: `capture`, `replay`, `compare`, `inspect`, `proxy`, `run`
- All public modules in `src/lib.rs`, binary dispatches from `src/main.rs`
- 107 tests, zero clippy warnings
- Dependencies: see `Cargo.toml` (no `regex` crate yet)

### Key Existing Code to Reuse
- `profile::assign_transaction_ids(queries, next_txn_id)` — transaction grouping
- `profile::QueryKind::from_sql(sql)` — query classification (works on PG SQL, will work on transformed SQL too)
- `capture::masking::mask_sql_literals(sql)` — PII masking (applied after transform)
- `profile::io::{read_profile, write_profile}` — MessagePack .wkl I/O

### MySQL Slow Query Log Format
Each entry in the slow log looks like:
```
# Time: 2024-03-08T10:00:00.123456Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.001234  Lock_time: 0.000100 Rows_sent: 1  Rows_examined: 100
SET timestamp=1709892000;
SELECT * FROM users WHERE id = 1;
```

Key parsing rules:
- `# Time:` line starts a new entry (optional — may be omitted if same second as previous)
- `# User@Host:` has username, optional host, and thread ID
- `# Query_time:` has timing in seconds (float)
- `SET timestamp=...;` line gives epoch timestamp (always present)
- SQL follows on remaining lines until the next `# ` comment or EOF
- Multi-line SQL is common (the SQL can span multiple lines)
- `use <database>;` lines should be captured for session metadata but not as queries

### Design Reference
- `docs/plans/2026-03-04-m4-mysql-capture-design.md` — Approved design

---

## Task 1: Add `regex` Dependency + Transform Module Skeleton

**Files:**
- Modify: `Cargo.toml`
- Modify: `src/lib.rs`
- Create: `src/transform/mod.rs`
- Create: `tests/transform_test.rs`

**What this does:** Add the `regex` crate, create the transform pipeline framework with `SqlTransformer` trait and `TransformPipeline`.

**Step 1: Add dependency and module**

Add to `Cargo.toml` after the `rmp-serde` line:
```toml
regex = "1"
```

Add to `src/lib.rs` (alphabetical, after `replay`):
```rust
pub mod transform;
```

**Step 2: Create `src/transform/mod.rs`**

```rust
pub mod mysql_to_pg;

/// Result of transforming a single SQL statement.
#[derive(Debug, Clone, PartialEq)]
pub enum TransformResult {
    /// SQL was transformed to PG-compatible syntax.
    Transformed(String),
    /// SQL could not be transformed and should be skipped.
    Skipped { reason: String },
    /// SQL is already PG-compatible (no changes needed).
    Unchanged,
}

/// A single SQL transformation rule.
pub trait SqlTransformer: Send + Sync {
    /// Apply this transformation to the given SQL.
    /// Returns the transformation result.
    fn transform(&self, sql: &str) -> TransformResult;

    /// Human-readable name for logging/reporting.
    fn name(&self) -> &str;
}

/// A composable pipeline of SQL transformers.
/// Transformers are applied in order. Each transformer receives the output
/// of the previous one. If any transformer returns `Skipped`, the pipeline
/// short-circuits and returns the skip.
pub struct TransformPipeline {
    transformers: Vec<Box<dyn SqlTransformer>>,
}

impl TransformPipeline {
    pub fn new(transformers: Vec<Box<dyn SqlTransformer>>) -> Self {
        Self { transformers }
    }

    /// Run all transformers in sequence on the input SQL.
    /// Returns the final SQL string and whether it was transformed.
    pub fn apply(&self, sql: &str) -> TransformResult {
        let mut current = sql.to_string();
        let mut was_transformed = false;

        for transformer in &self.transformers {
            match transformer.transform(&current) {
                TransformResult::Transformed(new_sql) => {
                    current = new_sql;
                    was_transformed = true;
                }
                TransformResult::Skipped { reason } => {
                    return TransformResult::Skipped { reason };
                }
                TransformResult::Unchanged => {}
            }
        }

        if was_transformed {
            TransformResult::Transformed(current)
        } else {
            TransformResult::Unchanged
        }
    }
}

/// Summary of transform results across a workload.
#[derive(Debug, Default)]
pub struct TransformReport {
    pub total_queries: usize,
    pub transformed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub skip_reasons: Vec<(String, String)>, // (sql_preview, reason)
}

impl TransformReport {
    pub fn record(&mut self, sql: &str, result: &TransformResult) {
        self.total_queries += 1;
        match result {
            TransformResult::Transformed(_) => self.transformed += 1,
            TransformResult::Unchanged => self.unchanged += 1,
            TransformResult::Skipped { reason } => {
                self.skipped += 1;
                let preview: String = sql.chars().take(80).collect();
                self.skip_reasons.push((preview, reason.clone()));
            }
        }
    }

    pub fn print_summary(&self) {
        println!();
        println!("  Transform Report");
        println!("  ================");
        println!("  Total queries:  {}", self.total_queries);
        println!("  Transformed:    {}", self.transformed);
        println!("  Unchanged:      {}", self.unchanged);
        println!("  Skipped:        {}", self.skipped);
        if !self.skip_reasons.is_empty() {
            println!();
            println!("  Skipped queries:");
            for (sql, reason) in self.skip_reasons.iter().take(10) {
                println!("    - {sql}");
                println!("      Reason: {reason}");
            }
            if self.skip_reasons.len() > 10 {
                println!("    ... and {} more", self.skip_reasons.len() - 10);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UppercaseTransformer;
    impl SqlTransformer for UppercaseTransformer {
        fn transform(&self, sql: &str) -> TransformResult {
            let upper = sql.to_uppercase();
            if upper == sql {
                TransformResult::Unchanged
            } else {
                TransformResult::Transformed(upper)
            }
        }
        fn name(&self) -> &str {
            "uppercase"
        }
    }

    struct SkipDdlTransformer;
    impl SqlTransformer for SkipDdlTransformer {
        fn transform(&self, sql: &str) -> TransformResult {
            if sql.to_uppercase().starts_with("CREATE") {
                TransformResult::Skipped {
                    reason: "DDL not supported".into(),
                }
            } else {
                TransformResult::Unchanged
            }
        }
        fn name(&self) -> &str {
            "skip_ddl"
        }
    }

    #[test]
    fn test_pipeline_transforms_in_order() {
        let pipeline = TransformPipeline::new(vec![Box::new(UppercaseTransformer)]);
        let result = pipeline.apply("select 1");
        assert_eq!(result, TransformResult::Transformed("SELECT 1".into()));
    }

    #[test]
    fn test_pipeline_unchanged_passthrough() {
        let pipeline = TransformPipeline::new(vec![Box::new(UppercaseTransformer)]);
        let result = pipeline.apply("SELECT 1");
        assert_eq!(result, TransformResult::Unchanged);
    }

    #[test]
    fn test_pipeline_skip_short_circuits() {
        let pipeline = TransformPipeline::new(vec![
            Box::new(SkipDdlTransformer),
            Box::new(UppercaseTransformer),
        ]);
        let result = pipeline.apply("CREATE TABLE foo (id int)");
        assert!(matches!(result, TransformResult::Skipped { .. }));
    }

    #[test]
    fn test_transform_report() {
        let mut report = TransformReport::default();
        report.record("SELECT 1", &TransformResult::Unchanged);
        report.record("select 1", &TransformResult::Transformed("SELECT 1".into()));
        report.record(
            "CREATE TABLE x",
            &TransformResult::Skipped {
                reason: "DDL".into(),
            },
        );
        assert_eq!(report.total_queries, 3);
        assert_eq!(report.transformed, 1);
        assert_eq!(report.unchanged, 1);
        assert_eq!(report.skipped, 1);
    }
}
```

**Step 3: Run tests**

```bash
cargo test --lib transform
```
Expected: 4 tests pass.

**Step 4: Commit**

```bash
git add Cargo.toml src/lib.rs src/transform/mod.rs
git commit -m "feat(transform): SQL transform pipeline framework with composable rules"
```

---

## Task 2: MySQL-to-PG Transform Rules

**Files:**
- Create: `src/transform/mysql_to_pg.rs`
- Create: `tests/mysql_transform_test.rs`

**What this does:** Implement the regex-based MySQL-to-PostgreSQL SQL transformers. Each transform is a separate struct implementing `SqlTransformer`. A convenience function builds the full MySQL-to-PG pipeline.

**Step 1: Create `src/transform/mysql_to_pg.rs`**

```rust
use regex::Regex;

use super::{SqlTransformer, TransformPipeline, TransformResult};

/// Build the standard MySQL-to-PostgreSQL transform pipeline.
pub fn mysql_to_pg_pipeline() -> TransformPipeline {
    TransformPipeline::new(vec![
        Box::new(SkipMysqlInternals),
        Box::new(BacktickToDoubleQuote),
        Box::new(LimitOffsetRewrite),
        Box::new(IfnullToCoalesce),
        Box::new(IfToCase),
        Box::new(UnixTimestampRewrite),
        Box::new(NowCompatible),
    ])
}

/// Skip MySQL-specific internal commands that have no PG equivalent.
struct SkipMysqlInternals;

impl SqlTransformer for SkipMysqlInternals {
    fn transform(&self, sql: &str) -> TransformResult {
        let upper = sql.trim().to_uppercase();
        if upper.starts_with("SHOW ")
            || upper.starts_with("SET NAMES ")
            || upper.starts_with("SET CHARACTER SET ")
            || upper.starts_with("SET AUTOCOMMIT")
            || upper.starts_with("SET SQL_MODE")
            || upper.starts_with("FLUSH ")
            || upper.starts_with("HANDLER ")
            || upper.starts_with("RESET ")
            || upper.starts_with("DESCRIBE ")
            || upper.starts_with("DESC ")
            || upper.starts_with("USE ")
        {
            TransformResult::Skipped {
                reason: format!("MySQL-specific command: {}", &sql.trim()[..sql.trim().len().min(40)]),
            }
        } else {
            TransformResult::Unchanged
        }
    }

    fn name(&self) -> &str {
        "skip_mysql_internals"
    }
}

/// Convert MySQL backtick-quoted identifiers to PG double-quoted identifiers.
/// `identifier` → "identifier"
struct BacktickToDoubleQuote;

impl SqlTransformer for BacktickToDoubleQuote {
    fn transform(&self, sql: &str) -> TransformResult {
        if !sql.contains('`') {
            return TransformResult::Unchanged;
        }
        let result = sql.replace('`', "\"");
        TransformResult::Transformed(result)
    }

    fn name(&self) -> &str {
        "backtick_to_double_quote"
    }
}

/// Rewrite MySQL `LIMIT offset, count` to PG `LIMIT count OFFSET offset`.
/// Does NOT rewrite standard `LIMIT count` (already PG-compatible).
struct LimitOffsetRewrite;

impl SqlTransformer for LimitOffsetRewrite {
    fn transform(&self, sql: &str) -> TransformResult {
        // Match: LIMIT <number>, <number>
        // MySQL: LIMIT offset, count → PG: LIMIT count OFFSET offset
        let re = Regex::new(r"(?i)\bLIMIT\s+(\d+)\s*,\s*(\d+)").unwrap();
        if let Some(caps) = re.captures(sql) {
            let offset = &caps[1];
            let count = &caps[2];
            let replacement = format!("LIMIT {count} OFFSET {offset}");
            let result = re.replace(sql, replacement.as_str()).to_string();
            return TransformResult::Transformed(result);
        }
        TransformResult::Unchanged
    }

    fn name(&self) -> &str {
        "limit_offset_rewrite"
    }
}

/// Replace `IFNULL(a, b)` with `COALESCE(a, b)`.
struct IfnullToCoalesce;

impl SqlTransformer for IfnullToCoalesce {
    fn transform(&self, sql: &str) -> TransformResult {
        let re = Regex::new(r"(?i)\bIFNULL\s*\(").unwrap();
        if re.is_match(sql) {
            let result = re.replace_all(sql, "COALESCE(").to_string();
            return TransformResult::Transformed(result);
        }
        TransformResult::Unchanged
    }

    fn name(&self) -> &str {
        "ifnull_to_coalesce"
    }
}

/// Replace `IF(cond, a, b)` with `CASE WHEN cond THEN a ELSE b END`.
/// Only handles simple cases where arguments don't contain nested parentheses.
struct IfToCase;

impl SqlTransformer for IfToCase {
    fn transform(&self, sql: &str) -> TransformResult {
        // Match IF( followed by content — we need to handle balanced parens
        let re = Regex::new(r"(?i)\bIF\s*\(").unwrap();
        if !re.is_match(sql) {
            return TransformResult::Unchanged;
        }

        let mut result = sql.to_string();
        let mut changed = false;

        // Find each IF( and attempt to extract 3 comma-separated arguments
        // We walk the string to handle nested parentheses
        loop {
            let re = Regex::new(r"(?i)\bIF\s*\(").unwrap();
            let m = match re.find(&result) {
                Some(m) => m,
                None => break,
            };

            let start = m.start();
            let paren_start = m.end() - 1; // position of '('

            // Extract balanced content inside IF(...)
            match extract_balanced_parens(&result, paren_start) {
                Some((inner, end)) => {
                    // Split inner by commas at depth 0
                    let parts = split_at_depth_zero(&inner);
                    if parts.len() == 3 {
                        let cond = parts[0].trim();
                        let then_val = parts[1].trim();
                        let else_val = parts[2].trim();
                        let replacement =
                            format!("CASE WHEN {cond} THEN {then_val} ELSE {else_val} END");
                        result = format!("{}{replacement}{}", &result[..start], &result[end + 1..]);
                        changed = true;
                    } else {
                        // Can't safely transform — not 3 args
                        break;
                    }
                }
                None => break, // Unbalanced parens
            }
        }

        if changed {
            TransformResult::Transformed(result)
        } else {
            TransformResult::Unchanged
        }
    }

    fn name(&self) -> &str {
        "if_to_case"
    }
}

/// Replace `UNIX_TIMESTAMP()` with `EXTRACT(EPOCH FROM NOW())::bigint`.
/// Replace `UNIX_TIMESTAMP(expr)` with `EXTRACT(EPOCH FROM expr)::bigint`.
struct UnixTimestampRewrite;

impl SqlTransformer for UnixTimestampRewrite {
    fn transform(&self, sql: &str) -> TransformResult {
        let re_no_arg = Regex::new(r"(?i)\bUNIX_TIMESTAMP\s*\(\s*\)").unwrap();
        let re_with_arg = Regex::new(r"(?i)\bUNIX_TIMESTAMP\s*\(").unwrap();

        let mut result = sql.to_string();
        let mut changed = false;

        // First handle no-arg version
        if re_no_arg.is_match(&result) {
            result = re_no_arg
                .replace_all(&result, "EXTRACT(EPOCH FROM NOW())::bigint")
                .to_string();
            changed = true;
        }

        // Then handle with-arg version
        if re_with_arg.is_match(&result) {
            // Find each UNIX_TIMESTAMP( and extract the argument
            loop {
                let m = match re_with_arg.find(&result) {
                    Some(m) => m,
                    None => break,
                };
                let start = m.start();
                let paren_start = m.end() - 1;

                match extract_balanced_parens(&result, paren_start) {
                    Some((inner, end)) => {
                        let replacement =
                            format!("EXTRACT(EPOCH FROM {})::bigint", inner.trim());
                        result = format!("{}{replacement}{}", &result[..start], &result[end + 1..]);
                        changed = true;
                    }
                    None => break,
                }
            }
        }

        if changed {
            TransformResult::Transformed(result)
        } else {
            TransformResult::Unchanged
        }
    }

    fn name(&self) -> &str {
        "unix_timestamp_rewrite"
    }
}

/// NOW() is compatible between MySQL and PG — no transform needed.
/// This is a no-op marker for documentation/completeness.
struct NowCompatible;

impl SqlTransformer for NowCompatible {
    fn transform(&self, _sql: &str) -> TransformResult {
        TransformResult::Unchanged
    }

    fn name(&self) -> &str {
        "now_compatible"
    }
}

/// Extract the content inside balanced parentheses starting at position `start`.
/// `start` must point to the opening '('.
/// Returns (inner_content, closing_paren_position) or None if unbalanced.
fn extract_balanced_parens(s: &str, start: usize) -> Option<(String, usize)> {
    let bytes = s.as_bytes();
    if bytes.get(start) != Some(&b'(') {
        return None;
    }
    let mut depth = 1;
    let mut i = start + 1;
    let mut in_single_quote = false;
    let mut in_double_quote = false;

    while i < bytes.len() && depth > 0 {
        let ch = bytes[i];
        if in_single_quote {
            if ch == b'\'' {
                // Check for escaped quote
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    i += 2;
                    continue;
                }
                in_single_quote = false;
            }
        } else if in_double_quote {
            if ch == b'"' {
                in_double_quote = false;
            }
        } else {
            match ch {
                b'\'' => in_single_quote = true,
                b'"' => in_double_quote = true,
                b'(' => depth += 1,
                b')' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some((s[start + 1..i].to_string(), i));
                    }
                }
                _ => {}
            }
        }
        i += 1;
    }
    None
}

/// Split a string by commas at parenthesis depth 0.
/// Respects quoted strings and nested parentheses.
fn split_at_depth_zero(s: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    let mut depth = 0;
    let mut in_single_quote = false;
    let mut in_double_quote = false;
    let bytes = s.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        let ch = bytes[i];
        if in_single_quote {
            current.push(ch as char);
            if ch == b'\'' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'\'' {
                    current.push('\'' );
                    i += 2;
                    continue;
                }
                in_single_quote = false;
            }
        } else if in_double_quote {
            current.push(ch as char);
            if ch == b'"' {
                in_double_quote = false;
            }
        } else {
            match ch {
                b'\'' => {
                    in_single_quote = true;
                    current.push(ch as char);
                }
                b'"' => {
                    in_double_quote = true;
                    current.push(ch as char);
                }
                b'(' => {
                    depth += 1;
                    current.push(ch as char);
                }
                b')' => {
                    depth -= 1;
                    current.push(ch as char);
                }
                b',' if depth == 0 => {
                    parts.push(current.clone());
                    current.clear();
                    i += 1;
                    continue;
                }
                _ => {
                    current.push(ch as char);
                }
            }
        }
        i += 1;
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_backtick_to_double_quote() {
        let t = BacktickToDoubleQuote;
        assert_eq!(
            t.transform("SELECT `id`, `name` FROM `users`"),
            TransformResult::Transformed("SELECT \"id\", \"name\" FROM \"users\"".into())
        );
        assert_eq!(
            t.transform("SELECT id FROM users"),
            TransformResult::Unchanged
        );
    }

    #[test]
    fn test_limit_offset_rewrite() {
        let t = LimitOffsetRewrite;
        // MySQL: LIMIT offset, count → PG: LIMIT count OFFSET offset
        assert_eq!(
            t.transform("SELECT * FROM t LIMIT 10, 20"),
            TransformResult::Transformed("SELECT * FROM t LIMIT 20 OFFSET 10".into())
        );
        // Standard LIMIT (no rewrite needed)
        assert_eq!(
            t.transform("SELECT * FROM t LIMIT 10"),
            TransformResult::Unchanged
        );
    }

    #[test]
    fn test_ifnull_to_coalesce() {
        let t = IfnullToCoalesce;
        assert_eq!(
            t.transform("SELECT IFNULL(name, 'unknown') FROM users"),
            TransformResult::Transformed("SELECT COALESCE(name, 'unknown') FROM users".into())
        );
        assert_eq!(
            t.transform("SELECT COALESCE(a, b) FROM t"),
            TransformResult::Unchanged
        );
    }

    #[test]
    fn test_if_to_case() {
        let t = IfToCase;
        assert_eq!(
            t.transform("SELECT IF(status = 1, 'active', 'inactive') FROM users"),
            TransformResult::Transformed(
                "SELECT CASE WHEN status = 1 THEN 'active' ELSE 'inactive' END FROM users".into()
            )
        );
    }

    #[test]
    fn test_unix_timestamp_no_arg() {
        let t = UnixTimestampRewrite;
        assert_eq!(
            t.transform("SELECT UNIX_TIMESTAMP()"),
            TransformResult::Transformed("SELECT EXTRACT(EPOCH FROM NOW())::bigint".into())
        );
    }

    #[test]
    fn test_skip_mysql_internals() {
        let t = SkipMysqlInternals;
        assert!(matches!(
            t.transform("SHOW VARIABLES LIKE 'version'"),
            TransformResult::Skipped { .. }
        ));
        assert!(matches!(
            t.transform("SET NAMES utf8mb4"),
            TransformResult::Skipped { .. }
        ));
        assert!(matches!(
            t.transform("USE mydb"),
            TransformResult::Skipped { .. }
        ));
        assert_eq!(
            t.transform("SELECT 1"),
            TransformResult::Unchanged
        );
    }

    #[test]
    fn test_extract_balanced_parens() {
        assert_eq!(
            extract_balanced_parens("foo(bar, baz)", 3),
            Some(("bar, baz".to_string(), 12))
        );
        assert_eq!(
            extract_balanced_parens("fn(a, (b, c), d)", 2),
            Some(("a, (b, c), d".to_string(), 15))
        );
        assert_eq!(extract_balanced_parens("no_paren", 0), None);
    }

    #[test]
    fn test_split_at_depth_zero() {
        let parts = split_at_depth_zero("a, b, c");
        assert_eq!(parts, vec!["a", " b", " c"]);

        let parts = split_at_depth_zero("a, fn(b, c), d");
        assert_eq!(parts, vec!["a", " fn(b, c)", " d"]);

        let parts = split_at_depth_zero("'hello, world', 42");
        assert_eq!(parts, vec!["'hello, world'", " 42"]);
    }
}
```

**Step 2: Create `tests/mysql_transform_test.rs`**

```rust
use pg_retest::transform::mysql_to_pg::mysql_to_pg_pipeline;
use pg_retest::transform::TransformResult;

#[test]
fn test_full_pipeline_simple_select() {
    let pipeline = mysql_to_pg_pipeline();
    let result = pipeline.apply("SELECT * FROM users WHERE id = 1");
    // Pure SQL, no MySQL-specific syntax — unchanged
    assert_eq!(result, TransformResult::Unchanged);
}

#[test]
fn test_full_pipeline_backtick_and_limit() {
    let pipeline = mysql_to_pg_pipeline();
    let result = pipeline.apply("SELECT `name`, `email` FROM `users` LIMIT 10, 20");
    match result {
        TransformResult::Transformed(sql) => {
            assert!(sql.contains('"'));
            assert!(!sql.contains('`'));
            assert!(sql.contains("LIMIT 20 OFFSET 10"));
        }
        _ => panic!("Expected Transformed, got {result:?}"),
    }
}

#[test]
fn test_full_pipeline_ifnull_and_if() {
    let pipeline = mysql_to_pg_pipeline();
    let result = pipeline.apply(
        "SELECT IFNULL(name, 'anon'), IF(active = 1, 'yes', 'no') FROM users",
    );
    match result {
        TransformResult::Transformed(sql) => {
            assert!(sql.contains("COALESCE(name, 'anon')"));
            assert!(sql.contains("CASE WHEN active = 1 THEN 'yes' ELSE 'no' END"));
        }
        _ => panic!("Expected Transformed, got {result:?}"),
    }
}

#[test]
fn test_full_pipeline_skips_show() {
    let pipeline = mysql_to_pg_pipeline();
    let result = pipeline.apply("SHOW VARIABLES LIKE 'version'");
    assert!(matches!(result, TransformResult::Skipped { .. }));
}

#[test]
fn test_full_pipeline_skips_use() {
    let pipeline = mysql_to_pg_pipeline();
    let result = pipeline.apply("USE production_db");
    assert!(matches!(result, TransformResult::Skipped { .. }));
}

#[test]
fn test_full_pipeline_dml_compatible() {
    let pipeline = mysql_to_pg_pipeline();
    // Standard DML is compatible between MySQL and PG
    let result = pipeline.apply("INSERT INTO orders (user_id, total) VALUES (1, 99.99)");
    assert_eq!(result, TransformResult::Unchanged);
}

#[test]
fn test_full_pipeline_transaction_control() {
    let pipeline = mysql_to_pg_pipeline();
    assert_eq!(pipeline.apply("BEGIN"), TransformResult::Unchanged);
    assert_eq!(pipeline.apply("COMMIT"), TransformResult::Unchanged);
    assert_eq!(pipeline.apply("ROLLBACK"), TransformResult::Unchanged);
}
```

**Step 3: Run tests**

```bash
cargo test --lib transform
cargo test --test mysql_transform_test
```

**Step 4: Commit**

```bash
git add src/transform/mysql_to_pg.rs tests/mysql_transform_test.rs
git commit -m "feat(transform): MySQL-to-PostgreSQL SQL transform rules (regex-based)"
```

---

## Task 3: MySQL Slow Query Log Parser

**Files:**
- Create: `src/capture/mysql_slow.rs`
- Modify: `src/capture/mod.rs` (add `pub mod mysql_slow;`)
- Create: `tests/fixtures/sample_mysql_slow.log`
- Create: `tests/mysql_slow_test.rs`

**What this does:** Parse MySQL slow query log format into `WorkloadProfile`. This is the primary MySQL capture path since it includes timing data.

**Step 1: Create `tests/fixtures/sample_mysql_slow.log`**

```
/usr/sbin/mysqld, Version: 8.0.36 (MySQL Community Server - GPL). started with:
Tcp port: 3306  Unix socket: /var/run/mysqld/mysqld.sock
Time                 Id Command    Argument
# Time: 2024-03-08T10:00:00.100000Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.001234  Lock_time: 0.000100 Rows_sent: 1  Rows_examined: 100
SET timestamp=1709892000;
SELECT * FROM users WHERE id = 1;
# Time: 2024-03-08T10:00:00.200000Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.002500  Lock_time: 0.000050 Rows_sent: 10  Rows_examined: 50
SET timestamp=1709892000;
SELECT `name`, `email` FROM `users` WHERE `active` = 1 LIMIT 10, 20;
# Time: 2024-03-08T10:00:00.500000Z
# User@Host: admin[admin] @ 192.168.1.10 []  Id:    99
# Query_time: 0.010000  Lock_time: 0.000000 Rows_sent: 0  Rows_examined: 0
SET timestamp=1709892000;
SHOW VARIABLES LIKE 'version';
# Time: 2024-03-08T10:00:01.000000Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.000500  Lock_time: 0.000010 Rows_sent: 0  Rows_examined: 1
SET timestamp=1709892001;
INSERT INTO orders (user_id, total) VALUES (1, 99.99);
# Time: 2024-03-08T10:00:01.500000Z
# User@Host: batch[batch] @ localhost []  Id:    55
# Query_time: 0.050000  Lock_time: 0.001000 Rows_sent: 5000  Rows_examined: 100000
SET timestamp=1709892001;
SELECT o.id, o.total, u.name
FROM orders o
JOIN users u ON o.user_id = u.id
WHERE o.created_at > '2024-01-01'
ORDER BY o.total DESC
LIMIT 5000;
# Time: 2024-03-08T10:00:02.000000Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.000100  Lock_time: 0.000000 Rows_sent: 0  Rows_examined: 0
SET timestamp=1709892002;
BEGIN;
# Time: 2024-03-08T10:00:02.100000Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.001000  Lock_time: 0.000100 Rows_sent: 0  Rows_examined: 1
SET timestamp=1709892002;
UPDATE users SET last_login = NOW() WHERE id = 1;
# Time: 2024-03-08T10:00:02.200000Z
# User@Host: app_user[app_user] @ localhost []  Id:    42
# Query_time: 0.000050  Lock_time: 0.000000 Rows_sent: 0  Rows_examined: 0
SET timestamp=1709892002;
COMMIT;
```

**Step 2: Create `src/capture/mysql_slow.rs`**

```rust
use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::path::Path;

use anyhow::{Context, Result};
use chrono::{DateTime, NaiveDateTime, Utc};
use tracing::{debug, warn};

use crate::profile::{assign_transaction_ids, Metadata, Query, QueryKind, Session, WorkloadProfile};
use crate::transform::mysql_to_pg::mysql_to_pg_pipeline;
use crate::transform::{TransformPipeline, TransformReport, TransformResult};

pub struct MysqlSlowLogCapture;

/// A raw parsed slow log entry.
struct SlowLogEntry {
    timestamp: DateTime<Utc>,
    user: String,
    thread_id: u64,
    query_time_us: u64,
    sql: String,
}

impl MysqlSlowLogCapture {
    pub fn capture_from_file(
        &self,
        path: &Path,
        source_host: &str,
        transform: bool,
    ) -> Result<WorkloadProfile> {
        let entries = self.parse_slow_log(path)?;
        let pipeline = if transform {
            Some(mysql_to_pg_pipeline())
        } else {
            None
        };
        self.build_profile(entries, source_host, pipeline.as_ref())
    }

    fn parse_slow_log(&self, path: &Path) -> Result<Vec<SlowLogEntry>> {
        let file = std::fs::File::open(path)
            .with_context(|| format!("Failed to open MySQL slow log: {}", path.display()))?;
        let reader = BufReader::new(file);

        let mut entries = Vec::new();
        let mut current_time: Option<DateTime<Utc>> = None;
        let mut current_user = String::new();
        let mut current_thread_id: u64 = 0;
        let mut current_query_time_us: u64 = 0;
        let mut current_sql_lines: Vec<String> = Vec::new();
        let mut in_query = false;

        for line in reader.lines() {
            let line = line.context("Failed to read line")?;
            let trimmed = line.trim();

            // Skip header lines
            if trimmed.is_empty()
                || trimmed.starts_with("/usr/")
                || trimmed.starts_with("Tcp port:")
                || trimmed.starts_with("Time ")
            {
                // If we were accumulating a query, flush it
                if in_query && !current_sql_lines.is_empty() {
                    let sql = current_sql_lines.join("\n").trim().to_string();
                    if !sql.is_empty() {
                        entries.push(SlowLogEntry {
                            timestamp: current_time.unwrap_or_else(Utc::now),
                            user: current_user.clone(),
                            thread_id: current_thread_id,
                            query_time_us: current_query_time_us,
                            sql,
                        });
                    }
                    current_sql_lines.clear();
                    in_query = false;
                }
                continue;
            }

            // # Time: 2024-03-08T10:00:00.100000Z
            if let Some(time_str) = trimmed.strip_prefix("# Time: ") {
                // Flush previous query if any
                if in_query && !current_sql_lines.is_empty() {
                    let sql = current_sql_lines.join("\n").trim().to_string();
                    if !sql.is_empty() {
                        entries.push(SlowLogEntry {
                            timestamp: current_time.unwrap_or_else(Utc::now),
                            user: current_user.clone(),
                            thread_id: current_thread_id,
                            query_time_us: current_query_time_us,
                            sql,
                        });
                    }
                    current_sql_lines.clear();
                    in_query = false;
                }

                current_time = parse_mysql_timestamp(time_str);
                if current_time.is_none() {
                    debug!("Failed to parse timestamp: {time_str}");
                }
                continue;
            }

            // # User@Host: app_user[app_user] @ localhost []  Id:    42
            if trimmed.starts_with("# User@Host:") {
                if let Some((user, thread_id)) = parse_user_host(trimmed) {
                    current_user = user;
                    current_thread_id = thread_id;
                }
                continue;
            }

            // # Query_time: 0.001234  Lock_time: 0.000100 ...
            if trimmed.starts_with("# Query_time:") {
                if let Some(qt_us) = parse_query_time(trimmed) {
                    current_query_time_us = qt_us;
                }
                continue;
            }

            // SET timestamp=...; — extract epoch for time reference, don't include as query
            if trimmed.starts_with("SET timestamp=") {
                if let Some(ts) = parse_set_timestamp(trimmed) {
                    // Only use this if we don't have a # Time: line
                    if current_time.is_none() {
                        current_time = Some(ts);
                    }
                }
                in_query = true;
                continue;
            }

            // Any other line is part of the SQL query
            if in_query || !trimmed.starts_with('#') {
                in_query = true;
                current_sql_lines.push(trimmed.to_string());
            }
        }

        // Flush final query
        if in_query && !current_sql_lines.is_empty() {
            let sql = current_sql_lines.join("\n").trim().to_string();
            if !sql.is_empty() {
                entries.push(SlowLogEntry {
                    timestamp: current_time.unwrap_or_else(Utc::now),
                    user: current_user.clone(),
                    thread_id: current_thread_id,
                    query_time_us: current_query_time_us,
                    sql,
                });
            }
        }

        Ok(entries)
    }

    fn build_profile(
        &self,
        entries: Vec<SlowLogEntry>,
        source_host: &str,
        pipeline: Option<&TransformPipeline>,
    ) -> Result<WorkloadProfile> {
        // Group by thread_id (MySQL's equivalent of session)
        let mut session_map: HashMap<u64, Vec<SlowLogEntry>> = HashMap::new();
        for entry in entries {
            session_map
                .entry(entry.thread_id)
                .or_default()
                .push(entry);
        }

        let mut sessions = Vec::new();
        let mut total_queries: u64 = 0;
        let mut next_txn_id: u64 = 1;
        let mut transform_report = TransformReport::default();

        let mut global_min_time: Option<DateTime<Utc>> = None;
        let mut global_max_time: Option<DateTime<Utc>> = None;

        for (thread_id, mut entries) in session_map {
            if entries.is_empty() {
                continue;
            }

            entries.sort_by_key(|e| e.timestamp);

            let first_time = entries[0].timestamp;
            let user = entries[0].user.clone();

            // Track global time range
            for e in &entries {
                match global_min_time {
                    None => global_min_time = Some(e.timestamp),
                    Some(t) if e.timestamp < t => global_min_time = Some(e.timestamp),
                    _ => {}
                }
                match global_max_time {
                    None => global_max_time = Some(e.timestamp),
                    Some(t) if e.timestamp > t => global_max_time = Some(e.timestamp),
                    _ => {}
                }
            }

            let mut queries: Vec<Query> = Vec::new();

            for entry in &entries {
                let sql = if let Some(pipe) = pipeline {
                    let result = pipe.apply(&entry.sql);
                    transform_report.record(&entry.sql, &result);
                    match result {
                        TransformResult::Transformed(sql) => sql,
                        TransformResult::Unchanged => entry.sql.clone(),
                        TransformResult::Skipped { reason } => {
                            debug!("Skipped query: {reason}");
                            continue; // Don't include skipped queries
                        }
                    }
                } else {
                    entry.sql.clone()
                };

                let offset =
                    (entry.timestamp - first_time).num_microseconds().unwrap_or(0) as u64;
                queries.push(Query {
                    sql: sql.clone(),
                    start_offset_us: offset,
                    duration_us: entry.query_time_us,
                    kind: QueryKind::from_sql(&sql),
                    transaction_id: None,
                });
            }

            // Assign transaction IDs
            assign_transaction_ids(&mut queries, &mut next_txn_id);

            total_queries += queries.len() as u64;

            if !queries.is_empty() {
                sessions.push(Session {
                    id: thread_id,
                    user,
                    database: String::new(), // MySQL slow log doesn't include DB per query
                    queries,
                });
            }
        }

        sessions.sort_by_key(|s| s.queries.first().map(|q| q.start_offset_us).unwrap_or(0));

        let capture_duration_us = match (global_min_time, global_max_time) {
            (Some(min), Some(max)) => (max - min).num_microseconds().unwrap_or(0) as u64,
            _ => 0,
        };

        // Print transform report if we ran transforms
        if pipeline.is_some() {
            transform_report.print_summary();
        }

        let total_sessions = sessions.len() as u64;
        Ok(WorkloadProfile {
            version: 2,
            captured_at: Utc::now(),
            source_host: source_host.to_string(),
            pg_version: "unknown".to_string(),
            capture_method: "mysql_slow_log".to_string(),
            sessions,
            metadata: Metadata {
                total_queries,
                total_sessions,
                capture_duration_us,
            },
        })
    }
}

/// Parse MySQL slow log timestamp: "2024-03-08T10:00:00.100000Z"
fn parse_mysql_timestamp(s: &str) -> Option<DateTime<Utc>> {
    let s = s.trim();
    // Try ISO 8601 first
    s.parse::<DateTime<Utc>>().ok().or_else(|| {
        // Try without timezone suffix
        NaiveDateTime::parse_from_str(s, "%Y-%m-%dT%H:%M:%S%.f")
            .ok()
            .map(|ndt| ndt.and_utc())
    })
}

/// Parse "# User@Host: app_user[app_user] @ localhost []  Id:    42"
/// Returns (username, thread_id)
fn parse_user_host(line: &str) -> Option<(String, u64)> {
    let rest = line.strip_prefix("# User@Host:")?.trim();

    // Username is before the first '['
    let bracket_pos = rest.find('[')?;
    let user = rest[..bracket_pos].trim().to_string();

    // Thread ID is after "Id:" at the end
    let id_pos = rest.rfind("Id:")?;
    let id_str = rest[id_pos + 3..].trim();
    let thread_id: u64 = id_str.parse().ok()?;

    Some((user, thread_id))
}

/// Parse "# Query_time: 0.001234  Lock_time: 0.000100 ..."
/// Returns query time in microseconds
fn parse_query_time(line: &str) -> Option<u64> {
    let rest = line.strip_prefix("# Query_time:")?.trim();
    let end = rest.find(|c: char| c.is_whitespace())?;
    let qt_str = &rest[..end];
    let qt_secs: f64 = qt_str.parse().ok()?;
    Some((qt_secs * 1_000_000.0).round() as u64)
}

/// Parse "SET timestamp=1709892000;" and return as DateTime<Utc>
fn parse_set_timestamp(line: &str) -> Option<DateTime<Utc>> {
    let rest = line.strip_prefix("SET timestamp=")?;
    let rest = rest.trim_end_matches(';').trim();
    let epoch: i64 = rest.parse().ok()?;
    DateTime::from_timestamp(epoch, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_mysql_timestamp() {
        let ts = parse_mysql_timestamp("2024-03-08T10:00:00.100000Z").unwrap();
        assert_eq!(ts.format("%Y-%m-%d %H:%M:%S").to_string(), "2024-03-08 10:00:00");
    }

    #[test]
    fn test_parse_user_host() {
        let (user, id) =
            parse_user_host("# User@Host: app_user[app_user] @ localhost []  Id:    42").unwrap();
        assert_eq!(user, "app_user");
        assert_eq!(id, 42);
    }

    #[test]
    fn test_parse_user_host_with_ip() {
        let (user, id) =
            parse_user_host("# User@Host: admin[admin] @ 192.168.1.10 []  Id:    99").unwrap();
        assert_eq!(user, "admin");
        assert_eq!(id, 99);
    }

    #[test]
    fn test_parse_query_time() {
        assert_eq!(
            parse_query_time("# Query_time: 0.001234  Lock_time: 0.000100 Rows_sent: 1  Rows_examined: 100"),
            Some(1234)
        );
        assert_eq!(
            parse_query_time("# Query_time: 0.050000  Lock_time: 0.001000 Rows_sent: 5000  Rows_examined: 100000"),
            Some(50000)
        );
    }

    #[test]
    fn test_parse_set_timestamp() {
        let ts = parse_set_timestamp("SET timestamp=1709892000;").unwrap();
        assert_eq!(ts.format("%Y-%m-%d").to_string(), "2024-03-08");
    }
}
```

**Step 3: Add to `src/capture/mod.rs`**

Add after `pub mod masking;`:
```rust
pub mod mysql_slow;
```

**Step 4: Create `tests/mysql_slow_test.rs`**

```rust
use pg_retest::capture::mysql_slow::MysqlSlowLogCapture;
use pg_retest::profile::QueryKind;

#[test]
fn test_mysql_slow_log_capture_with_transform() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "mysql-host",
            true, // transform enabled
        )
        .unwrap();

    assert_eq!(profile.capture_method, "mysql_slow_log");
    assert_eq!(profile.source_host, "mysql-host");

    // We expect 3 sessions: thread 42 (5 queries after transform), thread 55 (1 query), thread 99 (skipped - SHOW)
    // Thread 99's SHOW VARIABLES is skipped by the transform pipeline
    assert!(profile.sessions.len() >= 2, "Expected at least 2 sessions, got {}", profile.sessions.len());

    let total = profile.metadata.total_queries;
    // 8 entries in fixture, 1 SHOW is skipped = 7 queries
    assert!(total >= 6, "Expected at least 6 queries, got {total}");
}

#[test]
fn test_mysql_slow_log_capture_no_transform() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "mysql-host",
            false, // no transform
        )
        .unwrap();

    // Without transform, all queries including SHOW should be present
    assert!(profile.metadata.total_queries >= 7, "Expected at least 7 queries without transform");
}

#[test]
fn test_mysql_slow_log_timing_preserved() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            false,
        )
        .unwrap();

    // Find a session with queries
    let session = profile.sessions.iter().find(|s| s.queries.len() > 1).unwrap();

    // First query should have the expected duration (1234us for first entry in thread 42)
    let first_q = &session.queries[0];
    assert!(first_q.duration_us > 0, "Duration should be > 0");
}

#[test]
fn test_mysql_slow_log_backticks_transformed() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            true, // transform enabled
        )
        .unwrap();

    // The second query in thread 42 has backticks — they should be converted to double quotes
    let all_sql: Vec<&str> = profile
        .sessions
        .iter()
        .flat_map(|s| s.queries.iter())
        .map(|q| q.sql.as_str())
        .collect();

    // No backticks should remain after transform
    for sql in &all_sql {
        assert!(!sql.contains('`'), "Backtick found in transformed SQL: {sql}");
    }
}

#[test]
fn test_mysql_slow_log_transaction_ids() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            true,
        )
        .unwrap();

    // Thread 42 has BEGIN + UPDATE + COMMIT — should get transaction IDs
    let thread42 = profile.sessions.iter().find(|s| s.id == 42).unwrap();

    let begin_q = thread42.queries.iter().find(|q| q.kind == QueryKind::Begin);
    assert!(begin_q.is_some(), "Should have a BEGIN query");
    assert!(begin_q.unwrap().transaction_id.is_some(), "BEGIN should have transaction_id");

    let commit_q = thread42.queries.iter().find(|q| q.kind == QueryKind::Commit);
    assert!(commit_q.is_some(), "Should have a COMMIT query");
    assert_eq!(
        begin_q.unwrap().transaction_id,
        commit_q.unwrap().transaction_id,
        "BEGIN and COMMIT should have the same transaction_id"
    );
}

#[test]
fn test_mysql_slow_log_multiline_query() {
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            false,
        )
        .unwrap();

    // Thread 55 has a multi-line JOIN query
    let thread55 = profile.sessions.iter().find(|s| s.id == 55).unwrap();
    let q = &thread55.queries[0];
    assert!(q.sql.contains("JOIN"), "Multi-line query should contain JOIN");
    assert!(q.sql.contains("ORDER BY"), "Multi-line query should contain ORDER BY");
    assert_eq!(q.duration_us, 50000, "Query time should be 50000us (0.05s)");
}
```

**Step 5: Run tests**

```bash
cargo test --lib capture::mysql_slow
cargo test --test mysql_slow_test
```

**Step 6: Commit**

```bash
git add src/capture/mysql_slow.rs src/capture/mod.rs tests/fixtures/sample_mysql_slow.log tests/mysql_slow_test.rs
git commit -m "feat(capture): MySQL slow query log parser with SQL transform pipeline"
```

---

## Task 4: CLI Integration — `--source-type` Flag

**Files:**
- Modify: `src/cli.rs`
- Modify: `src/main.rs`

**What this does:** Add `--source-type` flag to the `capture` subcommand. Values: `pg-csv` (default, existing behavior), `mysql-slow`. The capture command dispatches to the appropriate parser.

**Step 1: Modify `src/cli.rs`**

Add a new field to `CaptureArgs` after the `source_log` field:
```rust
    /// Source log type: pg-csv (default), mysql-slow
    #[arg(long, default_value = "pg-csv")]
    pub source_type: String,
```

**Step 2: Modify `src/main.rs`**

Update `cmd_capture` to dispatch based on `source_type`:

Replace the body of `cmd_capture` with:
```rust
fn cmd_capture(args: pg_retest::cli::CaptureArgs) -> Result<()> {
    use pg_retest::capture::csv_log::CsvLogCapture;
    use pg_retest::capture::masking::mask_sql_literals;
    use pg_retest::capture::mysql_slow::MysqlSlowLogCapture;
    use pg_retest::profile::io;

    let mut profile = match args.source_type.as_str() {
        "pg-csv" => {
            let capture = CsvLogCapture;
            capture.capture_from_file(&args.source_log, &args.source_host, &args.pg_version)?
        }
        "mysql-slow" => {
            let capture = MysqlSlowLogCapture;
            capture.capture_from_file(&args.source_log, &args.source_host, true)?
        }
        other => anyhow::bail!("Unknown source type: {other}. Supported: pg-csv, mysql-slow"),
    };

    if args.mask_values {
        for session in &mut profile.sessions {
            for query in &mut session.queries {
                query.sql = mask_sql_literals(&query.sql);
            }
        }
        println!("Applied PII masking to SQL literals");
    }

    println!(
        "Captured {} queries across {} sessions",
        profile.metadata.total_queries, profile.metadata.total_sessions
    );

    io::write_profile(&args.output, &profile)?;
    println!("Wrote workload profile to {}", args.output.display());
    Ok(())
}
```

**Step 3: Verify compilation and --help**

```bash
cargo build
cargo run -- capture --help
```

Expected: `--source-type` flag visible with default `pg-csv`.

**Step 4: Smoke test with fixture**

```bash
cargo run -- capture --source-log tests/fixtures/sample_mysql_slow.log --source-type mysql-slow -o /tmp/mysql_test.wkl
cargo run -- inspect /tmp/mysql_test.wkl
```

**Step 5: Run cargo fmt and clippy**

**Step 6: Commit**

```bash
git add src/cli.rs src/main.rs
git commit -m "feat(cli): add --source-type flag for MySQL slow log capture"
```

---

## Task 5: Pipeline Config Support for MySQL Capture

**Files:**
- Modify: `src/config/mod.rs`
- Modify: `src/pipeline/mod.rs`
- Modify: `tests/config_test.rs` (add new config tests)

**What this does:** Add `source_type` field to `CaptureConfig` so the `pg-retest run` pipeline can use MySQL capture via TOML config.

**Step 1: Modify `src/config/mod.rs`**

Add to `CaptureConfig` after the `pg_version` field:
```rust
    /// Source type: "pg-csv" (default), "mysql-slow"
    #[serde(default = "default_source_type")]
    pub source_type: String,
```

Add the default function:
```rust
fn default_source_type() -> String {
    "pg-csv".to_string()
}
```

**Step 2: Modify `src/pipeline/mod.rs`**

Update `load_or_capture_workload` to use `source_type` — find the section that does `let capture = CsvLogCapture;` and replace with:

```rust
    // Dispatch to appropriate capture backend
    info!("Capturing from {} (type: {})", source_log.display(), capture_cfg.source_type);
    let mut profile = match capture_cfg.source_type.as_str() {
        "pg-csv" => {
            let capture = CsvLogCapture;
            capture
                .capture_from_file(
                    source_log,
                    capture_cfg.source_host.as_deref().unwrap_or("unknown"),
                    capture_cfg.pg_version.as_deref().unwrap_or("unknown"),
                )
                .map_err(|e| anyhow::anyhow!("Capture error: {e}"))?
        }
        "mysql-slow" => {
            use crate::capture::mysql_slow::MysqlSlowLogCapture;
            let capture = MysqlSlowLogCapture;
            capture
                .capture_from_file(
                    source_log,
                    capture_cfg.source_host.as_deref().unwrap_or("unknown"),
                    true, // always transform MySQL→PG
                )
                .map_err(|e| anyhow::anyhow!("Capture error: {e}"))?
        }
        other => anyhow::bail!("Capture error: unknown source_type: {other}"),
    };
```

Also add the `MysqlSlowLogCapture` import near the top of the function only for the `"mysql-slow"` branch (it's already scoped).

**Step 3: Add config test**

Append to the existing unit tests in `src/config/mod.rs`:
```rust
    #[test]
    fn test_parse_mysql_config() {
        let toml = r#"
[capture]
source_log = "mysql_slow.log"
source_type = "mysql-slow"
source_host = "mysql-prod"

[replay]
target = "host=localhost dbname=test"
read_only = true
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.capture.as_ref().unwrap().source_type, "mysql-slow");
    }

    #[test]
    fn test_default_source_type_is_pg_csv() {
        let toml = r#"
[capture]
source_log = "pg_log.csv"

[replay]
target = "host=localhost"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.capture.as_ref().unwrap().source_type, "pg-csv");
    }
```

**Step 4: Run tests**

```bash
cargo test --lib config
cargo test
```

**Step 5: Commit**

```bash
git add src/config/mod.rs src/pipeline/mod.rs
git commit -m "feat(pipeline): MySQL slow log support in CI/CD pipeline config"
```

---

## Task 6: Full Integration Test + Polish

**Files:**
- Create: `tests/mysql_integration_test.rs`
- Modify: `tests/fixtures/sample_config.toml` (add MySQL example)

**What this does:** End-to-end integration test that captures from MySQL slow log, transforms SQL, writes to `.wkl`, reads it back, and verifies the profile is correct. Also add a MySQL config example.

**Step 1: Create `tests/mysql_integration_test.rs`**

```rust
use pg_retest::capture::mysql_slow::MysqlSlowLogCapture;
use pg_retest::profile::io;
use tempfile::NamedTempFile;

#[test]
fn test_mysql_capture_roundtrip() {
    // Capture from MySQL slow log with transform
    let capture = MysqlSlowLogCapture;
    let profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "mysql-test",
            true,
        )
        .unwrap();

    // Write to .wkl file
    let file = NamedTempFile::with_suffix(".wkl").unwrap();
    io::write_profile(file.path(), &profile).unwrap();

    // Read back
    let loaded = io::read_profile(file.path()).unwrap();

    assert_eq!(loaded.source_host, "mysql-test");
    assert_eq!(loaded.capture_method, "mysql_slow_log");
    assert_eq!(loaded.metadata.total_sessions, profile.metadata.total_sessions);
    assert_eq!(loaded.metadata.total_queries, profile.metadata.total_queries);

    // Verify no backticks in any SQL (transform applied)
    for session in &loaded.sessions {
        for query in &session.queries {
            assert!(
                !query.sql.contains('`'),
                "Found backtick in SQL after transform: {}",
                query.sql
            );
        }
    }
}

#[test]
fn test_mysql_capture_with_masking() {
    use pg_retest::capture::masking::mask_sql_literals;

    let capture = MysqlSlowLogCapture;
    let mut profile = capture
        .capture_from_file(
            std::path::Path::new("tests/fixtures/sample_mysql_slow.log"),
            "test",
            true,
        )
        .unwrap();

    // Apply masking
    for session in &mut profile.sessions {
        for query in &mut session.queries {
            query.sql = mask_sql_literals(&query.sql);
        }
    }

    // Verify masking was applied — numeric values should be replaced with $N
    let has_masked = profile
        .sessions
        .iter()
        .flat_map(|s| &s.queries)
        .any(|q| q.sql.contains("$N") || q.sql.contains("$S"));
    assert!(has_masked, "PII masking should have replaced at least some literals");
}

#[test]
fn test_mysql_pipeline_config_roundtrip() {
    use pg_retest::config::{CaptureConfig, PipelineConfig, ReplayConfig};
    use pg_retest::pipeline::{self, run_pipeline};
    use std::path::PathBuf;

    let config = PipelineConfig {
        capture: Some(CaptureConfig {
            workload: None,
            source_log: Some(PathBuf::from("tests/fixtures/sample_mysql_slow.log")),
            source_host: Some("mysql-test".into()),
            pg_version: None,
            mask_values: false,
            source_type: "mysql-slow".into(),
        }),
        provision: None,
        replay: ReplayConfig {
            speed: 0.0,
            read_only: true,
            scale: 1,
            stagger_ms: 0,
            target: Some("host=127.0.0.1 port=1 dbname=test".into()), // will fail at replay
        },
        thresholds: None,
        output: None,
    };

    let result = run_pipeline(&config);
    // Pipeline should get past capture (not EXIT_CAPTURE_ERROR)
    // It will fail at replay since port 1 is not reachable
    assert_ne!(result.exit_code, pipeline::EXIT_CAPTURE_ERROR,
        "Pipeline should not fail at capture stage");
}
```

**Step 2: Run all tests**

```bash
cargo test
cargo clippy
```

**Step 3: Commit**

```bash
git add tests/mysql_integration_test.rs
git commit -m "test(mysql): integration tests for MySQL capture + transform + pipeline"
```

---

## Build Order & Dependencies

```
Task 1: Transform pipeline framework        ← foundation, regex dep
Task 2: MySQL-to-PG transform rules         ← depends on Task 1
Task 3: MySQL slow log parser               ← depends on Tasks 1-2
Task 4: CLI --source-type flag              ← depends on Task 3
Task 5: Pipeline config MySQL support        ← depends on Tasks 3-4
Task 6: Integration tests + polish           ← depends on Tasks 4-5
```

All tasks are sequential.

---

## Verification Checklist

After all tasks:
- [ ] `cargo test` — all tests pass (existing 107 + new transform/mysql/pipeline tests)
- [ ] `cargo clippy` — zero warnings
- [ ] `cargo run -- capture --help` — shows `--source-type` flag
- [ ] `cargo run -- capture --source-log tests/fixtures/sample_mysql_slow.log --source-type mysql-slow -o /tmp/mysql.wkl` — captures successfully
- [ ] `cargo run -- inspect /tmp/mysql.wkl` — shows profile with `capture_method: "mysql_slow_log"`, no backticks in SQL
- [ ] No backtick-quoted identifiers remain in transformed SQL
- [ ] SHOW/SET NAMES/USE queries are skipped in transform
- [ ] Multi-line SQL queries are handled correctly
- [ ] Transaction boundaries (BEGIN/COMMIT) get correct transaction IDs
