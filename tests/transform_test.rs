use pg_retest::profile::{Metadata, Query, QueryKind, Session, WorkloadProfile};
use pg_retest::transform::analyze::analyze_workload;
use pg_retest::transform::engine::apply_transform;
use pg_retest::transform::plan::*;

fn sample_profile() -> WorkloadProfile {
    WorkloadProfile {
        version: 2,
        captured_at: chrono::Utc::now(),
        source_host: "localhost:5432".into(),
        pg_version: "16.0".into(),
        capture_method: "test".into(),
        sessions: vec![
            Session {
                id: 1,
                user: "app".into(),
                database: "testdb".into(),
                queries: vec![
                    Query {
                        sql: "SELECT * FROM products WHERE id = $1".into(),
                        start_offset_us: 0,
                        duration_us: 500,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT p.name, c.label FROM products p JOIN categories c ON p.category_id = c.id WHERE p.id = $1".into(),
                        start_offset_us: 1000,
                        duration_us: 200,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "INSERT INTO orders (product_id, qty) VALUES ($1, $2)".into(),
                        start_offset_us: 2000,
                        duration_us: 300,
                        kind: QueryKind::Insert,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT * FROM products WHERE category_id = $1".into(),
                        start_offset_us: 3000,
                        duration_us: 800,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                ],
            },
            Session {
                id: 2,
                user: "app".into(),
                database: "testdb".into(),
                queries: vec![
                    Query {
                        sql: "SELECT * FROM products ORDER BY created_at DESC LIMIT 10".into(),
                        start_offset_us: 0,
                        duration_us: 1200,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                    Query {
                        sql: "SELECT count(*) FROM orders WHERE status = 'pending'".into(),
                        start_offset_us: 2000,
                        duration_us: 400,
                        kind: QueryKind::Select,
                        transaction_id: None,
                    },
                ],
            },
        ],
        metadata: Metadata {
            total_queries: 6,
            total_sessions: 2,
            capture_duration_us: 3800,
        },
    }
}

#[test]
fn test_analyze_groups_related_tables() {
    let profile = sample_profile();
    let analysis = analyze_workload(&profile);

    // products and categories should be grouped (they co-appear in session 1)
    let product_group = analysis
        .query_groups
        .iter()
        .find(|g| g.tables.contains(&"products".to_string()));
    assert!(product_group.is_some());
    let pg = product_group.unwrap();
    assert!(pg.tables.contains(&"categories".to_string()));
}

#[test]
fn test_analyze_separate_groups() {
    let profile = sample_profile();
    let analysis = analyze_workload(&profile);

    assert!(analysis.query_groups.len() >= 1);
    assert_eq!(analysis.profile_summary.total_queries, 6);
    assert_eq!(analysis.profile_summary.total_sessions, 2);
}

#[test]
fn test_analyze_parameter_patterns() {
    let profile = sample_profile();
    let analysis = analyze_workload(&profile);

    // At least one group should detect bind parameters
    let has_params = analysis
        .query_groups
        .iter()
        .any(|g| g.parameter_patterns.bind_params_seen > 0);
    assert!(has_params, "Should detect $1/$2 bind parameters");
}

#[test]
fn test_analyze_filter_columns() {
    let profile = sample_profile();
    let analysis = analyze_workload(&profile);

    // The product group should detect filter columns like id, category_id
    let product_group = analysis
        .query_groups
        .iter()
        .find(|g| g.tables.contains(&"products".to_string()))
        .unwrap();
    assert!(
        !product_group.parameter_patterns.common_filters.is_empty(),
        "Should extract common filter columns"
    );
}

#[test]
fn test_full_transform_pipeline() {
    let profile = sample_profile();
    let analysis = analyze_workload(&profile);

    // Build a plan based on analysis
    let plan = TransformPlan {
        version: 1,
        source: PlanSource {
            profile: "test.wkl".into(),
            prompt: "Double product traffic".into(),
        },
        analysis: PlanAnalysis {
            total_queries: analysis.profile_summary.total_queries,
            total_sessions: analysis.profile_summary.total_sessions,
            groups_identified: analysis.query_groups.len(),
        },
        groups: analysis
            .query_groups
            .iter()
            .map(|g| QueryGroup {
                name: g.tables.first().cloned().unwrap_or_else(|| "unknown".into()),
                description: format!("Queries touching {}", g.tables.join(", ")),
                tables: g.tables.clone(),
                query_indices: vec![],
                session_ids: g.sessions.clone(),
                query_count: g.query_count,
            })
            .collect(),
        transforms: vec![TransformRule::Scale {
            group: analysis
                .query_groups
                .first()
                .and_then(|g| g.tables.first())
                .cloned()
                .unwrap_or_else(|| "products".into()),
            factor: 2.0,
            stagger_ms: 10,
        }],
    };

    let result = apply_transform(&profile, &plan, Some(42)).unwrap();
    assert!(result.metadata.total_sessions >= 2); // at least original sessions
    assert!(result.metadata.total_queries >= 6); // at least original queries
    assert_eq!(result.capture_method, "transformed");
}

#[test]
fn test_transform_plan_toml_roundtrip_full() {
    let plan = TransformPlan {
        version: 1,
        source: PlanSource {
            profile: "workload.wkl".into(),
            prompt: "5x product traffic".into(),
        },
        analysis: PlanAnalysis {
            total_queries: 100,
            total_sessions: 5,
            groups_identified: 3,
        },
        groups: vec![QueryGroup {
            name: "products".into(),
            description: "Product catalog".into(),
            tables: vec!["products".into(), "categories".into()],
            query_indices: vec![0, 1, 2, 10, 11],
            session_ids: vec![1, 3],
            query_count: 5,
        }],
        transforms: vec![
            TransformRule::Scale {
                group: "products".into(),
                factor: 5.0,
                stagger_ms: 10,
            },
            TransformRule::Inject {
                description: "Reviews".into(),
                sql: "SELECT * FROM reviews WHERE product_id = $1".into(),
                after_group: "products".into(),
                frequency: 0.8,
                estimated_duration_us: 5000,
            },
            TransformRule::InjectSession {
                description: "Background".into(),
                queries: vec![InjectedQuery {
                    sql: "INSERT INTO audit VALUES ($1)".into(),
                    duration_us: 1000,
                }],
                repeat: 10,
                interval_ms: 500,
            },
            TransformRule::Remove {
                group: "reporting".into(),
            },
        ],
    };

    // TOML roundtrip
    let toml_str = toml::to_string_pretty(&plan).unwrap();
    let parsed: TransformPlan = toml::from_str(&toml_str).unwrap();
    assert_eq!(parsed.groups.len(), 1);
    assert_eq!(parsed.transforms.len(), 4);
    assert_eq!(parsed.groups[0].name, "products");

    // JSON roundtrip (for web API)
    let json_str = serde_json::to_string(&plan).unwrap();
    let parsed_json: TransformPlan = serde_json::from_str(&json_str).unwrap();
    assert_eq!(parsed_json.transforms.len(), 4);
}

#[test]
fn test_deterministic_transform() {
    let profile = sample_profile();
    let plan = TransformPlan {
        version: 1,
        source: PlanSource {
            profile: "test.wkl".into(),
            prompt: "test".into(),
        },
        analysis: PlanAnalysis {
            total_queries: 6,
            total_sessions: 2,
            groups_identified: 1,
        },
        groups: vec![QueryGroup {
            name: "products".into(),
            description: "".into(),
            tables: vec!["products".into()],
            query_indices: vec![0, 3, 4],
            session_ids: vec![1, 2],
            query_count: 3,
        }],
        transforms: vec![TransformRule::Inject {
            description: "".into(),
            sql: "SELECT 1".into(),
            after_group: "products".into(),
            frequency: 0.5,
            estimated_duration_us: 100,
        }],
    };

    let r1 = apply_transform(&profile, &plan, Some(123)).unwrap();
    let r2 = apply_transform(&profile, &plan, Some(123)).unwrap();
    assert_eq!(r1.sessions.len(), r2.sessions.len());
    assert_eq!(r1.metadata.total_queries, r2.metadata.total_queries);

    // Same seed = identical query content
    for (s1, s2) in r1.sessions.iter().zip(r2.sessions.iter()) {
        assert_eq!(s1.queries.len(), s2.queries.len());
        for (q1, q2) in s1.queries.iter().zip(s2.queries.iter()) {
            assert_eq!(q1.sql, q2.sql);
        }
    }

    // Different seed = potentially different result (but doesn't crash)
    let r3 = apply_transform(&profile, &plan, Some(456)).unwrap();
    assert!(r3.metadata.total_queries > 0);
}

#[test]
fn test_inject_session_creates_new_sessions() {
    let profile = sample_profile();
    let plan = TransformPlan {
        version: 1,
        source: PlanSource {
            profile: "test.wkl".into(),
            prompt: "inject sessions".into(),
        },
        analysis: PlanAnalysis {
            total_queries: 6,
            total_sessions: 2,
            groups_identified: 1,
        },
        groups: vec![],
        transforms: vec![TransformRule::InjectSession {
            description: "Background audit".into(),
            queries: vec![
                InjectedQuery {
                    sql: "INSERT INTO audit_log VALUES (now())".into(),
                    duration_us: 500,
                },
                InjectedQuery {
                    sql: "SELECT count(*) FROM audit_log".into(),
                    duration_us: 200,
                },
            ],
            repeat: 3,
            interval_ms: 1000,
        }],
    };

    let result = apply_transform(&profile, &plan, Some(42)).unwrap();
    // Should have original 2 sessions + 3 injected
    assert_eq!(result.sessions.len(), 5);
    // Each injected session has 2 queries
    let injected_queries: usize = result.sessions[2..].iter().map(|s| s.queries.len()).sum();
    assert_eq!(injected_queries, 6);
    assert_eq!(result.capture_method, "transformed");
}

#[test]
fn test_remove_group_filters_queries() {
    let profile = sample_profile();
    let plan = TransformPlan {
        version: 1,
        source: PlanSource {
            profile: "test.wkl".into(),
            prompt: "remove orders".into(),
        },
        analysis: PlanAnalysis {
            total_queries: 6,
            total_sessions: 2,
            groups_identified: 1,
        },
        groups: vec![QueryGroup {
            name: "orders".into(),
            description: "Order queries".into(),
            tables: vec!["orders".into()],
            query_indices: vec![2, 5],
            session_ids: vec![1, 2],
            query_count: 2,
        }],
        transforms: vec![TransformRule::Remove {
            group: "orders".into(),
        }],
    };

    let result = apply_transform(&profile, &plan, Some(42)).unwrap();
    // No query in the result should touch the orders table
    for session in &result.sessions {
        for query in &session.queries {
            let sql_lower = query.sql.to_lowercase();
            assert!(
                !sql_lower.contains("orders"),
                "Removed group query still present: {}",
                query.sql
            );
        }
    }
    // Should have fewer queries than original
    assert!(result.metadata.total_queries < 6);
}
