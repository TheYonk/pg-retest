use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};

use anyhow::Result;
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

use crate::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use crate::transform::plan::{TransformPlan, TransformRule};

use super::analyze::extract_tables;

/// Apply a transform plan to a workload profile, producing a new profile.
///
/// The transform is fully deterministic when `seed` is provided. If `seed`
/// is `None`, a seed is derived by hashing the plan's prompt string.
pub fn apply_transform(
    profile: &WorkloadProfile,
    plan: &TransformPlan,
    seed: Option<u64>,
) -> Result<WorkloadProfile> {
    let actual_seed = seed.unwrap_or_else(|| {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        plan.source.prompt.hash(&mut hasher);
        hasher.finish()
    });
    let mut rng = StdRng::seed_from_u64(actual_seed);

    // Build flat_index -> group_name lookup from plan.groups.
    // flat_index is the global query index across all sessions in order.
    let mut query_group_map: HashMap<usize, String> = HashMap::new();
    for group in &plan.groups {
        for &idx in &group.query_indices {
            query_group_map.insert(idx, group.name.clone());
        }
    }

    // Collect groups to remove.
    let remove_groups: HashSet<String> = plan
        .transforms
        .iter()
        .filter_map(|t| match t {
            TransformRule::Remove { group } => Some(group.clone()),
            _ => None,
        })
        .collect();

    // Collect inject rules.
    struct InjectRule {
        sql: String,
        after_group: String,
        frequency: f64,
        estimated_duration_us: u64,
    }
    let inject_rules: Vec<InjectRule> = plan
        .transforms
        .iter()
        .filter_map(|t| match t {
            TransformRule::Inject {
                sql,
                after_group,
                frequency,
                estimated_duration_us,
                ..
            } => Some(InjectRule {
                sql: sql.clone(),
                after_group: after_group.clone(),
                frequency: *frequency,
                estimated_duration_us: *estimated_duration_us,
            }),
            _ => None,
        })
        .collect();

    // Collect scale rules: group_name -> (factor, stagger_ms).
    let scale_rules: HashMap<String, (f64, u64)> = plan
        .transforms
        .iter()
        .filter_map(|t| match t {
            TransformRule::Scale {
                group,
                factor,
                stagger_ms,
            } => Some((group.clone(), (*factor, *stagger_ms))),
            _ => None,
        })
        .collect();

    // Build group_name -> set of tables for affinity matching.
    let group_tables: HashMap<String, HashSet<String>> = plan
        .groups
        .iter()
        .map(|g| {
            let tables: HashSet<String> = g.tables.iter().cloned().collect();
            (g.name.clone(), tables)
        })
        .collect();

    // -----------------------------------------------------------------------
    // Step 1: Remove + Inject into existing sessions
    // -----------------------------------------------------------------------
    let mut modified_sessions: Vec<Session> = Vec::new();
    let mut flat_index: usize = 0;

    for session in &profile.sessions {
        let mut new_queries: Vec<Query> = Vec::new();

        for query in &session.queries {
            let group_name = query_group_map.get(&flat_index).cloned();
            flat_index += 1;

            // Skip queries whose group is in remove_groups.
            if let Some(ref gn) = group_name {
                if remove_groups.contains(gn) {
                    continue;
                }
            }

            new_queries.push(query.clone());

            // Check inject rules: if query belongs to after_group, maybe inject.
            if let Some(ref gn) = group_name {
                for rule in &inject_rules {
                    if *gn == rule.after_group && rng.gen::<f64>() < rule.frequency {
                        let offset = query.start_offset_us + query.duration_us;
                        new_queries.push(Query {
                            sql: rule.sql.clone(),
                            start_offset_us: offset,
                            duration_us: rule.estimated_duration_us,
                            kind: QueryKind::from_sql(&rule.sql),
                            transaction_id: None,
                        });
                    }
                }
            }
        }

        modified_sessions.push(Session {
            id: session.id,
            user: session.user.clone(),
            database: session.database.clone(),
            queries: new_queries,
        });
    }

    // -----------------------------------------------------------------------
    // Step 2: Weighted session duplication (scaling)
    // -----------------------------------------------------------------------
    let mut next_id: u64 = profile.sessions.iter().map(|s| s.id).max().unwrap_or(0) + 1;
    let mut global_stagger_count: u64 = 0;
    let mut duplicated_sessions: Vec<Session> = Vec::new();

    // Determine a representative stagger_us from scale rules (use max).
    let stagger_us: u64 = scale_rules
        .values()
        .map(|(_, ms)| ms * 1000)
        .max()
        .unwrap_or(10_000);

    for session in &modified_sessions {
        // Compute group affinity: for each query, find which groups it belongs to
        // based on table overlap, then compute weighted scale factor.
        let mut group_query_counts: HashMap<String, usize> = HashMap::new();
        let mut total_matched: usize = 0;

        for query in &session.queries {
            let tables = extract_tables(&query.sql);
            for (gname, gtables) in &group_tables {
                if tables.iter().any(|t| gtables.contains(t)) {
                    *group_query_counts.entry(gname.clone()).or_insert(0) += 1;
                    total_matched += 1;
                    break; // each query counted once
                }
            }
        }

        let effective_scale = if total_matched == 0 || session.queries.is_empty() {
            1.0
        } else {
            let total = session.queries.len() as f64;
            let mut weighted = 0.0;
            let mut matched_pct_sum = 0.0;

            for (gname, &count) in &group_query_counts {
                let pct = count as f64 / total;
                let factor = scale_rules.get(gname).map(|(f, _)| *f).unwrap_or(1.0);
                weighted += pct * factor;
                matched_pct_sum += pct;
            }

            // Unmatched queries contribute at scale 1.0.
            let unmatched_pct = 1.0 - matched_pct_sum;
            weighted += unmatched_pct * 1.0;
            weighted
        };

        let copies = (effective_scale.round() as u64).max(1) - 1;

        for _ in 0..copies {
            global_stagger_count += 1;
            let offset = global_stagger_count * stagger_us;

            let dup_queries: Vec<Query> = session
                .queries
                .iter()
                .map(|q| Query {
                    sql: q.sql.clone(),
                    start_offset_us: q.start_offset_us + offset,
                    duration_us: q.duration_us,
                    kind: q.kind,
                    transaction_id: None,
                })
                .collect();

            duplicated_sessions.push(Session {
                id: next_id,
                user: session.user.clone(),
                database: session.database.clone(),
                queries: dup_queries,
            });
            next_id += 1;
        }
    }

    modified_sessions.extend(duplicated_sessions);

    // -----------------------------------------------------------------------
    // Step 3: Inject new sessions
    // -----------------------------------------------------------------------
    let (default_user, default_database) = profile
        .sessions
        .first()
        .map(|s| (s.user.clone(), s.database.clone()))
        .unwrap_or_else(|| ("postgres".into(), "postgres".into()));

    for rule in &plan.transforms {
        if let TransformRule::InjectSession {
            queries,
            repeat,
            interval_ms,
            ..
        } = rule
        {
            for i in 0..*repeat {
                let base_offset = i as u64 * interval_ms * 1000;
                let session_queries: Vec<Query> = queries
                    .iter()
                    .enumerate()
                    .map(|(qi, iq)| {
                        let offset = base_offset + qi as u64 * iq.duration_us;
                        Query {
                            sql: iq.sql.clone(),
                            start_offset_us: offset,
                            duration_us: iq.duration_us,
                            kind: QueryKind::from_sql(&iq.sql),
                            transaction_id: None,
                        }
                    })
                    .collect();

                modified_sessions.push(Session {
                    id: next_id,
                    user: default_user.clone(),
                    database: default_database.clone(),
                    queries: session_queries,
                });
                next_id += 1;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Step 4: Update metadata
    // -----------------------------------------------------------------------
    let total_queries: u64 = modified_sessions
        .iter()
        .map(|s| s.queries.len() as u64)
        .sum();
    let total_sessions = modified_sessions.len() as u64;
    let capture_duration_us = modified_sessions
        .iter()
        .flat_map(|s| s.queries.iter())
        .map(|q| q.start_offset_us + q.duration_us)
        .max()
        .unwrap_or(0);

    Ok(WorkloadProfile {
        version: profile.version,
        captured_at: profile.captured_at,
        source_host: profile.source_host.clone(),
        pg_version: profile.pg_version.clone(),
        capture_method: "transformed".into(),
        sessions: modified_sessions,
        metadata: Metadata {
            total_queries,
            total_sessions,
            capture_duration_us,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::profile::*;
    use crate::transform::plan::*;

    fn test_profile() -> WorkloadProfile {
        WorkloadProfile {
            version: 2,
            captured_at: chrono::Utc::now(),
            source_host: "localhost".into(),
            pg_version: "16".into(),
            capture_method: "test".into(),
            sessions: vec![
                Session {
                    id: 1,
                    user: "app".into(),
                    database: "mydb".into(),
                    queries: vec![
                        Query {
                            sql: "SELECT * FROM products WHERE id = $1".into(),
                            start_offset_us: 0,
                            duration_us: 100,
                            kind: QueryKind::Select,
                            transaction_id: None,
                        },
                        Query {
                            sql: "SELECT * FROM categories".into(),
                            start_offset_us: 200,
                            duration_us: 50,
                            kind: QueryKind::Select,
                            transaction_id: None,
                        },
                        Query {
                            sql: "INSERT INTO orders (product_id) VALUES ($1)".into(),
                            start_offset_us: 400,
                            duration_us: 80,
                            kind: QueryKind::Insert,
                            transaction_id: None,
                        },
                    ],
                },
                Session {
                    id: 2,
                    user: "app".into(),
                    database: "mydb".into(),
                    queries: vec![Query {
                        sql: "SELECT * FROM products WHERE active = true".into(),
                        start_offset_us: 0,
                        duration_us: 150,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    }],
                },
            ],
            metadata: Metadata {
                total_queries: 4,
                total_sessions: 2,
                capture_duration_us: 480,
            },
        }
    }

    #[test]
    fn test_apply_scale_increases_sessions() {
        let profile = test_profile();
        let plan = TransformPlan {
            version: 1,
            source: PlanSource {
                profile: "test.wkl".into(),
                prompt: "test".into(),
            },
            analysis: PlanAnalysis {
                total_queries: 4,
                total_sessions: 2,
                groups_identified: 2,
            },
            groups: vec![
                QueryGroup {
                    name: "product_catalog".into(),
                    description: "".into(),
                    tables: vec!["products".into(), "categories".into()],
                    query_indices: vec![0, 1, 3],
                    session_ids: vec![1, 2],
                    query_count: 3,
                },
                QueryGroup {
                    name: "orders".into(),
                    description: "".into(),
                    tables: vec!["orders".into()],
                    query_indices: vec![2],
                    session_ids: vec![1],
                    query_count: 1,
                },
            ],
            transforms: vec![TransformRule::Scale {
                group: "product_catalog".into(),
                factor: 3.0,
                stagger_ms: 10,
            }],
        };
        let result = apply_transform(&profile, &plan, None).unwrap();
        assert!(result.sessions.len() > 2);
        assert!(result.metadata.total_queries > 4);
    }

    #[test]
    fn test_apply_remove_drops_queries() {
        let profile = test_profile();
        let plan = TransformPlan {
            version: 1,
            source: PlanSource {
                profile: "test.wkl".into(),
                prompt: "test".into(),
            },
            analysis: PlanAnalysis {
                total_queries: 4,
                total_sessions: 2,
                groups_identified: 1,
            },
            groups: vec![QueryGroup {
                name: "orders".into(),
                description: "".into(),
                tables: vec!["orders".into()],
                query_indices: vec![2],
                session_ids: vec![1],
                query_count: 1,
            }],
            transforms: vec![TransformRule::Remove {
                group: "orders".into(),
            }],
        };
        let result = apply_transform(&profile, &plan, None).unwrap();
        let s1 = result.sessions.iter().find(|s| s.id == 1).unwrap();
        assert_eq!(s1.queries.len(), 2);
        assert!(!s1.queries.iter().any(|q| q.sql.contains("orders")));
    }

    #[test]
    fn test_apply_inject_adds_queries() {
        let profile = test_profile();
        let plan = TransformPlan {
            version: 1,
            source: PlanSource {
                profile: "test.wkl".into(),
                prompt: "test".into(),
            },
            analysis: PlanAnalysis {
                total_queries: 4,
                total_sessions: 2,
                groups_identified: 1,
            },
            groups: vec![QueryGroup {
                name: "product_catalog".into(),
                description: "".into(),
                tables: vec!["products".into(), "categories".into()],
                query_indices: vec![0, 1, 3],
                session_ids: vec![1, 2],
                query_count: 3,
            }],
            transforms: vec![TransformRule::Inject {
                description: "Review lookup".into(),
                sql: "SELECT * FROM reviews WHERE product_id = $1".into(),
                after_group: "product_catalog".into(),
                frequency: 1.0,
                estimated_duration_us: 5000,
            }],
        };
        let result = apply_transform(&profile, &plan, Some(42)).unwrap();
        let total: usize = result.sessions.iter().map(|s| s.queries.len()).sum();
        assert!(total > 4);
        assert!(result
            .sessions
            .iter()
            .any(|s| s.queries.iter().any(|q| q.sql.contains("reviews"))));
    }

    #[test]
    fn test_apply_inject_session() {
        let profile = test_profile();
        let plan = TransformPlan {
            version: 1,
            source: PlanSource {
                profile: "test.wkl".into(),
                prompt: "test".into(),
            },
            analysis: PlanAnalysis {
                total_queries: 4,
                total_sessions: 2,
                groups_identified: 0,
            },
            groups: vec![],
            transforms: vec![TransformRule::InjectSession {
                description: "Background job".into(),
                queries: vec![InjectedQuery {
                    sql: "INSERT INTO audit_log VALUES ($1)".into(),
                    duration_us: 1000,
                }],
                repeat: 3,
                interval_ms: 100,
            }],
        };
        let result = apply_transform(&profile, &plan, None).unwrap();
        assert_eq!(result.sessions.len(), 5); // 2 original + 3 injected
    }

    #[test]
    fn test_deterministic_with_seed() {
        let profile = test_profile();
        let plan = TransformPlan {
            version: 1,
            source: PlanSource {
                profile: "test.wkl".into(),
                prompt: "test".into(),
            },
            analysis: PlanAnalysis {
                total_queries: 4,
                total_sessions: 2,
                groups_identified: 1,
            },
            groups: vec![QueryGroup {
                name: "product_catalog".into(),
                description: "".into(),
                tables: vec!["products".into()],
                query_indices: vec![0, 1, 3],
                session_ids: vec![1, 2],
                query_count: 3,
            }],
            transforms: vec![TransformRule::Inject {
                description: "50% frequency".into(),
                sql: "SELECT 1".into(),
                after_group: "product_catalog".into(),
                frequency: 0.5,
                estimated_duration_us: 100,
            }],
        };
        let r1 = apply_transform(&profile, &plan, Some(42)).unwrap();
        let r2 = apply_transform(&profile, &plan, Some(42)).unwrap();
        assert_eq!(r1.sessions.len(), r2.sessions.len());
        assert_eq!(r1.metadata.total_queries, r2.metadata.total_queries);
    }
}
