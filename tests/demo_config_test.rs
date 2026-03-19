use pg_retest::web::state::DemoConfig;
use std::path::PathBuf;
use std::sync::Mutex;

// Env var tests must not run in parallel — use a module-level mutex.
static ENV_LOCK: Mutex<()> = Mutex::new(());

#[test]
fn test_demo_config_disabled_by_default() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::remove_var("PG_RETEST_DEMO");
    let config = DemoConfig::from_env();
    assert!(config.is_none());
}

#[test]
fn test_demo_config_requires_db_strings() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PG_RETEST_DEMO", "true");
    std::env::remove_var("DEMO_DB_A");
    std::env::remove_var("DEMO_DB_B");
    let config = DemoConfig::from_env();
    assert!(config.is_none());
    std::env::remove_var("PG_RETEST_DEMO");
}

#[test]
fn test_demo_config_enabled_with_both_db_strings() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PG_RETEST_DEMO", "true");
    std::env::set_var(
        "DEMO_DB_A",
        "host=db-a dbname=ecommerce user=demo password=demo",
    );
    std::env::set_var(
        "DEMO_DB_B",
        "host=db-b dbname=ecommerce user=demo password=demo",
    );
    std::env::set_var("DEMO_WORKLOAD", "/demo/workload.wkl");
    let config = DemoConfig::from_env();
    assert!(config.is_some());
    let c = config.unwrap();
    assert!(c.db_a.contains("db-a"));
    assert!(c.db_b.contains("db-b"));
    assert_eq!(c.workload_path, PathBuf::from("/demo/workload.wkl"));
    assert_eq!(c.init_sql_path, PathBuf::from("/demo/init-db-b.sql"));
    std::env::remove_var("PG_RETEST_DEMO");
    std::env::remove_var("DEMO_DB_A");
    std::env::remove_var("DEMO_DB_B");
    std::env::remove_var("DEMO_WORKLOAD");
}

#[test]
fn test_demo_config_default_workload_path() {
    let _guard = ENV_LOCK.lock().unwrap();
    std::env::set_var("PG_RETEST_DEMO", "true");
    std::env::set_var(
        "DEMO_DB_A",
        "host=db-a dbname=ecommerce user=demo password=demo",
    );
    std::env::set_var(
        "DEMO_DB_B",
        "host=db-b dbname=ecommerce user=demo password=demo",
    );
    std::env::remove_var("DEMO_WORKLOAD");
    let config = DemoConfig::from_env();
    assert!(config.is_some());
    let c = config.unwrap();
    assert_eq!(c.workload_path, PathBuf::from("/demo/workload.wkl"));
    assert_eq!(c.init_sql_path, PathBuf::from("/demo/init-db-b.sql"));
    std::env::remove_var("PG_RETEST_DEMO");
    std::env::remove_var("DEMO_DB_A");
    std::env::remove_var("DEMO_DB_B");
}
