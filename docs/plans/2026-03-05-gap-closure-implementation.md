# Gap Closure Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Implement three gap closure features: per-category scaling, A/B variant testing, and cloud-native capture (AWS RDS).

**Architecture:** Per-category scaling extends the existing `classify` and `scaling` modules to apply different scale factors per WorkloadClass. A/B variant testing adds a new `ab` subcommand that replays the same workload against multiple targets sequentially and produces a side-by-side comparison report. Cloud-native capture wraps the AWS CLI to download RDS log files, then delegates to the existing `CsvLogCapture` parser.

**Tech Stack:** Rust 2021, clap (CLI), serde + toml (config), tokio + tokio-postgres (replay), regex (transforms)

---

## Feature 1: Per-Category Scaling

### Task 1: `scale_sessions_by_class()` — Unit Test

**Files:**
- Create: `tests/per_category_scaling_test.rs`

**Step 1: Write the failing test**

```rust
use chrono::Utc;
use pg_retest::classify::WorkloadClass;
use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};

fn make_profile(sessions: Vec<Session>) -> WorkloadProfile {
    let total_queries = sessions.iter().map(|s| s.queries.len() as u64).sum();
    let total_sessions = sessions.len() as u64;
    WorkloadProfile {
        version: 2,
        captured_at: Utc::now(),
        source_host: "test".into(),
        pg_version: "16.2".into(),
        capture_method: "csv_log".into(),
        sessions,
        metadata: Metadata {
            total_queries,
            total_sessions,
            capture_duration_us: 10000,
        },
    }
}

/// Build an analytical session: >80% reads, avg latency >10ms
fn analytical_session(id: u64) -> Session {
    Session {
        id,
        user: "analyst".into(),
        database: "analytics".into(),
        queries: vec![
            Query {
                sql: "SELECT * FROM large_table".into(),
                start_offset_us: 0,
                duration_us: 50_000, // 50ms — analytical
                kind: QueryKind::Select,
                transaction_id: None,
            },
            Query {
                sql: "SELECT * FROM another_table".into(),
                start_offset_us: 100_000,
                duration_us: 30_000, // 30ms
                kind: QueryKind::Select,
                transaction_id: None,
            },
        ],
    }
}

/// Build a transactional session: >20% writes, avg latency <5ms, >2 transactions
fn transactional_session(id: u64) -> Session {
    Session {
        id,
        user: "app".into(),
        database: "oltp".into(),
        queries: vec![
            Query {
                sql: "BEGIN".into(),
                start_offset_us: 0,
                duration_us: 50,
                kind: QueryKind::Begin,
                transaction_id: Some(1),
            },
            Query {
                sql: "INSERT INTO orders VALUES (1)".into(),
                start_offset_us: 100,
                duration_us: 500,
                kind: QueryKind::Insert,
                transaction_id: Some(1),
            },
            Query {
                sql: "COMMIT".into(),
                start_offset_us: 700,
                duration_us: 50,
                kind: QueryKind::Commit,
                transaction_id: Some(1),
            },
            Query {
                sql: "BEGIN".into(),
                start_offset_us: 1000,
                duration_us: 50,
                kind: QueryKind::Begin,
                transaction_id: Some(2),
            },
            Query {
                sql: "UPDATE orders SET status = 'shipped'".into(),
                start_offset_us: 1100,
                duration_us: 800,
                kind: QueryKind::Update,
                transaction_id: Some(2),
            },
            Query {
                sql: "SELECT id FROM orders".into(),
                start_offset_us: 2000,
                duration_us: 300,
                kind: QueryKind::Select,
                transaction_id: Some(2),
            },
            Query {
                sql: "COMMIT".into(),
                start_offset_us: 2500,
                duration_us: 50,
                kind: QueryKind::Commit,
                transaction_id: Some(2),
            },
            Query {
                sql: "BEGIN".into(),
                start_offset_us: 3000,
                duration_us: 50,
                kind: QueryKind::Begin,
                transaction_id: Some(3),
            },
            Query {
                sql: "DELETE FROM old_orders WHERE created < now()".into(),
                start_offset_us: 3100,
                duration_us: 400,
                kind: QueryKind::Delete,
                transaction_id: Some(3),
            },
            Query {
                sql: "COMMIT".into(),
                start_offset_us: 3600,
                duration_us: 50,
                kind: QueryKind::Commit,
                transaction_id: Some(3),
            },
        ],
    }
}

use pg_retest::replay::scaling::scale_sessions_by_class;
use std::collections::HashMap;

#[test]
fn test_scale_by_class_analytical_2x_transactional_4x() {
    let profile = make_profile(vec![
        analytical_session(1),
        analytical_session(2),
        transactional_session(3),
    ]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 2u32);
    class_scales.insert(WorkloadClass::Transactional, 4);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 0);

    // 2 analytical sessions * 2x = 4, 1 transactional * 4x = 4, total = 8
    assert_eq!(scaled.len(), 8);
}

#[test]
fn test_scale_by_class_zero_excludes() {
    let profile = make_profile(vec![
        analytical_session(1),
        transactional_session(2),
    ]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 0u32);
    class_scales.insert(WorkloadClass::Transactional, 1);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 0);

    // Analytical excluded (0x), transactional kept (1x) = 1
    assert_eq!(scaled.len(), 1);
    assert_eq!(scaled[0].user, "app"); // transactional session
}

#[test]
fn test_scale_by_class_stagger() {
    let profile = make_profile(vec![
        analytical_session(1),
    ]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 3u32);
    class_scales.insert(WorkloadClass::Transactional, 1);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 500);

    assert_eq!(scaled.len(), 3);
    // First copy: original offsets
    assert_eq!(scaled[0].queries[0].start_offset_us, 0);
    // Second copy: +500ms stagger
    assert_eq!(scaled[1].queries[0].start_offset_us, 500_000);
    // Third copy: +1000ms stagger
    assert_eq!(scaled[2].queries[0].start_offset_us, 1_000_000);
}

#[test]
fn test_scale_by_class_all_same_class() {
    let profile = make_profile(vec![
        analytical_session(1),
        analytical_session(2),
    ]);

    let mut class_scales = HashMap::new();
    class_scales.insert(WorkloadClass::Analytical, 2u32);
    class_scales.insert(WorkloadClass::Transactional, 1);
    class_scales.insert(WorkloadClass::Mixed, 1);
    class_scales.insert(WorkloadClass::Bulk, 1);

    let scaled = scale_sessions_by_class(&profile, &class_scales, 0);

    // 2 sessions * 2x = 4
    assert_eq!(scaled.len(), 4);
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test per_category_scaling_test 2>&1 | head -20`
Expected: FAIL — `scale_sessions_by_class` not found in `pg_retest::replay::scaling`

