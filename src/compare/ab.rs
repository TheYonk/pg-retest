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
