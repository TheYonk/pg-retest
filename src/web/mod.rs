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

/// Start the web server on the given port.
pub async fn run_server(port: u16, data_dir: PathBuf) -> Result<()> {
    // Ensure data directory exists
    std::fs::create_dir_all(&data_dir)?;

    // Initialize SQLite
    let db_path = data_dir.join("pg-retest.db");
    let conn = rusqlite::Connection::open(&db_path)?;
    db::init_db(&conn)?;

    let state = AppState::new(conn, data_dir.clone());

    // Build router: API routes + static file fallback
    let app = routes::build_router(state).fallback(static_handler);

    let addr = format!("0.0.0.0:{port}");
    let listener = tokio::net::TcpListener::bind(&addr).await?;

    println!("pg-retest web dashboard: http://localhost:{port}");
    println!("Data directory: {}", data_dir.display());

    axum::serve(listener, app).await?;
    Ok(())
}
