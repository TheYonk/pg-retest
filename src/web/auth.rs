use axum::{
    body::Body,
    http::{Request, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// Axum middleware that validates a bearer token on every request.
pub async fn require_auth(request: Request<Body>, next: Next) -> Response {
    let expected = request.extensions().get::<AuthToken>().map(|t| t.0.clone());

    let expected = match expected {
        Some(t) => t,
        None => return next.run(request).await,
    };

    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok());

    match auth_header {
        Some(header) if header.starts_with("Bearer ") => {
            let token = &header[7..];
            if token == expected {
                next.run(request).await
            } else {
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"error": "Invalid bearer token"})),
                )
                    .into_response()
            }
        }
        _ => (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": "Missing or invalid Authorization header. Use: Authorization: Bearer <token>"})),
        )
            .into_response(),
    }
}

#[derive(Clone)]
pub struct AuthToken(pub String);