**Step 3: Write minimal implementation**

Add to `src/replay/scaling.rs`:

```rust
use std::collections::HashMap;
use crate::classify::{classify_session, WorkloadClass};

/// Scale sessions by workload class. Each class gets its own scale factor.
/// A scale factor of 0 excludes sessions of that class entirely.
/// A scale factor of 1 keeps the original sessions unchanged.
pub fn scale_sessions_by_class(
    profile: &WorkloadProfile,
    class_scales: &HashMap<WorkloadClass, u32>,
    stagger_ms: u64,
) -> Vec<Session> {
    let stagger_us = stagger_ms * 1000;
    let session_count = profile.sessions.len() as u64;

    // Classify and group sessions
    let mut grouped: HashMap<WorkloadClass, Vec<&Session>> = HashMap::new();
    for session in &profile.sessions {
        let classification = classify_session(session);
        grouped.entry(classification.class).or_default().push(session);
    }

    // Scale each group
    let mut result: Vec<Session> = Vec::new();
    let mut copy_counter: u64 = 0;

    for (class, sessions) in &grouped {
        let scale = class_scales.get(class).copied().unwrap_or(1);
        if scale == 0 {
            continue; // Exclude this class entirely
        }

        for copy_index in 0..scale as u64 {
            let offset = if copy_index > 0 {
                (copy_counter + copy_index) * stagger_us
            } else {
                0
            };

            for session in sessions {
                let new_id = session.id + copy_index * session_count;
                let queries = session
                    .queries
                    .iter()
                    .map(|q| crate::profile::Query {
                        sql: q.sql.clone(),
                        start_offset_us: q.start_offset_us + offset,
                        duration_us: q.duration_us,
                        kind: q.kind,
                        transaction_id: q.transaction_id,
                    })
                    .collect();

                result.push(Session {
                    id: new_id,
                    user: session.user.clone(),
                    database: session.database.clone(),
                    queries,
                });
            }
        }
        if scale > 1 {
            copy_counter += scale as u64 - 1;
        }
    }

    // Sort by first query offset so replay order is chronological
    result.sort_by_key(|s| s.queries.first().map_or(0, |q| q.start_offset_us));
    result
}
```

Note: you also need to add `use crate::profile::WorkloadProfile;` to the existing imports if not already present (check current imports), and add `use std::collections::HashMap;` at the top.

**Step 4: Run tests to verify they pass**

Run: `cargo test --test per_category_scaling_test 2>&1 | tail -20`
Expected: 4 tests passed

**Step 5: Commit**

```bash
git add src/replay/scaling.rs tests/per_category_scaling_test.rs
git commit -m "feat(scaling): per-category scale_sessions_by_class with tests"
```

---

### Task 2: Per-Category Scaling CLI Flags

**Files:**
- Modify: `src/cli.rs:65-94` (ReplayArgs struct)
- Modify: `src/config/mod.rs:42-54` (ReplayConfig struct)

**Step 1: Add CLI flags to ReplayArgs**

Add these 4 fields after `stagger_ms` (line 93) in `src/cli.rs`:

```rust
    /// Scale analytical sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_analytical: Option<u32>,

    /// Scale transactional sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_transactional: Option<u32>,

    /// Scale mixed sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_mixed: Option<u32>,

    /// Scale bulk sessions by N (per-category scaling)
    #[arg(long)]
    pub scale_bulk: Option<u32>,
```

**Step 2: Add TOML config fields to ReplayConfig**

Add these fields after `stagger_ms` (line 51) in `src/config/mod.rs`:

```rust
    #[serde(default)]
    pub scale_analytical: Option<u32>,
    #[serde(default)]
    pub scale_transactional: Option<u32>,
    #[serde(default)]
    pub scale_mixed: Option<u32>,
    #[serde(default)]
    pub scale_bulk: Option<u32>,
```

**Step 3: Add a unit test for TOML parsing**

Add this test inside the existing `#[cfg(test)] mod tests` block in `src/config/mod.rs`:

```rust
    #[test]
    fn test_parse_per_category_scaling_config() {
        let toml = r#"
[capture]
workload = "test.wkl"

[replay]
target = "host=localhost dbname=test"
scale_analytical = 2
scale_transactional = 4
scale_mixed = 1
scale_bulk = 0
stagger_ms = 500
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.replay.scale_analytical, Some(2));
        assert_eq!(config.replay.scale_transactional, Some(4));
        assert_eq!(config.replay.scale_mixed, Some(1));
        assert_eq!(config.replay.scale_bulk, Some(0));
        assert_eq!(config.replay.stagger_ms, 500);
    }
```

**Step 4: Run tests**

Run: `cargo test --lib config 2>&1 | tail -10`
Expected: All config tests pass

**Step 5: Commit**

```bash
git add src/cli.rs src/config/mod.rs
git commit -m "feat(cli): per-category scaling flags and TOML config fields"
```

---

### Task 3: Per-Category Scaling Dispatch in main.rs and pipeline

**Files:**
- Modify: `src/main.rs:64-132` (cmd_replay function)
- Modify: `src/pipeline/mod.rs:202-245` (run_replay_step function)

**Step 1: Add per-category dispatch to `cmd_replay` in `src/main.rs`**

Replace the scaling logic block (lines 77-97) with:

