use std::collections::{BTreeSet, HashMap, HashSet};

use regex::Regex;
use serde::Serialize;

use crate::profile::WorkloadProfile;

// ---------------------------------------------------------------------------
// Public output types
// ---------------------------------------------------------------------------

/// Full workload analysis result.
#[derive(Debug, Clone, Serialize)]
pub struct WorkloadAnalysis {
    pub profile_summary: ProfileSummary,
    pub query_groups: Vec<AnalyzedGroup>,
    pub ungrouped_queries: usize,
}

/// High-level summary of the captured workload.
#[derive(Debug, Clone, Serialize)]
pub struct ProfileSummary {
    pub total_queries: u64,
    pub total_sessions: u64,
    pub capture_duration_s: f64,
    pub source_host: String,
}

/// A group of queries that share table access patterns.
#[derive(Debug, Clone, Serialize)]
pub struct AnalyzedGroup {
    pub id: usize,
    pub tables: Vec<String>,
    pub query_count: usize,
    pub sample_queries: Vec<String>,
    pub kinds: HashMap<String, usize>,
    pub avg_duration_us: u64,
    pub sessions: Vec<u64>,
    pub pct_of_total: f64,
    pub parameter_patterns: ParameterPatterns,
}

/// Information about parameterised patterns in a query group.
#[derive(Debug, Clone, Serialize)]
pub struct ParameterPatterns {
    pub common_filters: Vec<String>,
    pub bind_params_seen: usize,
}

// ---------------------------------------------------------------------------
// SQL keyword set
// ---------------------------------------------------------------------------

fn is_sql_keyword(word: &str) -> bool {
    matches!(
        word,
        "select"
            | "from"
            | "where"
            | "and"
            | "or"
            | "not"
            | "in"
            | "is"
            | "null"
            | "true"
            | "false"
            | "as"
            | "on"
            | "set"
            | "values"
            | "order"
            | "by"
            | "group"
            | "having"
            | "limit"
            | "offset"
            | "union"
            | "all"
            | "exists"
            | "between"
            | "like"
            | "case"
            | "when"
            | "then"
            | "else"
            | "end"
            | "inner"
            | "outer"
            | "left"
            | "right"
            | "cross"
            | "natural"
            | "using"
            | "into"
            | "update"
            | "delete"
            | "insert"
            | "join"
            | "with"
            | "distinct"
            | "asc"
            | "desc"
            | "create"
            | "alter"
            | "drop"
            | "table"
            | "index"
            | "view"
            | "begin"
            | "commit"
            | "rollback"
            | "abort"
            | "truncate"
            | "for"
            | "each"
            | "row"
            | "returning"
            | "default"
            | "primary"
            | "key"
            | "references"
            | "foreign"
            | "constraint"
            | "check"
            | "unique"
            | "if"
            | "cascade"
            | "restrict"
            | "no"
            | "action"
            | "only"
            | "recursive"
            | "lateral"
            | "any"
            | "some"
            | "fetch"
            | "next"
            | "first"
            | "last"
            | "rows"
            | "preceding"
            | "following"
            | "current"
            | "over"
            | "partition"
            | "window"
            | "range"
            | "unbounded"
            | "ilike"
            | "similar"
            | "to"
            | "do"
            | "nothing"
            | "conflict"
            | "excluded"
            | "coalesce"
            | "cast"
            | "extract"
            | "epoch"
            | "interval"
            | "timestamp"
            | "date"
            | "time"
            | "zone"
            | "at"
            | "text"
            | "int"
            | "integer"
            | "bigint"
            | "smallint"
            | "boolean"
            | "varchar"
            | "char"
            | "numeric"
            | "decimal"
            | "real"
            | "float"
            | "serial"
            | "bigserial"
            | "uuid"
            | "jsonb"
            | "json"
            | "array"
            | "type"
            | "enum"
            | "start"
            | "transaction"
            | "work"
            | "isolation"
            | "level"
            | "read"
            | "write"
            | "committed"
            | "uncommitted"
            | "repeatable"
            | "serializable"
            | "savepoint"
            | "release"
            | "count"
            | "sum"
            | "avg"
            | "min"
            | "max"
            | "now"
            | "upper"
            | "lower"
            | "length"
            | "substring"
            | "replace"
            | "trim"
            | "position"
            | "greatest"
            | "least"
            | "abs"
            | "ceil"
            | "floor"
            | "round"
    )
}

// ---------------------------------------------------------------------------
// Table extraction
// ---------------------------------------------------------------------------

