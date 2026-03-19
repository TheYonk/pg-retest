use pg_retest::web;

#[tokio::test]
async fn test_health_endpoint() {
    // Start server on random port
    let data_dir = tempfile::tempdir().unwrap();
    let data_path = data_dir.path().to_path_buf();

    let db_path = data_path.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    web::db::init_db(&conn).unwrap();

    let state = web::state::AppState::new(conn, data_path, None);
    let app = web::routes::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Give server a moment to start
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/v1/health", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["status"], "ok");
    assert_eq!(body["name"], "pg-retest");
}

#[tokio::test]
async fn test_workload_list_empty() {
    let data_dir = tempfile::tempdir().unwrap();
    let data_path = data_dir.path().to_path_buf();

    let db_path = data_path.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    web::db::init_db(&conn).unwrap();

    let state = web::state::AppState::new(conn, data_path, None);
    let app = web::routes::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/v1/workloads", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["workloads"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_runs_list_empty() {
    let data_dir = tempfile::tempdir().unwrap();
    let data_path = data_dir.path().to_path_buf();

    let db_path = data_path.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    web::db::init_db(&conn).unwrap();

    let state = web::state::AppState::new(conn, data_path, None);
    let app = web::routes::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/v1/runs", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["runs"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_tasks_endpoint() {
    let data_dir = tempfile::tempdir().unwrap();
    let data_path = data_dir.path().to_path_buf();

    let db_path = data_path.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    web::db::init_db(&conn).unwrap();

    let state = web::state::AppState::new(conn, data_path, None);
    let app = web::routes::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://{}/api/v1/tasks", addr))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(body["tasks"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn test_pipeline_validate() {
    let data_dir = tempfile::tempdir().unwrap();
    let data_path = data_dir.path().to_path_buf();

    let db_path = data_path.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    web::db::init_db(&conn).unwrap();

    let state = web::state::AppState::new(conn, data_path, None);
    let app = web::routes::build_router(state);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // Valid config
    let resp = client
        .post(format!("http://{}/api/v1/pipeline/validate", addr))
        .json(&serde_json::json!({
            "config_toml": "[replay]\ntarget = \"postgres://localhost/test\"\n"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], true);

    // Invalid config
    let resp = client
        .post(format!("http://{}/api/v1/pipeline/validate", addr))
        .json(&serde_json::json!({
            "config_toml": "not valid toml {{{"
        }))
        .send()
        .await
        .unwrap();

    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(body["valid"], false);
}

#[tokio::test]
async fn test_static_files_served() {
    let data_dir = tempfile::tempdir().unwrap();
    let data_path = data_dir.path().to_path_buf();

    let db_path = data_path.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    web::db::init_db(&conn).unwrap();

    let state = web::state::AppState::new(conn, data_path, None);
    let app = web::routes::build_router(state).fallback(web::static_handler);

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    let client = reqwest::Client::new();

    // Root should return index.html
    let resp = client
        .get(format!("http://{}/", addr))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let body = resp.text().await.unwrap();
    assert!(body.contains("pg-retest"));
}