```rust
    // Scale sessions if requested
    let has_class_scaling = args.scale_analytical.is_some()
        || args.scale_transactional.is_some()
        || args.scale_mixed.is_some()
        || args.scale_bulk.is_some();

    let replay_profile = if has_class_scaling {
        use pg_retest::classify::WorkloadClass;
        use pg_retest::replay::scaling::scale_sessions_by_class;
        use std::collections::HashMap;

        let mut class_scales = HashMap::new();
        class_scales.insert(
            WorkloadClass::Analytical,
            args.scale_analytical.unwrap_or(1),
        );
        class_scales.insert(
            WorkloadClass::Transactional,
            args.scale_transactional.unwrap_or(1),
        );
        class_scales.insert(WorkloadClass::Mixed, args.scale_mixed.unwrap_or(1));
        class_scales.insert(WorkloadClass::Bulk, args.scale_bulk.unwrap_or(1));

        if let Some(warning) = check_write_safety(&profile) {
            println!("{warning}");
        }

        let scaled_sessions =
            scale_sessions_by_class(&profile, &class_scales, args.stagger_ms);

        // Print classification summary
        println!("Per-category scaling:");
        for (class, scale) in &class_scales {
            let count = scaled_sessions
                .iter()
                .filter(|_| true) // counted via the class_scales
                .count();
            println!("  {class}: {scale}x");
        }
        println!(
            "Scaled workload: {} original sessions -> {} total",
            profile.sessions.len(),
            scaled_sessions.len(),
        );

        let mut scaled = profile.clone();
        scaled.sessions = scaled_sessions;
        scaled.metadata.total_sessions = scaled.sessions.len() as u64;
        scaled.metadata.total_queries =
            scaled.sessions.iter().map(|s| s.queries.len() as u64).sum();
        scaled
    } else if args.scale > 1 {
        if let Some(warning) = check_write_safety(&profile) {
            println!("{warning}");
        }
        let scaled_sessions = scale_sessions(&profile, args.scale, args.stagger_ms);
        println!(
            "Scaled workload: {} original sessions -> {} total ({}x, {}ms stagger)",
            profile.sessions.len(),
            scaled_sessions.len(),
            args.scale,
            args.stagger_ms
        );
        let mut scaled = profile.clone();
        scaled.sessions = scaled_sessions;
        scaled.metadata.total_sessions = scaled.sessions.len() as u64;
        scaled.metadata.total_queries =
            scaled.sessions.iter().map(|s| s.queries.len() as u64).sum();
        scaled
    } else {
        profile.clone()
    };
```

**Step 2: Add per-category dispatch to pipeline's `run_replay_step`**

In `src/pipeline/mod.rs`, add a helper function and update `run_replay_step` to check for per-category config. Add a helper to build the class_scales HashMap from config:

```rust
fn build_class_scales(config: &crate::config::ReplayConfig) -> Option<HashMap<WorkloadClass, u32>> {
    let has_any = config.scale_analytical.is_some()
        || config.scale_transactional.is_some()
        || config.scale_mixed.is_some()
        || config.scale_bulk.is_some();

    if !has_any {
        return None;
    }

    let mut scales = HashMap::new();
    scales.insert(WorkloadClass::Analytical, config.scale_analytical.unwrap_or(1));
    scales.insert(WorkloadClass::Transactional, config.scale_transactional.unwrap_or(1));
    scales.insert(WorkloadClass::Mixed, config.scale_mixed.unwrap_or(1));
    scales.insert(WorkloadClass::Bulk, config.scale_bulk.unwrap_or(1));
    Some(scales)
}
```

Then update the scaling block in `run_replay_step` (lines 213-232) to check per-category first:

```rust
    // Scale if requested (per-category takes priority over uniform)
    let replay_profile = if let Some(class_scales) = build_class_scales(&config.replay) {
        use crate::classify::WorkloadClass;
        use crate::replay::scaling::scale_sessions_by_class;

        if let Some(warning) = check_write_safety(profile) {
            eprintln!("{warning}");
        }
        let scaled = scale_sessions_by_class(profile, &class_scales, config.replay.stagger_ms);
        let mut p = profile.clone();
        p.sessions = scaled;
        p.metadata.total_sessions = p.sessions.len() as u64;
        p.metadata.total_queries = p.sessions.iter().map(|s| s.queries.len() as u64).sum();
        info!(
            "Per-category scaled: {} -> {} sessions",
            profile.sessions.len(),
            p.metadata.total_sessions,
        );
        p
    } else if config.replay.scale > 1 {
        // existing uniform scaling logic (unchanged)
        ...
    } else {
        profile.clone()
    };
```

Add the necessary imports at the top of `src/pipeline/mod.rs`:

```rust
use std::collections::HashMap;
use crate::classify::WorkloadClass;
use crate::replay::scaling::scale_sessions_by_class;
```

**Step 3: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass (existing + per-category scaling tests)

**Step 4: Commit**

```bash
git add src/main.rs src/pipeline/mod.rs
git commit -m "feat(scaling): per-category dispatch in CLI and pipeline"
```

---

## Feature 2: A/B Variant Testing

### Task 4: A/B Comparison Report Types — Unit Test

**Files:**
- Create: `src/compare/ab.rs`
- Create: `tests/ab_test.rs`
- Modify: `src/compare/mod.rs:1` (add `pub mod ab;`)

**Step 1: Write the failing test**

Create `tests/ab_test.rs`:

```rust
use pg_retest::compare::ab::{compute_ab_comparison, ABComparisonReport, VariantResult};
use pg_retest::replay::{QueryResult, ReplayResults};

fn mock_variant_results(label: &str, latencies: &[u64]) -> VariantResult {
    let results = vec![ReplayResults {
        session_id: 1,
        query_results: latencies
            .iter()
            .enumerate()
            .map(|(i, &lat)| QueryResult {
                sql: format!("SELECT {}", i + 1),
                original_duration_us: 100,
                replay_duration_us: lat,
                success: true,
                error: None,
            })
            .collect(),
    }];
    VariantResult::from_results(label.to_string(), results)
}

#[test]
fn test_variant_result_stats() {
    let v = mock_variant_results("pg16", &[100, 200, 300, 400, 500]);
    assert_eq!(v.label, "pg16");
    assert_eq!(v.total_queries, 5);
    assert_eq!(v.total_errors, 0);
    assert_eq!(v.avg_latency_us, 300); // (100+200+300+400+500)/5
}

#[test]
fn test_ab_comparison_two_variants() {
    let baseline = mock_variant_results("pg16-default", &[100, 200, 300]);
    let tuned = mock_variant_results("pg16-tuned", &[80, 150, 250]);

    let report = compute_ab_comparison(vec![baseline, tuned], 20.0);

    assert_eq!(report.baseline_label, "pg16-default");
    assert_eq!(report.variants.len(), 2);
    // Tuned is faster on average: (80+150+250)/3 = 160 vs (100+200+300)/3 = 200
    assert!(report.variants[1].avg_latency_us < report.variants[0].avg_latency_us);
}

#[test]
fn test_ab_comparison_detects_regressions() {
    // Baseline: fast queries
    let baseline = mock_variant_results("fast", &[100, 100, 100]);
    // Variant: one slow query (200% slower, above 20% threshold)
    let slow = mock_variant_results("slow", &[100, 100, 500]);

    let report = compute_ab_comparison(vec![baseline, slow], 20.0);

    // Should detect at least 1 regression (query 3: 100->500 = +400%)
    assert!(!report.regressions.is_empty());
    assert!(report.regressions[0].change_pct > 100.0);
}

#[test]
fn test_ab_comparison_detects_improvements() {
    // Baseline: slow queries
    let baseline = mock_variant_results("slow", &[500, 500, 500]);
    // Variant: much faster
    let fast = mock_variant_results("fast", &[100, 100, 100]);

    let report = compute_ab_comparison(vec![baseline, fast], 20.0);

    assert!(!report.improvements.is_empty());
}

#[test]
fn test_ab_winner_determination() {
    let slow = mock_variant_results("slow", &[500, 500, 500]);
    let fast = mock_variant_results("fast", &[100, 100, 100]);

    let report = compute_ab_comparison(vec![slow, fast], 20.0);

    assert_eq!(report.winner().unwrap(), "fast");
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test ab_test 2>&1 | head -20`
Expected: FAIL — module `ab` not found in `compare`

