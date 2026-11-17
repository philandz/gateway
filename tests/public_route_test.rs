//! `/api/public/*` must reach the media service in grpc_transcode mode.
//!
//! Regression: the route used to exist only at the outer root, so Traefik's
//! untouched `/api/public/cat.mp4` was 404 in production while `/public/cat.mp4` worked.

use axum::http::{header, StatusCode};
use axum::routing::{any, get};
use axum::Router;
use gateway::{api_router, proxy, AppState, IdentityTransport};
use reqwest::Client;
use std::net::SocketAddr;
use std::sync::Arc;

const BODY: &[u8] = b"\x00\x00\x00\x18ftypmp42-fake-video";

/// Mimics the media service: serves one object at /public/cat.mp4.
async fn mock_media() -> SocketAddr {
    let app = Router::new().route(
        "/public/cat.mp4",
        get(|| async { ([(header::CONTENT_TYPE, "video/mp4")], BODY) }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

fn state(media: SocketAddr) -> Arc<AppState> {
    let dead = "http://127.0.0.1:1".to_string();
    Arc::new(AppState {
        client: Client::new(),
        monolith_url: dead.clone(),
        identity_url: dead.clone(),
        media_url: format!("http://{}", media),
        identity_grpc_url: dead.clone(),
        media_grpc_url: dead.clone(),
        budget_grpc_url: dead.clone(),
        portfolio_grpc_url: dead.clone(),
        category_grpc_url: dead.clone(),
        insight_grpc_url: dead.clone(),
        entry_grpc_url: dead.clone(),
        sharing_grpc_url: dead.clone(),
        notification_grpc_url: dead,
        identity_transport: IdentityTransport::GrpcTranscode,
        jwt_secret: "test-secret".to_string(),
    })
}

/// Same shape as main.rs: outer `/public` for backwards compatibility plus `/api`.
async fn spawn_gateway(transport: IdentityTransport) -> SocketAddr {
    let media = mock_media().await;
    let app = Router::new()
        .route("/public/{*path}", any(proxy::media_public_proxy_handler))
        .nest("/api", api_router(transport))
        .with_state(state(media));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

async fn fetch(gw: SocketAddr, path: &str) -> (StatusCode, Option<String>, Vec<u8>) {
    let res = Client::new()
        .get(format!("http://{}{}", gw, path))
        .send()
        .await
        .unwrap();
    let status = res.status();
    let ctype = res
        .headers()
        .get(header::CONTENT_TYPE)
        .map(|v| v.to_str().unwrap().to_string());
    (status, ctype, res.bytes().await.unwrap().to_vec())
}

#[tokio::test]
async fn api_public_is_served_in_grpc_transcode_mode() {
    let gw = spawn_gateway(IdentityTransport::GrpcTranscode).await;
    let (status, ctype, body) = fetch(gw, "/api/public/cat.mp4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype.as_deref(), Some("video/mp4"));
    assert_eq!(body, BODY);
}

#[tokio::test]
async fn root_public_still_works() {
    let gw = spawn_gateway(IdentityTransport::GrpcTranscode).await;
    let (status, ctype, body) = fetch(gw, "/public/cat.mp4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(ctype.as_deref(), Some("video/mp4"));
    assert_eq!(body, BODY);
}

#[tokio::test]
async fn api_public_is_served_in_proxy_mode() {
    let gw = spawn_gateway(IdentityTransport::ProxyHttp).await;
    let (status, _, body) = fetch(gw, "/api/public/cat.mp4").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, BODY);
}

#[tokio::test]
async fn unknown_public_object_is_not_found() {
    let gw = spawn_gateway(IdentityTransport::GrpcTranscode).await;
    let (status, _, _) = fetch(gw, "/api/public/missing.mp4").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
