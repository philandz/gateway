use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    response::Response,
    routing::any,
    Router,
};
use std::sync::Arc;

use crate::AppState;

/// Creates a proxy router that forwards identity and monolith paths via HTTP.
pub fn router_with_identity() -> Router<Arc<AppState>> {
    Router::new()
        .route("/identity/{*path}", any(identity_proxy_handler))
        .route("/media/{*path}", any(media_proxy_handler))
        .route("/public/{*path}", any(media_public_proxy_handler))
        .route("/{*path}", any(monolith_proxy_handler))
}

/// Creates the fallback proxy router for monolith-only paths.
pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/media/{*path}", any(media_proxy_handler))
        .route("/public/{*path}", any(media_public_proxy_handler))
        .route("/{*path}", any(monolith_proxy_handler))
}

async fn identity_proxy_handler(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    let downstream_path = path.strip_prefix("/identity").unwrap_or(path);
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let uri = format!("{}{}{}", state.identity_url, downstream_path, query);

    forward_request(&state, req, &uri).await
}

async fn media_proxy_handler(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    let downstream_path = path.strip_prefix("/media").unwrap_or(path);
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let uri = format!("{}{}{}", state.media_url, downstream_path, query);

    forward_request(&state, req, &uri).await
}

async fn media_public_proxy_handler(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    let downstream_path = path.strip_prefix("/public").unwrap_or(path);
    let query = req
        .uri()
        .query()
        .map(|q| format!("?{}", q))
        .unwrap_or_default();
    let uri = format!("{}/public{}{}", state.media_url, downstream_path, query);

    forward_request(&state, req, &uri).await
}

/// Proxy handler for all other routes — forwards to the monolith.
async fn monolith_proxy_handler(
    State(state): State<Arc<AppState>>,
    req: Request,
) -> Result<Response, StatusCode> {
    let path = req.uri().path();
    let path_query = req
        .uri()
        .path_and_query()
        .map(|v| v.as_str())
        .unwrap_or(path);

    let uri = format!("{}{}", state.monolith_url, path_query);

    forward_request(&state, req, &uri).await
}

/// Shared logic: convert an incoming Axum request into a reqwest request,
/// send it upstream, and map the response back.
async fn forward_request(
    state: &AppState,
    req: Request,
    uri: &str,
) -> Result<Response, StatusCode> {
    let mut proxy_req = state.client.request(req.method().clone(), uri);

    // Forward all headers except Host (reqwest sets it for the upstream)
    for (name, value) in req.headers() {
        if name != reqwest::header::HOST {
            proxy_req = proxy_req.header(name.clone(), value.clone());
        }
    }

    // Buffer and forward body
    let body_bytes = axum::body::to_bytes(req.into_body(), usize::MAX)
        .await
        .map_err(|_| StatusCode::BAD_REQUEST)?;
    proxy_req = proxy_req.body(body_bytes);

    // Execute the proxied request
    let res = proxy_req.send().await.map_err(|e| {
        tracing::error!("Proxy error: {}", e);
        StatusCode::BAD_GATEWAY
    })?;

    // Map reqwest::Response → axum::response::Response
    let mut response_builder = Response::builder().status(res.status());

    for (name, value) in res.headers() {
        response_builder = response_builder.header(name.clone(), value.clone());
    }

    let response_body = res
        .bytes()
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let body = Body::from(response_body);

    response_builder
        .body(body)
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
}
