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
                reason: format!(
                    "MySQL-specific command: {}",
                    &sql.trim()[..sql.trim().len().min(40)]
                ),
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
struct LimitOffsetRewrite;

impl SqlTransformer for LimitOffsetRewrite {
    fn transform(&self, sql: &str) -> TransformResult {
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
struct IfToCase;

impl SqlTransformer for IfToCase {
    fn transform(&self, sql: &str) -> TransformResult {
        let re = Regex::new(r"(?i)\bIF\s*\(").unwrap();
        if !re.is_match(sql) {
            return TransformResult::Unchanged;
        }

        let mut result = sql.to_string();
        let mut changed = false;

        while let Some(m) = re.find(&result) {
            let start = m.start();
            let paren_start = m.end() - 1;

            let Some((inner, end)) = extract_balanced_parens(&result, paren_start) else {
                break;
            };

            let parts = split_at_depth_zero(&inner);
            if parts.len() == 3 {
                let cond = parts[0].trim();
                let then_val = parts[1].trim();
                let else_val = parts[2].trim();
                let replacement = format!("CASE WHEN {cond} THEN {then_val} ELSE {else_val} END");
                result = format!("{}{replacement}{}", &result[..start], &result[end + 1..]);
                changed = true;
            } else {
                break;
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
        while let Some(m) = re_with_arg.find(&result) {
            let start = m.start();
            let paren_start = m.end() - 1;

            match extract_balanced_parens(&result, paren_start) {
                Some((inner, end)) => {
                    let replacement = format!("EXTRACT(EPOCH FROM {})::bigint", inner.trim());
                    result = format!("{}{replacement}{}", &result[..start], &result[end + 1..]);
                    changed = true;
                }
                None => break,
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
                    current.push('\'');
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
        assert_eq!(
            t.transform("SELECT * FROM t LIMIT 10, 20"),
            TransformResult::Transformed("SELECT * FROM t LIMIT 20 OFFSET 10".into())
        );
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
        assert_eq!(t.transform("SELECT 1"), TransformResult::Unchanged);
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
