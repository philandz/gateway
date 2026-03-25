use axum::{routing::get, Router};
use reqwest::Client;
use std::{env, net::SocketAddr, sync::Arc};
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use gateway::{AppState, IdentityTransport};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();

    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "gateway=debug,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    let monolith_url =
        env::var("UPSTREAM_URL").unwrap_or_else(|_| "http://localhost:8080".to_string());
    let identity_url =
        env::var("IDENTITY_URL").unwrap_or_else(|_| "http://127.0.0.1:3001".to_string());
    let identity_grpc_url =
        env::var("IDENTITY_GRPC_URL").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());
    let identity_transport = IdentityTransport::from_env(
        &env::var("IDENTITY_TRANSPORT").unwrap_or_else(|_| "grpc_transcode".to_string()),
    );
    let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".to_string());
    let port: u16 = env::var("PORT")
        .unwrap_or_else(|_| "3000".to_string())
        .parse()
        .unwrap_or(3000);

    let state = Arc::new(AppState {
        client: Client::new(),
        monolith_url,
        identity_url,
        identity_grpc_url,
        identity_transport,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods(Any)
        .allow_headers(Any);

    let x_request_id = axum::http::HeaderName::from_static("x-request-id");

    let api_router = match identity_transport {
        IdentityTransport::ProxyHttp => gateway::proxy::router_with_identity(),
        IdentityTransport::GrpcTranscode => Router::new()
            .nest("/identity", gateway::identity::router())
            .merge(gateway::proxy::router()),
    };

    let app = Router::new()
        .route("/health", get(|| async { "OK" }))
        .nest("/api", api_router)
        .merge(gateway::swagger::router())
        .layer(TraceLayer::new_for_http())
        .layer(cors)
        .layer(PropagateRequestIdLayer::new(x_request_id.clone()))
        .layer(SetRequestIdLayer::new(x_request_id, MakeRequestUuid))
        .with_state(state);

    let addr: SocketAddr = format!("{}:{}", host, port).parse()?;
    tracing::info!("API Gateway listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
