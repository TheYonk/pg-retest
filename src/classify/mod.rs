use std::fmt;

use serde::{Deserialize, Serialize};

use crate::profile::{QueryKind, Session, WorkloadProfile};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum WorkloadClass {
    Analytical,
    Transactional,
    Mixed,
    Bulk,
}

impl fmt::Display for WorkloadClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            WorkloadClass::Analytical => write!(f, "Analytical"),
            WorkloadClass::Transactional => write!(f, "Transactional"),
            WorkloadClass::Mixed => write!(f, "Mixed"),
            WorkloadClass::Bulk => write!(f, "Bulk"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionClassification {
    pub session_id: u64,
    pub class: WorkloadClass,
    pub read_pct: f64,
    pub write_pct: f64,
    pub avg_latency_us: u64,
    pub transaction_count: u64,
    pub query_count: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkloadClassification {
    pub overall_class: WorkloadClass,
    pub sessions: Vec<SessionClassification>,
    pub class_counts: ClassCounts,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClassCounts {
    pub analytical: u64,
    pub transactional: u64,
    pub mixed: u64,
    pub bulk: u64,
}

pub fn classify_session(session: &Session) -> SessionClassification {
    let query_count = session.queries.len() as u64;
    if query_count == 0 {
        return SessionClassification {
            session_id: session.id,
            class: WorkloadClass::Mixed,
            read_pct: 0.0,
            write_pct: 0.0,
            avg_latency_us: 0,
            transaction_count: 0,
            query_count: 0,
        };
    }

    let mut reads: u64 = 0;
    let mut writes: u64 = 0;
    let mut total_latency: u64 = 0;
    let mut transaction_count: u64 = 0;

    for q in &session.queries {
        total_latency += q.duration_us;
        match q.kind {
            QueryKind::Select => reads += 1,
            QueryKind::Insert | QueryKind::Update | QueryKind::Delete | QueryKind::Ddl => {
                writes += 1
            }
            QueryKind::Begin => transaction_count += 1,
            QueryKind::Commit | QueryKind::Rollback | QueryKind::Other => {}
        }
    }

    let data_queries = reads + writes;
    let read_pct = if data_queries > 0 {
        reads as f64 / data_queries as f64 * 100.0
    } else {
        0.0
    };
    let write_pct = if data_queries > 0 {
        writes as f64 / data_queries as f64 * 100.0
    } else {
        0.0
    };
    let avg_latency_us = total_latency / query_count;

    // Classification thresholds
    let class = if write_pct > 80.0 && transaction_count <= 2 {
        WorkloadClass::Bulk
    } else if read_pct > 80.0 && avg_latency_us > 10_000 {
        // >80% reads, avg latency >10ms → analytical
        WorkloadClass::Analytical
    } else if write_pct > 20.0 && avg_latency_us < 5_000 && transaction_count > 2 {
        // >20% writes, avg <5ms, multiple transactions → transactional
        WorkloadClass::Transactional
    } else {
        WorkloadClass::Mixed
    };

    SessionClassification {
        session_id: session.id,
        class,
        read_pct,
        write_pct,
        avg_latency_us,
        transaction_count,
        query_count,
    }
}

pub fn classify_workload(profile: &WorkloadProfile) -> WorkloadClassification {
    let sessions: Vec<SessionClassification> =
        profile.sessions.iter().map(classify_session).collect();

    let mut counts = ClassCounts {
        analytical: 0,
        transactional: 0,
        mixed: 0,
        bulk: 0,
    };

    for s in &sessions {
        match s.class {
            WorkloadClass::Analytical => counts.analytical += 1,
            WorkloadClass::Transactional => counts.transactional += 1,
            WorkloadClass::Mixed => counts.mixed += 1,
            WorkloadClass::Bulk => counts.bulk += 1,
        }
    }

    // Majority vote
    let overall_class = [
        (counts.analytical, WorkloadClass::Analytical),
        (counts.transactional, WorkloadClass::Transactional),
        (counts.mixed, WorkloadClass::Mixed),
        (counts.bulk, WorkloadClass::Bulk),
    ]
    .into_iter()
    .max_by_key(|(count, _)| *count)
    .map(|(_, class)| class)
    .unwrap_or(WorkloadClass::Mixed);

    WorkloadClassification {
        overall_class,
        sessions,
        class_counts: counts,
    }
}

pub fn print_classification(classification: &WorkloadClassification) {
    println!();
    println!("  Workload Classification");
    println!("  =======================");
    println!();
    println!("  Overall: {}", classification.overall_class);
    println!(
        "  Sessions: {} total ({} analytical, {} transactional, {} mixed, {} bulk)",
        classification.sessions.len(),
        classification.class_counts.analytical,
        classification.class_counts.transactional,
        classification.class_counts.mixed,
        classification.class_counts.bulk,
    );
    println!();

    println!(
        "  {:<10} {:<16} {:>8} {:>8} {:>12} {:>6}",
        "Session", "Class", "Reads%", "Writes%", "Avg Lat(ms)", "Txns"
    );
    println!("  {}", "-".repeat(64));

    for s in &classification.sessions {
        println!(
            "  {:<10} {:<16} {:>7.1}% {:>7.1}% {:>11.1} {:>6}",
            s.session_id,
            s.class.to_string(),
            s.read_pct,
            s.write_pct,
            s.avg_latency_us as f64 / 1000.0,
            s.transaction_count,
        );
    }
    println!();
}