/// Extract table names referenced in a SQL statement.
///
/// Handles FROM, JOIN, INTO, and UPDATE clauses. Results are lowercased
/// and SQL keywords are excluded.
pub fn extract_tables(sql: &str) -> BTreeSet<String> {
    let mut tables = BTreeSet::new();

    // Pattern: FROM <table>, JOIN <table>, INTO <table>, UPDATE <table>
    // The table name is the next identifier token after the keyword.
    let re = Regex::new(r"(?i)\b(?:FROM|JOIN|INTO|UPDATE)\s+([A-Za-z_][A-Za-z0-9_.]*)")
        .expect("table regex");

    for cap in re.captures_iter(sql) {
        let name = cap[1].to_lowercase();
        // Take only the table part if schema-qualified (schema.table)
        let table = name.rsplit('.').next().unwrap_or(&name);
        if !is_sql_keyword(table) {
            tables.insert(table.to_string());
        }
    }

    tables
}

// ---------------------------------------------------------------------------
// Filter column extraction
// ---------------------------------------------------------------------------

/// Extract column names from WHERE clauses.
///
/// Looks for `column_name <op>` patterns where `<op>` is one of
/// `=`, `!=`, `<>`, `<`, `>`, `<=`, `>=`, `IN`, `LIKE`, `ILIKE`,
/// `BETWEEN`, `IS`.
pub fn extract_filter_columns(sql: &str) -> Vec<String> {
    let mut cols = Vec::new();
    let mut seen = HashSet::new();

    // Find WHERE clause content. We capture everything after WHERE until
    // a top-level ORDER BY, GROUP BY, HAVING, LIMIT, or end of string.
    let where_re =
        Regex::new(r"(?i)\bWHERE\s+(.*?)(?:\bORDER\b|\bGROUP\b|\bHAVING\b|\bLIMIT\b|\bFOR\b|$)")
            .expect("where regex");

    // Within the WHERE clause, look for column names before operators.
    let col_re = Regex::new(
        r"(?i)\b([A-Za-z_][A-Za-z0-9_]*)\s*(?:=|!=|<>|<=|>=|<|>|\bIN\b|\bLIKE\b|\bILIKE\b|\bBETWEEN\b|\bIS\b)"
    )
    .expect("column regex");

    for where_cap in where_re.captures_iter(sql) {
        let clause = &where_cap[1];
        for col_cap in col_re.captures_iter(clause) {
            let col = col_cap[1].to_lowercase();
            if !is_sql_keyword(&col) && seen.insert(col.clone()) {
                cols.push(col);
            }
        }
    }

    cols
}

// ---------------------------------------------------------------------------
// Union-Find
// ---------------------------------------------------------------------------

struct UnionFind {
    parent: Vec<usize>,
    rank: Vec<usize>,
}

impl UnionFind {
    fn new(n: usize) -> Self {
        Self {
            parent: (0..n).collect(),
            rank: vec![0; n],
        }
    }

    fn find(&mut self, x: usize) -> usize {
        if self.parent[x] != x {
            self.parent[x] = self.find(self.parent[x]); // path compression
        }
        self.parent[x]
    }

