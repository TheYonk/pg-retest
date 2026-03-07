use std::collections::HashMap;

use crate::classify::{classify_session, WorkloadClass};
use crate::profile::{QueryKind, Session, WorkloadProfile};

/// Duplicate sessions N times with unique IDs and staggered start offsets.
pub fn scale_sessions(profile: &WorkloadProfile, scale: u32, stagger_ms: u64) -> Vec<Session> {
    if scale <= 1 {
        return profile.sessions.clone();
    }

    let session_count = profile.sessions.len() as u64;
    let stagger_us = stagger_ms * 1000;
    let mut scaled = Vec::with_capacity(profile.sessions.len() * scale as usize);

    for copy_index in 0..scale as u64 {
        for session in &profile.sessions {
            let new_id = session.id + copy_index * session_count;
            let offset = copy_index * stagger_us;

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

            scaled.push(Session {
                id: new_id,
                user: session.user.clone(),
                database: session.database.clone(),
                queries,
            });
        }
    }

    scaled
}

/// Check if scaling a workload with write queries is potentially unsafe.
/// Returns a warning message if writes are detected.
pub fn check_write_safety(profile: &WorkloadProfile) -> Option<String> {
    let mut write_count: u64 = 0;
    let mut total_count: u64 = 0;

    for session in &profile.sessions {
        for q in &session.queries {
            total_count += 1;
            match q.kind {
                QueryKind::Insert | QueryKind::Update | QueryKind::Delete | QueryKind::Ddl => {
                    write_count += 1;
                }
                _ => {}
            }
        }
    }

    if write_count > 0 {
        Some(format!(
            "Warning: scaling a workload with {} write queries (out of {} total). \
             Scaled writes will execute multiple times, which changes data state \
             and may produce different results than the original workload.",
            write_count, total_count
        ))
    } else {
        None
    }
}

/// Scale sessions by workload class. Each class gets its own scale factor.
/// A scale factor of 0 excludes sessions of that class entirely.
/// A scale factor of 1 keeps the original sessions unchanged.
pub fn scale_sessions_by_class(
    profile: &WorkloadProfile,
    class_scales: &HashMap<WorkloadClass, u32>,
    stagger_ms: u64,
) -> Vec<Session> {
    let stagger_us = stagger_ms * 1000;

    // Classify and group sessions
    let mut grouped: HashMap<WorkloadClass, Vec<&Session>> = HashMap::new();
    for session in &profile.sessions {
        let classification = classify_session(session);
        grouped
            .entry(classification.class)
            .or_default()
            .push(session);
    }

    // First pass: add all copy-0 (original) sessions with no stagger
    let mut result: Vec<Session> = Vec::new();
    let mut next_id: u64 = profile.sessions.iter().map(|s| s.id).max().unwrap_or(0) + 1;

    for (class, sessions) in &grouped {
        let scale = class_scales.get(class).copied().unwrap_or(1);
        if scale == 0 {
            continue;
        }
        // Copy 0: keep original sessions with original IDs and offsets
        for session in sessions {
            result.push((*session).clone());
        }
    }

    // Second pass: add duplicate copies (copy_index >= 1) with stagger
    let mut global_copy: u64 = 0; // tracks stagger across all classes
    for (class, sessions) in &grouped {
        let scale = class_scales.get(class).copied().unwrap_or(1);
        if scale <= 1 {
            continue; // 0 already excluded, 1 already added
        }

        for _copy_index in 1..scale as u64 {
            global_copy += 1;
            let offset = global_copy * stagger_us;

            for session in sessions {
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
                    id: next_id,
                    user: session.user.clone(),
                    database: session.database.clone(),
                    queries,
                });
                next_id += 1;
            }
        }
    }

    // Sort by first query offset so replay order is chronological
    result.sort_by_key(|s| s.queries.first().map_or(0, |q| q.start_offset_us));
    result
}