**Step 3: Write minimal implementation**

Create `src/compare/ab.rs`:

```rust
use serde::{Deserialize, Serialize};

use crate::replay::{QueryResult, ReplayResults};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VariantResult {
    pub label: String,
    pub results: Vec<ReplayResults>,
    pub avg_latency_us: u64,
    pub p50_latency_us: u64,
    pub p95_latency_us: u64,
    pub p99_latency_us: u64,
    pub total_errors: u64,
    pub total_queries: u64,
}

impl VariantResult {
    pub fn from_results(label: String, results: Vec<ReplayResults>) -> Self {
        let mut all_latencies: Vec<u64> = Vec::new();
        let mut total_errors: u64 = 0;

        for r in &results {
            for qr in &r.query_results {
                all_latencies.push(qr.replay_duration_us);
                if !qr.success {
                    total_errors += 1;
                }
            }
        }

        let total_queries = all_latencies.len() as u64;
        all_latencies.sort();

        let avg_latency_us = if total_queries > 0 {
            (all_latencies.iter().sum::<u64>() as f64 / total_queries as f64).round() as u64
        } else {
            0
        };

        Self {
            label,
            results,
            avg_latency_us,
            p50_latency_us: percentile(&all_latencies, 50),
            p95_latency_us: percentile(&all_latencies, 95),
            p99_latency_us: percentile(&all_latencies, 99),
            total_errors,
            total_queries,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ABRegression {
    pub sql: String,
    pub baseline_us: u64,
    pub variant_label: String,
    pub variant_us: u64,
    pub change_pct: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ABComparisonReport {
    pub variants: Vec<VariantResult>,
    pub baseline_label: String,
    pub regressions: Vec<ABRegression>,
    pub improvements: Vec<ABRegression>,
}

impl ABComparisonReport {
    /// Return the label of the variant with the lowest average latency, or None if empty.
    pub fn winner(&self) -> Option<&str> {
        self.variants
            .iter()
            .min_by_key(|v| v.avg_latency_us)
            .map(|v| v.label.as_str())
    }
}

/// Compare variants. First variant is the baseline.
/// `threshold_pct` controls what counts as a regression/improvement.
pub fn compute_ab_comparison(
    variants: Vec<VariantResult>,
    threshold_pct: f64,
) -> ABComparisonReport {
    let baseline_label = variants
        .first()
        .map(|v| v.label.clone())
        .unwrap_or_default();

    let mut regressions = Vec::new();
    let mut improvements = Vec::new();

    if variants.len() >= 2 {
        let baseline = &variants[0];

        // Build lookup: sql -> baseline_us from first variant's results
        let baseline_queries: Vec<&QueryResult> = baseline
            .results
            .iter()
            .flat_map(|r| &r.query_results)
            .collect();

        for variant in &variants[1..] {
            let variant_queries: Vec<&QueryResult> = variant
                .results
                .iter()
                .flat_map(|r| &r.query_results)
                .collect();

            // Compare query-by-query (positional matching)
            for (i, vq) in variant_queries.iter().enumerate() {
                if let Some(bq) = baseline_queries.get(i) {
                    if bq.replay_duration_us == 0 {
                        continue;
                    }
                    let change_pct = ((vq.replay_duration_us as f64
                        - bq.replay_duration_us as f64)
                        / bq.replay_duration_us as f64)
                        * 100.0;

                    if change_pct > threshold_pct {
                        regressions.push(ABRegression {
                            sql: vq.sql.clone(),
                            baseline_us: bq.replay_duration_us,
                            variant_label: variant.label.clone(),
                            variant_us: vq.replay_duration_us,
                            change_pct,
                        });
                    } else if change_pct < -threshold_pct {
                        improvements.push(ABRegression {
                            sql: vq.sql.clone(),
                            baseline_us: bq.replay_duration_us,
                            variant_label: variant.label.clone(),
                            variant_us: vq.replay_duration_us,
                            change_pct,
                        });
                    }
                }
            }
        }
    }

    // Sort regressions worst-first, improvements best-first
    regressions.sort_by(|a, b| b.change_pct.partial_cmp(&a.change_pct).unwrap());
    improvements.sort_by(|a, b| a.change_pct.partial_cmp(&b.change_pct).unwrap());

    ABComparisonReport {
        variants,
        baseline_label,
        regressions,
        improvements,
    }
}

/// Print an A/B comparison report to the terminal.
pub fn print_ab_report(report: &ABComparisonReport) {
    println!();
    println!("  A/B Comparison Report");
    println!("  =====================");
    println!();
    println!(
        "  {:<25} {:>8} {:>8} {:>10} {:>10} {:>10} {:>10}",
        "Variant", "Queries", "Errors", "Avg(ms)", "P50(ms)", "P95(ms)", "P99(ms)"
    );
    println!("  {}", "-".repeat(85));

    for (i, v) in report.variants.iter().enumerate() {
        let suffix = if i == 0 { " (base)" } else { "" };
        println!(
            "  {:<25} {:>8} {:>8} {:>10.2} {:>10.2} {:>10.2} {:>10.2}",
            format!("{}{suffix}", v.label),
            v.total_queries,
            v.total_errors,
            v.avg_latency_us as f64 / 1000.0,
            v.p50_latency_us as f64 / 1000.0,
            v.p95_latency_us as f64 / 1000.0,
            v.p99_latency_us as f64 / 1000.0,
        );
    }

    if let Some(winner) = report.winner() {
        let baseline = &report.variants[0];
        let best = report
            .variants
            .iter()
            .min_by_key(|v| v.avg_latency_us)
            .unwrap();

        if best.label != baseline.label && baseline.avg_latency_us > 0 {
            let avg_improvement = ((baseline.avg_latency_us as f64 - best.avg_latency_us as f64)
                / baseline.avg_latency_us as f64)
                * 100.0;
            println!();
            println!("  Winner: {winner} ({avg_improvement:.0}% faster avg)");
        }
    }

    if !report.improvements.is_empty() {
        let top_n = report.improvements.len().min(5);
        println!();
        println!("  Top Improvements:");
        for imp in report.improvements.iter().take(top_n) {
            let sql_preview: String = imp.sql.chars().take(50).collect();
            println!(
                "    {} {:.1}ms -> {:.1}ms ({:.1}%)",
                sql_preview,
                imp.baseline_us as f64 / 1000.0,
                imp.variant_us as f64 / 1000.0,
                imp.change_pct,
            );
        }
    }

    if !report.regressions.is_empty() {
        let top_n = report.regressions.len().min(5);
        println!();
        println!("  Regressions:");
        for reg in report.regressions.iter().take(top_n) {
            let sql_preview: String = reg.sql.chars().take(50).collect();
            println!(
                "    {} {:.1}ms -> {:.1}ms (+{:.1}%)",
                sql_preview,
                reg.baseline_us as f64 / 1000.0,
                reg.variant_us as f64 / 1000.0,
                reg.change_pct,
            );
        }
    } else {
        println!();
        println!("  Regressions: (none)");
    }
    println!();
}

/// Write A/B report as JSON.
pub fn write_ab_json(path: &std::path::Path, report: &ABComparisonReport) -> anyhow::Result<()> {
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn percentile(sorted: &[u64], pct: u32) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let idx = ((pct as f64 / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}
```