    fn union(&mut self, a: usize, b: usize) {
        let ra = self.find(a);
        let rb = self.find(b);
        if ra == rb {
            return;
        }
        // union by rank
        match self.rank[ra].cmp(&self.rank[rb]) {
            std::cmp::Ordering::Less => self.parent[ra] = rb,
            std::cmp::Ordering::Greater => self.parent[rb] = ra,
            std::cmp::Ordering::Equal => {
                self.parent[rb] = ra;
                self.rank[ra] += 1;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Bind-parameter counting
// ---------------------------------------------------------------------------

fn count_bind_params(sql: &str) -> usize {
    let re = Regex::new(r"\$\d+").expect("bind param regex");
    re.find_iter(sql).count()
}

// ---------------------------------------------------------------------------
// Main analysis entry point
// ---------------------------------------------------------------------------

/// Analyse a workload profile: extract tables, group queries by shared table
/// access via Union-Find, and compute per-group statistics.
pub fn analyze_workload(profile: &WorkloadProfile) -> WorkloadAnalysis {
    // 1. Flatten all queries across sessions, keeping (session_id, &Query).
    let mut flat: Vec<(u64, &crate::profile::Query)> = Vec::new();
    for session in &profile.sessions {
        for query in &session.queries {
            flat.push((session.id, query));
        }
    }

    // 2. Extract tables for every query.
    let query_tables: Vec<BTreeSet<String>> =
        flat.iter().map(|(_, q)| extract_tables(&q.sql)).collect();

    // 3. Build a table-name -> index mapping for Union-Find.
    let mut table_index: HashMap<String, usize> = HashMap::new();
    let mut table_list: Vec<String> = Vec::new();
    for tables in &query_tables {
        for t in tables {
            if !table_index.contains_key(t) {
                let idx = table_list.len();
                table_index.insert(t.clone(), idx);
                table_list.push(t.clone());
            }
        }
    }

    // 4. Union tables that co-occur in the same query.
    let mut uf = UnionFind::new(table_list.len());
    for tables in &query_tables {
        let indices: Vec<usize> = tables
            .iter()
            .filter_map(|t| table_index.get(t).copied())
            .collect();
        for pair in indices.windows(2) {
            uf.union(pair[0], pair[1]);
        }
    }

    // 5. Map each table to its root representative.
    let mut root_to_tables: HashMap<usize, BTreeSet<String>> = HashMap::new();
    for (name, &idx) in &table_index {
        let root = uf.find(idx);
        root_to_tables.entry(root).or_default().insert(name.clone());
    }

    // 6. Group queries by their root. Queries whose tables are empty go to
    //    the "ungrouped" bucket.
    //    root -> (query indices, session ids, queries refs)
    let mut group_queries: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut ungrouped: usize = 0;

    for (qi, tables) in query_tables.iter().enumerate() {
        if tables.is_empty() {
            ungrouped += 1;
            continue;
        }
        // Pick any table from the set (first) to determine the root.
        let first_table = tables.iter().next().unwrap();
        let table_idx = table_index[first_table];
        let root = uf.find(table_idx);
        group_queries.entry(root).or_default().push(qi);
    }

    // 7. Build AnalyzedGroup per root.
    let total_grouped = flat.len() - ungrouped;
    let total_queries = flat.len();

    let mut groups: Vec<AnalyzedGroup> = Vec::new();
    for (root, qindices) in &group_queries {
        let tables_set = &root_to_tables[root];
        let tables_vec: Vec<String> = tables_set.iter().cloned().collect();

        // Stats
        let mut kinds: HashMap<String, usize> = HashMap::new();
        let mut total_duration: u64 = 0;
        let mut session_set: BTreeSet<u64> = BTreeSet::new();
        let mut sample_queries: Vec<String> = Vec::new();
        let mut bind_params_seen: usize = 0;
        let mut all_filter_cols: Vec<String> = Vec::new();
        let mut seen_filters: HashSet<String> = HashSet::new();

        for &qi in qindices {
            let (sid, q) = &flat[qi];
            session_set.insert(*sid);
            total_duration += q.duration_us;

            let kind_label = format!("{:?}", q.kind);
            *kinds.entry(kind_label).or_insert(0) += 1;

            if sample_queries.len() < 5 {
                // Deduplicate samples (by normalised SQL prefix).
                let preview: String = q.sql.chars().take(200).collect();
                if !sample_queries.contains(&preview) {
                    sample_queries.push(preview);
                }
            }

            bind_params_seen += count_bind_params(&q.sql);

            for col in extract_filter_columns(&q.sql) {
                if seen_filters.insert(col.clone()) {
                    all_filter_cols.push(col);
                }
            }
        }

        let query_count = qindices.len();
        let avg_duration = if query_count > 0 {
            total_duration / query_count as u64
        } else {
            0
        };
        let pct = if total_queries > 0 {
            (query_count as f64 / total_queries as f64) * 100.0
        } else {
            0.0
        };

        groups.push(AnalyzedGroup {
            id: 0, // assigned after sorting
            tables: tables_vec,
            query_count,
            sample_queries,
            kinds,
            avg_duration_us: avg_duration,
            sessions: session_set.into_iter().collect(),
            pct_of_total: pct,
            parameter_patterns: ParameterPatterns {
                common_filters: all_filter_cols,
                bind_params_seen,
            },
        });
    }

    // 8. Sort groups by query_count descending, assign IDs.
    groups.sort_by(|a, b| b.query_count.cmp(&a.query_count));
    for (i, g) in groups.iter_mut().enumerate() {
        g.id = i;
    }

    // Ensure ungrouped count also includes any remainder.
    let _ = total_grouped; // already accounted for

    WorkloadAnalysis {
        profile_summary: ProfileSummary {
            total_queries: profile.metadata.total_queries,
            total_sessions: profile.metadata.total_sessions,
            capture_duration_s: profile.metadata.capture_duration_us as f64 / 1_000_000.0,
            source_host: profile.source_host.clone(),
        },
        query_groups: groups,
        ungrouped_queries: ungrouped,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::*;

    #[test]
    fn test_extract_tables_select() {
        let tables = extract_tables(
            "SELECT p.name, c.title FROM products p JOIN categories c ON p.category_id = c.id",
        );
        assert!(tables.contains("products"));
        assert!(tables.contains("categories"));
    }

    #[test]
    fn test_extract_tables_insert() {
        let tables = extract_tables("INSERT INTO orders (customer_id, total) VALUES ($1, $2)");
        assert!(tables.contains("orders"));
    }

    #[test]
    fn test_extract_tables_update() {
        let tables = extract_tables("UPDATE products SET price = $1 WHERE id = $2");
        assert!(tables.contains("products"));
    }

    #[test]
    fn test_extract_tables_delete() {
        let tables = extract_tables("DELETE FROM order_items WHERE order_id = $1");
        assert!(tables.contains("order_items"));
    }

    #[test]
    fn test_extract_tables_subquery() {
        let tables = extract_tables(
            "SELECT * FROM products WHERE category_id IN (SELECT id FROM categories WHERE active = true)",
        );
        assert!(tables.contains("products"));
        assert!(tables.contains("categories"));
    }

    #[test]
    fn test_extract_tables_no_tables() {
        let tables = extract_tables("SET statement_timeout = '30s'");
        assert!(tables.is_empty());
    }

    #[test]
    fn test_extract_filter_columns() {
        let cols =
            extract_filter_columns("SELECT * FROM products WHERE category_id = $1 AND price > 100");
        assert!(cols.contains(&"category_id".to_string()));
        assert!(cols.contains(&"price".to_string()));
    }

    #[test]
    fn test_group_queries_by_tables() {
        let profile = WorkloadProfile {
            version: 2,
            captured_at: chrono::Utc::now(),
            source_host: "localhost".into(),
            pg_version: "16".into(),
            capture_method: "test".into(),
            sessions: vec![Session {
                id: 1,
                user: "test".into(),
                database: "testdb".into(),
                queries: vec![
                    Query {
                        sql: "SELECT * FROM products WHERE id = $1".into(),
                        start_offset_us: 0,
                        duration_us: 100,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT * FROM products JOIN categories ON products.category_id = categories.id".into(),
                        start_offset_us: 100,
                        duration_us: 200,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "INSERT INTO orders (product_id) VALUES ($1)".into(),
                        start_offset_us: 300,
                        duration_us: 50,
                        kind: QueryKind::Insert,
                        transaction_id: None,
                    },
                ],
            }],
            metadata: Metadata {
                total_queries: 3,
                total_sessions: 1,
                capture_duration_us: 350,
            },
        };

        let analysis = analyze_workload(&profile);
        // products and categories should be in the same group (shared via JOIN query)
        // orders should be in a separate group
        assert!(analysis.query_groups.len() >= 2);
        let product_group = analysis
            .query_groups
            .iter()
            .find(|g| g.tables.contains(&"products".to_string()))
            .unwrap();
        assert!(product_group.tables.contains(&"categories".to_string()));
        assert_eq!(product_group.query_count, 2);
        let orders_group = analysis
            .query_groups
            .iter()
            .find(|g| g.tables.contains(&"orders".to_string()))
            .unwrap();
        assert_eq!(orders_group.query_count, 1);
    }

    #[test]
    fn test_analysis_summary() {
        let profile = WorkloadProfile {
            version: 2,
            captured_at: chrono::Utc::now(),
            source_host: "localhost:5432".into(),
            pg_version: "16".into(),
            capture_method: "test".into(),
            sessions: vec![Session {
                id: 1,
                user: "app".into(),
                database: "mydb".into(),
                queries: vec![
                    Query {
                        sql: "SELECT 1".into(),
                        start_offset_us: 0,
                        duration_us: 100,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SET statement_timeout = '30s'".into(),
                        start_offset_us: 100,
                        duration_us: 10,
                        kind: QueryKind::Other,
                        transaction_id: None,
                    },
                ],
            }],
            metadata: Metadata {
                total_queries: 2,
                total_sessions: 1,
                capture_duration_us: 110,
            },
        };

        let analysis = analyze_workload(&profile);
        assert_eq!(analysis.profile_summary.total_queries, 2);
        assert_eq!(analysis.profile_summary.total_sessions, 1);
        // Both queries have no extractable tables, so both are ungrouped
        assert_eq!(analysis.ungrouped_queries, 2);
    }
}
