use pg_retest::transform::mysql_to_pg::mysql_to_pg_pipeline;
use pg_retest::transform::TransformResult;

#[test]
fn test_full_pipeline_simple_select() {
    let pipeline = mysql_to_pg_pipeline();
    let result = pipeline.apply("SELECT * FROM users WHERE id = 1");
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
    let result =
        pipeline.apply("SELECT IFNULL(name, 'anon'), IF(active = 1, 'yes', 'no') FROM users");
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