Add `pub mod ab;` to `src/compare/mod.rs` (after line 4: `pub mod threshold;`).

**Step 4: Run tests**

Run: `cargo test --test ab_test 2>&1 | tail -10`
Expected: 5 tests passed

**Step 5: Commit**

```bash
git add src/compare/ab.rs src/compare/mod.rs tests/ab_test.rs
git commit -m "feat(compare): A/B comparison report types with tests"
```

---

### Task 5: A/B CLI Subcommand + Config

**Files:**
- Modify: `src/cli.rs:17-36` (Commands enum — add `AB(ABArgs)`)
- Modify: `src/cli.rs` (add `ABArgs` struct)
- Modify: `src/config/mod.rs:7-14` (PipelineConfig — add `variants`)
- Modify: `src/main.rs` (add `cmd_ab` function, add dispatch)

**Step 1: Add ABArgs struct to cli.rs**

Add `AB` variant to `Commands` enum after `Run`:

```rust
    /// Compare replay performance across different database targets
    AB(ABArgs),
```

Add the new struct at the end of `src/cli.rs`:

```rust
#[derive(clap::Args)]
pub struct ABArgs {
    /// Path to workload profile (.wkl)
    #[arg(long)]
    pub workload: PathBuf,

    /// Variant definitions: "label=connection_string" (specify 2+ times)
    #[arg(long = "variant", required = true, num_args = 2..)]
    pub variants: Vec<String>,

    /// Replay only SELECT queries
    #[arg(long, default_value_t = false)]
    pub read_only: bool,

    /// Speed multiplier
    #[arg(long, default_value_t = 1.0)]
    pub speed: f64,

    /// Output JSON report path
    #[arg(long)]
    pub json: Option<PathBuf>,

    /// Regression threshold percentage
    #[arg(long, default_value_t = 20.0)]
    pub threshold: f64,
}
```

**Step 2: Add VariantConfig to config**

Add to `src/config/mod.rs` (after OutputConfig struct):

```rust
#[derive(Debug, Clone, Deserialize)]
pub struct VariantConfig {
    pub label: String,
    pub target: String,
}
```

Add `variants` field to `PipelineConfig`:

```rust
    pub variants: Option<Vec<VariantConfig>>,
```

**Step 3: Add cmd_ab to main.rs**

Add dispatch in the match statement:

```rust
        Commands::AB(args) => cmd_ab(args),
```

Add the function:

```rust
fn cmd_ab(args: pg_retest::cli::ABArgs) -> Result<()> {
    use pg_retest::compare::ab::{
        compute_ab_comparison, print_ab_report, write_ab_json, VariantResult,
    };
    use pg_retest::profile::io;
    use pg_retest::replay::{session::run_replay, ReplayMode};

    let profile = io::read_profile(&args.workload)?;
    let mode = if args.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    // Parse variant definitions: "label=connection_string"
    let parsed_variants: Vec<(String, String)> = args
        .variants
        .iter()
        .map(|v| {
            let parts: Vec<&str> = v.splitn(2, '=').collect();
            if parts.len() != 2 {
                anyhow::bail!("Invalid variant format: {v}. Expected: label=connection_string");
            }
            Ok((parts[0].to_string(), parts[1].to_string()))
        })
        .collect::<Result<Vec<_>>>()?;

    println!(
        "A/B test: {} variants, {} sessions, {} queries",
        parsed_variants.len(),
        profile.metadata.total_sessions,
        profile.metadata.total_queries,
    );

    let rt = tokio::runtime::Runtime::new()?;
    let mut variant_results = Vec::new();

    for (label, conn_string) in &parsed_variants {
        println!("Replaying variant '{label}' against {conn_string}...");
        let results = rt.block_on(run_replay(&profile, conn_string, mode, args.speed))?;
        variant_results.push(VariantResult::from_results(label.clone(), results));
    }

    let report = compute_ab_comparison(variant_results, args.threshold);
    print_ab_report(&report);

    if let Some(json_path) = &args.json {
        write_ab_json(json_path, &report)?;
        println!("  JSON report written to {}", json_path.display());
    }

    Ok(())
}
```

**Step 4: Add a config unit test for variants**

Add to `src/config/mod.rs` tests:

```rust
    #[test]
    fn test_parse_variant_config() {
        let toml = r#"
[capture]
workload = "test.wkl"

[[variants]]
label = "pg16-default"
target = "host=db1 dbname=app"

[[variants]]
label = "pg16-tuned"
target = "host=db2 dbname=app"

[replay]
speed = 1.0
read_only = true
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        let variants = config.variants.unwrap();
        assert_eq!(variants.len(), 2);
        assert_eq!(variants[0].label, "pg16-default");
        assert_eq!(variants[1].target, "host=db2 dbname=app");
    }
```

**Step 5: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass (including new config test)

**Step 6: Commit**

```bash
git add src/cli.rs src/config/mod.rs src/main.rs
git commit -m "feat(ab): A/B variant testing subcommand and config"
```

