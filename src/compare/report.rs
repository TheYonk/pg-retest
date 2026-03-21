use std::path::Path;

use anyhow::Result;

use super::ComparisonReport;

pub fn print_terminal_report(report: &ComparisonReport) {
    println!();
    println!("  pg-retest Comparison Report");
    println!("  ===========================");
    println!();
    println!(
        "  {:<16} {:>10} {:>10} {:>10} {:>8}",
        "Metric", "Source", "Replay", "Delta", "Status"
    );
    println!("  {}", "-".repeat(58));

    let rows = vec![
        make_row(
            "Total queries",
            report.total_queries_source,
            report.total_queries_replayed,
        ),
        make_latency_row(
            "Avg latency",
            report.source_avg_latency_us,
            report.replay_avg_latency_us,
        ),
        make_latency_row(
            "P50 latency",
            report.source_p50_latency_us,
            report.replay_p50_latency_us,
        ),
        make_latency_row(
            "P95 latency",
            report.source_p95_latency_us,
            report.replay_p95_latency_us,
        ),
        make_latency_row(
            "P99 latency",
            report.source_p99_latency_us,
            report.replay_p99_latency_us,
        ),
        (
            "Errors".to_string(),
            "0".to_string(),
            report.total_errors.to_string(),
            if report.total_errors > 0 {
                format!("+{}", report.total_errors)
            } else {
                "0".to_string()
            },
            if report.total_errors > 0 {
                "WARN"
            } else {
                "OK"
            }
            .to_string(),
        ),
    ];

    for (metric, source, replay, delta, status) in &rows {
        println!(
            "  {:<16} {:>10} {:>10} {:>10} {:>8}",
            metric, source, replay, delta, status
        );
    }
    println!();

    if report.total_queries_filtered > 0 {
        println!(
            "  (Filtered {} source queries to match replay population)",
            report.total_queries_filtered
        );
        println!();
    }

    if !report.regressions.is_empty() {
        let top_n = report.regressions.len().min(10);
        println!("  Top {} Regressions:", top_n);
        println!("  {}", "-".repeat(58));
        for (i, reg) in report.regressions.iter().take(top_n).enumerate() {
            let sql_preview: String = reg.sql.chars().take(50).collect();
            println!(
                "  {}. {} +{:.1}% ({:.1}ms -> {:.1}ms)",
                i + 1,
                sql_preview,
                reg.change_pct,
                reg.original_us as f64 / 1000.0,
                reg.replay_us as f64 / 1000.0,
            );
        }
        println!();
    }
}

pub fn write_json_report(path: &Path, report: &ComparisonReport) -> Result<()> {
    let json = serde_json::to_string_pretty(report)?;
    std::fs::write(path, json)?;
    Ok(())
}

fn make_row(metric: &str, source: u64, replay: u64) -> (String, String, String, String, String) {
    let delta = replay as i64 - source as i64;
    let delta_str = if delta == 0 {
        "0".to_string()
    } else {
        format!("{:+}", delta)
    };
    let status = if delta == 0 { "OK" } else { "DIFF" };
    (
        metric.to_string(),
        source.to_string(),
        replay.to_string(),
        delta_str,
        status.to_string(),
    )
}

fn make_latency_row(
    metric: &str,
    source_us: u64,
    replay_us: u64,
) -> (String, String, String, String, String) {
    let source_ms = source_us as f64 / 1000.0;
    let replay_ms = replay_us as f64 / 1000.0;
    let delta_pct = if source_us > 0 {
        ((replay_us as f64 - source_us as f64) / source_us as f64) * 100.0
    } else {
        0.0
    };
    let status = if delta_pct < -5.0 {
        "FASTER"
    } else if delta_pct > 5.0 {
        "SLOWER"
    } else {
        "OK"
    };
    (
        metric.to_string(),
        format!("{:.1}ms", source_ms),
        format!("{:.1}ms", replay_ms),
        format!("{:+.1}%", delta_pct),
        status.to_string(),
    )
}
