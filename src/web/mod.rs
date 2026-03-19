pub mod db;
pub mod handlers;
pub mod routes;
pub mod state;
pub mod tasks;
pub mod ws;

use std::path::PathBuf;

use anyhow::Result;
use axum::{
    body::Body,
    http::{header, Response, StatusCode},
    response::IntoResponse,
};
use rust_embed::Embed;

use self::state::AppState;

/// Static files embedded from src/web/static/
#[derive(Embed)]
#[folder = "src/web/static/"]
struct StaticAssets;

/// Serve embedded static files, falling back to index.html for SPA routing.
pub async fn static_handler(uri: axum::http::Uri) -> impl IntoResponse {
    let path = uri.path().trim_start_matches('/');

    // Try exact path first
    if let Some(file) = StaticAssets::get(path) {
        let mime = mime_guess::from_path(path)
            .first_or_octet_stream()
            .to_string();
        return Response::builder()
            .header(header::CONTENT_TYPE, mime)
            .body(Body::from(file.data.to_vec()))
            .unwrap();
    }

    // SPA fallback: serve index.html for non-API routes
    if let Some(file) = StaticAssets::get("index.html") {
        return Response::builder()
            .header(header::CONTENT_TYPE, "text/html")
            .body(Body::from(file.data.to_vec()))
            .unwrap();
    }

    Response::builder()
        .status(StatusCode::NOT_FOUND)
        .body(Body::from("Not Found"))
        .unwrap()
}

/// Auto-import the demo workload into SQLite if it hasn't been imported yet.
fn import_demo_workload(
    conn: &rusqlite::Connection,
    dc: &state::DemoConfig,
    data_dir: &std::path::Path,
) -> Result<()> {
    use crate::profile::io::read_profile;

    // Check if already imported
    let existing = db::list_workloads(conn)?;
    if existing.iter().any(|w| w.name == "demo-ecommerce") {
        return Ok(());
    }

    let profile = read_profile(&dc.workload_path)?;
    let dest = data_dir.join("workloads").join("demo-ecommerce.wkl");
    if !dest.exists() {
        std::fs::create_dir_all(dest.parent().unwrap())?;
        std::fs::copy(&dc.workload_path, &dest)?;
    }

    let row = db::WorkloadRow {
        id: uuid::Uuid::new_v4().to_string(),
        name: "demo-ecommerce".to_string(),
        file_path: dest.to_string_lossy().to_string(),
        source_type: Some(if profile.capture_method.is_empty() {
            "demo".to_string()
        } else {
            profile.capture_method.clone()
        }),
        source_host: Some(profile.source_host.clone()),
        captured_at: Some(profile.captured_at.to_rfc3339()),
        total_sessions: Some(profile.sessions.len() as i64),
        total_queries: Some(
            profile
                .sessions
                .iter()
                .map(|s| s.queries.len() as i64)
                .sum(),
        ),
        capture_duration_us: None,
        classification: None,
        created_at: None,
    };
    db::insert_workload(conn, &row)?;
    Ok(())
}

/// Start the web server on the given port.
pub async fn run_server(port: u16, data_dir: PathBuf) -> Result<()> {
    // Ensure data directory exists
    std::fs::create_dir_all(&data_dir)?;

    // Initialize SQLite
    let db_path = data_dir.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path)?;
    db::init_db(&conn)?;

    // Parse demo configuration from environment
    let demo_config = state::DemoConfig::from_env();

    if let Some(ref dc) = demo_config {
        println!("Demo mode enabled:");
        println!("  DB A: {}", dc.db_a);
        println!("  DB B: {}", dc.db_b);
        println!("  Workload: {}", dc.workload_path.display());

        // Auto-import the demo workload if the file exists
        if dc.workload_path.exists() {
            match import_demo_workload(&conn, dc, &data_dir) {
                Ok(()) => println!("  Demo workload imported successfully"),
                Err(e) => eprintln!("  Warning: failed to import demo workload: {e}"),
            }
        } else {
            println!(
                "  Demo workload not found at {} (will import when available)",
                dc.workload_path.display()
            );
        }
    }

    let state = AppState::new(conn, data_dir.clone(), demo_config);

    // Build router: API routes + static file fallback
    let app = routes::build_router(state).fallback(static_handler);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    println!("pg-retest web dashboard: http://localhost:{port}");
    println!("Data directory: {}", data_dir.display());

    axum::serve(listener, app).await?;
    Ok(())
}