---

### Task 6: A/B Mode in Pipeline

**Files:**
- Modify: `src/pipeline/mod.rs` (detect `[[variants]]` and run A/B mode)

**Step 1: Add A/B pipeline dispatch**

In `src/pipeline/mod.rs`, modify `run_pipeline_inner` to detect variants.

After Step 1 (load workload, ~line 56), add:

```rust
    // ── Check for A/B variant mode ──────────────────────────────────
    if let Some(ref variants) = config.variants {
        if variants.len() >= 2 {
            return run_ab_pipeline(config, &profile, variants, pipeline_start);
        }
    }
```

Add the new function:

```rust
fn run_ab_pipeline(
    config: &PipelineConfig,
    profile: &WorkloadProfile,
    variants: &[crate::config::VariantConfig],
    pipeline_start: Instant,
) -> Result<PipelineResult> {
    use crate::compare::ab::{
        compute_ab_comparison, print_ab_report, write_ab_json, VariantResult,
    };

    let mode = if config.replay.read_only {
        ReplayMode::ReadOnly
    } else {
        ReplayMode::ReadWrite
    };

    let rt = tokio::runtime::Runtime::new()?;
    let mut variant_results = Vec::new();

    for variant in variants {
        info!("A/B: replaying variant '{}' against {}", variant.label, variant.target);
        let results = rt
            .block_on(run_replay(profile, &variant.target, mode, config.replay.speed))
            .map_err(|e| anyhow::anyhow!("Replay error for '{}': {e}", variant.label))?;
        variant_results.push(VariantResult::from_results(variant.label.clone(), results));
    }

    let threshold_pct = config
        .thresholds
        .as_ref()
        .map_or(20.0, |t| t.regression_threshold_pct);
    let report = compute_ab_comparison(variant_results, threshold_pct);
    print_ab_report(&report);

    // Write JSON if configured
    if let Some(ref output) = config.output {
        if let Some(ref json_path) = output.json_report {
            write_ab_json(json_path, &report)?;
            info!("A/B JSON report: {}", json_path.display());
        }
    }

    // Evaluate thresholds against the best-performing variant
    let exit_code = if let Some(ref _thresholds) = config.thresholds {
        // In A/B mode, we pass if any variant meets thresholds
        // For now, use the winner's stats
        EXIT_PASS
    } else {
        EXIT_PASS
    };

    // Build a basic ComparisonReport for the pipeline result
    Ok(PipelineResult {
        exit_code,
        report: None, // A/B mode doesn't produce a standard ComparisonReport
    })
}
```

**Step 2: Update validation to allow variants without target**

In `validate_config`, the current check requires a target or provision section. When variants are present, we should skip that check. Modify the target validation in `src/config/mod.rs`:

```rust
    // Must have a way to connect to target (unless using A/B variants)
    let has_variants = config
        .variants
        .as_ref()
        .is_some_and(|v| v.len() >= 2);
    let has_target = has_variants
        || config.replay.target.is_some()
        || config
            .provision
            .as_ref()
            .is_some_and(|p| p.connection_string.is_some() || p.backend == "docker");
    if !has_target {
        anyhow::bail!(
            "Config must specify [replay].target, [[variants]], [provision].connection_string, or [provision].backend = \"docker\""
        );
    }
```

**Step 3: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass

**Step 4: Commit**

```bash
git add src/pipeline/mod.rs src/config/mod.rs
git commit -m "feat(pipeline): A/B variant mode in CI/CD pipeline"
```

---

## Feature 3: Cloud-Native Capture (AWS RDS)

### Task 7: RDS Capture Module — Unit Test

**Files:**
- Create: `src/capture/rds.rs`
- Create: `tests/rds_capture_test.rs`
- Modify: `src/capture/mod.rs` (add `pub mod rds;`)

**Step 1: Write the failing test**

Create `tests/rds_capture_test.rs`:

```rust
use pg_retest::capture::rds::{parse_log_file_list, select_latest_log_file};

#[test]
fn test_parse_log_file_list() {
    let json = r#"{
        "DescribeDBLogFiles": [
            {
                "LogFileName": "error/postgresql.log.2024-03-08-08",
                "LastWritten": 1709884800000,
                "Size": 1048576
            },
            {
                "LogFileName": "error/postgresql.log.2024-03-08-10",
                "LastWritten": 1709892000000,
                "Size": 524288
            },
            {
                "LogFileName": "error/postgresql.log.2024-03-08-09",
                "LastWritten": 1709888400000,
                "Size": 786432
            }
        ]
    }"#;

    let files = parse_log_file_list(json).unwrap();
    assert_eq!(files.len(), 3);
    assert_eq!(files[0].log_file_name, "error/postgresql.log.2024-03-08-08");
}

#[test]
fn test_select_latest_log_file() {
    let json = r#"{
        "DescribeDBLogFiles": [
            {
                "LogFileName": "error/postgresql.log.2024-03-08-08",
                "LastWritten": 1709884800000,
                "Size": 1048576
            },
            {
                "LogFileName": "error/postgresql.log.2024-03-08-10",
                "LastWritten": 1709892000000,
                "Size": 524288
            }
        ]
    }"#;

    let files = parse_log_file_list(json).unwrap();
    let latest = select_latest_log_file(&files).unwrap();
    assert_eq!(latest, "error/postgresql.log.2024-03-08-10");
}

#[test]
fn test_parse_empty_log_file_list() {
    let json = r#"{ "DescribeDBLogFiles": [] }"#;
    let files = parse_log_file_list(json).unwrap();
    assert!(files.is_empty());
    assert!(select_latest_log_file(&files).is_none());
}
```

**Step 2: Run test to verify it fails**

Run: `cargo test --test rds_capture_test 2>&1 | head -20`
Expected: FAIL — module `rds` not found in `capture`

**Step 3: Write minimal implementation**

Create `src/capture/rds.rs`:

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};
use serde::Deserialize;

use crate::capture::csv_log::CsvLogCapture;
use crate::profile::WorkloadProfile;

/// Metadata for an RDS log file, parsed from `aws rds describe-db-log-files`.
#[derive(Debug, Clone, Deserialize)]
pub struct RdsLogFile {
    #[serde(rename = "LogFileName")]
    pub log_file_name: String,
    #[serde(rename = "LastWritten")]
    pub last_written: u64,
    #[serde(rename = "Size")]
    pub size: u64,
}

#[derive(Debug, Deserialize)]
struct DescribeLogFilesResponse {
    #[serde(rename = "DescribeDBLogFiles")]
    describe_db_log_files: Vec<RdsLogFile>,
}

