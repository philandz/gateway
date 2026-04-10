use axum::extract::Request;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::Router;
use gateway::{proxy, AppState, IdentityTransport};
use reqwest::Client;
use std::net::SocketAddr;
use std::sync::Arc;

/// Spins up a mock monolith server with multiple endpoints for testing.
async fn mock_monolith() -> SocketAddr {
    let app = Router::new()
        .route("/hello", get(|| async { "world" }))
        .route(
            "/echo-ip",
            get(|req: Request| async move {
                req.headers()
                    .get("X-Forwarded-For")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_else(|| "none".to_string())
            }),
        )
        .route(
            "/echo-body",
            post(|body: axum::body::Bytes| async move { body }),
        )
        .route(
            "/query",
            get(|req: Request| async move { req.uri().query().unwrap_or("no-query").to_string() }),
        )
        .route(
            "/status/201",
            get(|| async { (StatusCode::CREATED, "created") }),
        )
        .route(
            "/echo-headers",
            get(|req: Request| async move {
                req.headers()
                    .get("Authorization")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_else(|| "none".to_string())
            }),
        );

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    addr
}

/// Spins up a Gateway proxy backed by the given monolith address.
async fn spawn_proxy(monolith_addr: SocketAddr) -> SocketAddr {
    let state = Arc::new(AppState {
        client: Client::new(),
        monolith_url: format!("http://{}", monolith_addr),
        identity_url: "http://127.0.0.1:1".to_string(), // unused in monolith tests
        media_url: "http://127.0.0.1:1".to_string(),
        identity_grpc_url: "http://127.0.0.1:1".to_string(),
        identity_transport: IdentityTransport::ProxyHttp,
    });

    let proxy_app = proxy::router().with_state(state);
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(proxy_listener, proxy_app).await.unwrap();
    });

    proxy_addr
}

#[tokio::test]
async fn test_proxy_forwards_get_request() {
    let mono = mock_monolith().await;
    let gw = spawn_proxy(mono).await;
    let client = Client::new();

    let res = client
        .get(format!("http://{}/hello", gw))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "world");
}

#[tokio::test]
async fn test_proxy_forwards_x_forwarded_for() {
    let mono = mock_monolith().await;
    let gw = spawn_proxy(mono).await;
    let client = Client::new();

    let res = client
        .get(format!("http://{}/echo-ip", gw))
        .header("X-Forwarded-For", "10.0.0.1")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "10.0.0.1");
}

#[tokio::test]
async fn test_proxy_forwards_post_body() {
    let mono = mock_monolith().await;
    let gw = spawn_proxy(mono).await;
    let client = Client::new();

    let payload = r#"{"amount":42}"#;
    let res = client
        .post(format!("http://{}/echo-body", gw))
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), payload);
}

#[tokio::test]
async fn test_proxy_preserves_query_string() {
    let mono = mock_monolith().await;
    let gw = spawn_proxy(mono).await;
    let client = Client::new();

    let res = client
        .get(format!("http://{}/query?page=2&limit=10", gw))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "page=2&limit=10");
}

#[tokio::test]
async fn test_proxy_preserves_upstream_status_code() {
    let mono = mock_monolith().await;
    let gw = spawn_proxy(mono).await;
    let client = Client::new();

    let res = client
        .get(format!("http://{}/status/201", gw))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 201);
    assert_eq!(res.text().await.unwrap(), "created");
}

#[tokio::test]
async fn test_proxy_forwards_authorization_header() {
    let mono = mock_monolith().await;
    let gw = spawn_proxy(mono).await;
    let client = Client::new();

    let res = client
        .get(format!("http://{}/echo-headers", gw))
        .header("Authorization", "Bearer test-jwt-token")
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    assert_eq!(res.text().await.unwrap(), "Bearer test-jwt-token");
}

#[tokio::test]
async fn test_proxy_returns_bad_gateway_on_unreachable_upstream() {
    let state = Arc::new(AppState {
        client: Client::new(),
        monolith_url: "http://127.0.0.1:1".to_string(),
        identity_url: "http://127.0.0.1:1".to_string(),
        media_url: "http://127.0.0.1:1".to_string(),
        identity_grpc_url: "http://127.0.0.1:1".to_string(),
        identity_transport: IdentityTransport::ProxyHttp,
    });

    let proxy_app = proxy::router().with_state(state);
    let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let proxy_addr = proxy_listener.local_addr().unwrap();

    tokio::spawn(async move {
        axum::serve(proxy_listener, proxy_app).await.unwrap();
    });

    let client = Client::new();
    let res = client
        .get(format!("http://{}/anything", proxy_addr))
        .send()
        .await
        .unwrap();
    assert_eq!(res.status(), 502);
}