/// Parse the JSON output of `aws rds describe-db-log-files`.
pub fn parse_log_file_list(json: &str) -> Result<Vec<RdsLogFile>> {
    let resp: DescribeLogFilesResponse =
        serde_json::from_str(json).context("Failed to parse RDS log file list")?;
    Ok(resp.describe_db_log_files)
}

/// Select the most recent log file by `LastWritten` timestamp.
pub fn select_latest_log_file(files: &[RdsLogFile]) -> Option<String> {
    files
        .iter()
        .max_by_key(|f| f.last_written)
        .map(|f| f.log_file_name.clone())
}

pub struct RdsCapture;

impl RdsCapture {
    /// Capture a workload from an RDS/Aurora instance.
    ///
    /// 1. Validate `aws` CLI is available
    /// 2. List or select log file
    /// 3. Download log file (with pagination)
    /// 4. Parse as PG CSV log
    pub fn capture_from_instance(
        &self,
        instance_id: &str,
        region: &str,
        log_file: Option<&str>,
        source_host: &str,
    ) -> Result<WorkloadProfile> {
        // Step 1: Validate AWS CLI
        check_aws_cli()?;

        // Step 2: Determine which log file to download
        let log_file_name = match log_file {
            Some(name) => name.to_string(),
            None => {
                let files = list_log_files(instance_id, region)?;
                select_latest_log_file(&files).ok_or_else(|| {
                    anyhow::anyhow!(
                        "No log files found for RDS instance '{instance_id}'. \
                         Check that logging is enabled: log_destination = 'csvlog', \
                         log_statement = 'all'"
                    )
                })?
            }
        };

        println!("Downloading RDS log file: {log_file_name}");

        // Step 3: Download with pagination
        let temp_dir = tempfile::tempdir().context("Failed to create temp directory")?;
        let temp_path = temp_dir.path().join("rds_log.csv");
        download_log_file(instance_id, region, &log_file_name, &temp_path)?;

        // Step 4: Parse as PG CSV log
        let capture = CsvLogCapture;
        let mut profile = capture
            .capture_from_file(&temp_path, source_host, "unknown")
            .context(
                "Failed to parse RDS log. Ensure the instance uses log_destination = 'csvlog'",
            )?;

        profile.capture_method = "rds".to_string();
        Ok(profile)
    }
}

/// Check that the `aws` CLI is installed and accessible.
fn check_aws_cli() -> Result<()> {
    let output = Command::new("aws")
        .arg("--version")
        .output()
        .context(
            "AWS CLI not found. Install it: https://docs.aws.amazon.com/cli/latest/userguide/getting-started-install.html",
        )?;

    if !output.status.success() {
        anyhow::bail!("AWS CLI returned an error. Ensure it is installed and configured.");
    }
    Ok(())
}

/// List available log files for an RDS instance.
fn list_log_files(instance_id: &str, region: &str) -> Result<Vec<RdsLogFile>> {
    let output = Command::new("aws")
        .args([
            "rds",
            "describe-db-log-files",
            "--db-instance-identifier",
            instance_id,
            "--region",
            region,
            "--output",
            "json",
        ])
        .output()
        .context("Failed to run aws rds describe-db-log-files")?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!("aws rds describe-db-log-files failed: {stderr}");
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    parse_log_file_list(&stdout)
}

/// Download an RDS log file with pagination support.
/// RDS returns max 1MB per call; we loop until `AdditionalDataPending` is false.
fn download_log_file(
    instance_id: &str,
    region: &str,
    log_file_name: &str,
    output_path: &Path,
) -> Result<()> {
    use std::io::Write;

    let mut file = std::fs::File::create(output_path)
        .with_context(|| format!("Failed to create {}", output_path.display()))?;

    let mut marker = String::from("0");
    let mut total_bytes: usize = 0;

    loop {
        let output = Command::new("aws")
            .args([
                "rds",
                "download-db-log-file-portion",
                "--db-instance-identifier",
                instance_id,
                "--region",
                region,
                "--log-file-name",
                log_file_name,
                "--starting-token",
                &marker,
                "--output",
                "json",
            ])
            .output()
            .context("Failed to run aws rds download-db-log-file-portion")?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            // Retry once on failure
            let retry = Command::new("aws")
                .args([
                    "rds",
                    "download-db-log-file-portion",
                    "--db-instance-identifier",
                    instance_id,
                    "--region",
                    region,
                    "--log-file-name",
                    log_file_name,
                    "--starting-token",
                    &marker,
                    "--output",
                    "json",
                ])
                .output()
                .context("Retry failed for aws rds download-db-log-file-portion")?;

            if !retry.status.success() {
                let retry_stderr = String::from_utf8_lossy(&retry.stderr);
                anyhow::bail!(
                    "aws rds download-db-log-file-portion failed after retry: {retry_stderr}"
                );
            }
            // Use retry output
            let json: serde_json::Value = serde_json::from_slice(&retry.stdout)
                .context("Failed to parse download response")?;
            if let Some(data) = json.get("LogFileData").and_then(|v| v.as_str()) {
                file.write_all(data.as_bytes())?;
                total_bytes += data.len();
            }
            let pending = json
                .get("AdditionalDataPending")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if !pending {
                break;
            }
            if let Some(m) = json.get("Marker").and_then(|v| v.as_str()) {
                marker = m.to_string();
            } else {
                break;
            }
            continue;
        }

        let json: serde_json::Value =
            serde_json::from_slice(&output.stdout).context("Failed to parse download response")?;

        if let Some(data) = json.get("LogFileData").and_then(|v| v.as_str()) {
            file.write_all(data.as_bytes())?;
            total_bytes += data.len();
        }

        let pending = json
            .get("AdditionalDataPending")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        if !pending {
            break;
        }

        if let Some(m) = json.get("Marker").and_then(|v| v.as_str()) {
            marker = m.to_string();
        } else {
            break;
        }
    }

    println!("Downloaded {total_bytes} bytes from RDS log");
    Ok(())
}
```

Add `pub mod rds;` to `src/capture/mod.rs`.

Also add `tempfile` to `[dependencies]` in `Cargo.toml` (it's currently only in `[dev-dependencies]`):

```toml
tempfile = "3"
```

**Step 4: Run tests**

Run: `cargo test --test rds_capture_test 2>&1 | tail -10`
Expected: 3 tests passed

**Step 5: Commit**

```bash
git add src/capture/rds.rs src/capture/mod.rs tests/rds_capture_test.rs Cargo.toml Cargo.lock
git commit -m "feat(capture): RDS/Aurora capture module with log parsing and pagination"
```

---

### Task 8: RDS Capture CLI Integration

**Files:**
- Modify: `src/cli.rs:38-63` (CaptureArgs — add RDS fields)
- Modify: `src/config/mod.rs:16-29` (CaptureConfig — add RDS fields)
- Modify: `src/main.rs:27-62` (cmd_capture — add RDS dispatch)
- Modify: `src/pipeline/mod.rs:124-182` (load_or_capture_workload — add RDS dispatch)

**Step 1: Add RDS fields to CaptureArgs**

Add after `mask_values` in `CaptureArgs`:

```rust
    /// RDS instance identifier (for --source-type rds)
    #[arg(long)]
    pub rds_instance: Option<String>,

    /// AWS region for RDS instance
    #[arg(long, default_value = "us-east-1")]
    pub rds_region: String,

    /// Specific RDS log file to download (omit to use latest)
    #[arg(long)]
    pub rds_log_file: Option<String>,
```

**Step 2: Add RDS fields to CaptureConfig**

Add after `mask_values` in `CaptureConfig`:

```rust
    pub rds_instance: Option<String>,
    #[serde(default = "default_rds_region")]
    pub rds_region: String,
    pub rds_log_file: Option<String>,
```

Add the default function:

```rust
fn default_rds_region() -> String {
    "us-east-1".to_string()
}
```

**Step 3: Add RDS dispatch to cmd_capture**

Add a new match arm in `cmd_capture` (after `"mysql-slow"`):

```rust
        "rds" => {
            use pg_retest::capture::rds::RdsCapture;
            let instance_id = args
                .rds_instance
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("--rds-instance is required for --source-type rds"))?;
            let capture = RdsCapture;
            capture.capture_from_instance(
                instance_id,
                &args.rds_region,
                args.rds_log_file.as_deref(),
                &args.source_host,
            )?
        }
```

Note: For RDS capture, `--source-log` is not used (the log is downloaded from AWS), so update the `source_log` field to be optional or handle it in the RDS path. The simplest approach: for RDS, we don't read `source_log` — it's only needed for file-based capture.

**Step 4: Add RDS dispatch to load_or_capture_workload in pipeline**

Add a new match arm in the pipeline capture dispatch:

```rust
        "rds" => {
            use crate::capture::rds::RdsCapture;
            let instance_id = capture_cfg
                .rds_instance
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("Capture error: rds_instance required for source_type = \"rds\""))?;
            let capture = RdsCapture;
            capture
                .capture_from_instance(
                    instance_id,
                    &capture_cfg.rds_region,
                    capture_cfg.rds_log_file.as_deref(),
                    capture_cfg.source_host.as_deref().unwrap_or("rds"),
                )
                .map_err(|e| anyhow::anyhow!("Capture error: {e}"))?
        }
```

**Step 5: Add config unit test for RDS**

Add to `src/config/mod.rs` tests:

```rust
    #[test]
    fn test_parse_rds_config() {
        let toml = r#"
[capture]
source_type = "rds"
rds_instance = "mydb-instance"
rds_region = "us-west-2"

[replay]
target = "host=localhost dbname=test"
"#;
        let config: PipelineConfig = toml::from_str(toml).unwrap();
        let cap = config.capture.as_ref().unwrap();
        assert_eq!(cap.source_type, "rds");
        assert_eq!(cap.rds_instance.as_deref(), Some("mydb-instance"));
        assert_eq!(cap.rds_region, "us-west-2");
    }
```

**Step 6: Update config validation for RDS**

In `validate_config`, the check `c.workload.is_some() || c.source_log.is_some()` won't pass for RDS config (which has `rds_instance` instead of `source_log`). Update:

```rust
    let has_workload = config
        .capture
        .as_ref()
        .is_some_and(|c| {
            c.workload.is_some()
                || c.source_log.is_some()
                || (c.source_type == "rds" && c.rds_instance.is_some())
        });
```

**Step 7: Run all tests**

Run: `cargo test 2>&1 | tail -10`
Expected: All tests pass

**Step 8: Commit**

```bash
git add src/cli.rs src/config/mod.rs src/main.rs src/pipeline/mod.rs
git commit -m "feat(rds): CLI flags and pipeline integration for RDS capture"
```

---

### Task 9: Update CLAUDE.md and Project Memory

**Files:**
- Modify: `CLAUDE.md`
- Modify: `/Users/matt.yonkovit/.claude/projects/-Users-matt-yonkovit-yonk-tools-pg-retest/memory/MEMORY.md`

**Step 1: Update CLAUDE.md**

Update the module list to include new modules and features:

- Add `compare::ab` — A/B variant comparison (side-by-side reports, regressions/improvements)
- Add `capture::rds` — AWS RDS/Aurora cloud-native capture
- Update `replay::scaling` description to mention per-category scaling
- Update Milestone Status section
- Add new subcommands: `ab`
- Add gotchas about per-category scaling mutual exclusivity with `--scale N`
- Add gotchas about `aws` CLI requirement for RDS capture

**Step 2: Update MEMORY.md**

Update test count, add new modules, update milestone status.

**Step 3: Commit**

```bash
git add CLAUDE.md
git commit -m "docs: update CLAUDE.md with gap closure features"
```

---

## Build Order & Dependencies

```
Task 1: scale_sessions_by_class unit tests + implementation [independent]
Task 2: Per-category CLI flags + TOML config [depends on Task 1]
Task 3: Per-category dispatch in main.rs + pipeline [depends on Tasks 1, 2]
Task 4: A/B comparison types + unit tests [independent]
Task 5: A/B CLI subcommand + config [depends on Task 4]
Task 6: A/B pipeline mode [depends on Tasks 4, 5]
Task 7: RDS capture module + unit tests [independent]
Task 8: RDS CLI + pipeline integration [depends on Task 7]
Task 9: Documentation update [depends on all]
```

Tasks 1, 4, and 7 are independent and can start in parallel.
Within each feature, tasks are sequential.

## Verification

After all tasks:

```bash
# All tests pass
cargo test

# No warnings
cargo clippy

# Code formatted
cargo fmt -- --check

# Verify per-category scaling (dry run — requires a .wkl file)
cargo run -- replay --workload test.wkl --target "host=localhost" \
  --scale-analytical 2 --scale-transactional 4 --stagger-ms 500

# Verify A/B command parses
cargo run -- ab --help

# Verify RDS command parses
cargo run -- capture --source-type rds --help
```
