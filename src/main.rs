use axum::{routing::get, Router};
use reqwest::Client;
use std::{env, net::SocketAddr, sync::Arc};
use tower_http::cors::{Any, CorsLayer};
use tower_http::request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer};
use tower_http::trace::TraceLayer;

use gateway::{AppState, IdentityTransport};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    dotenvy::dotenv().ok();
    let rust_log = env::var("RUST_LOG").ok();
    philand_logging::init(
        "gateway",
        rust_log
            .as_deref()
            .or(Some("gateway=debug,tower_http=debug")),
    );

    let config = philand_configs::GatewayServiceConfig::from_env()
        .map_err(|e| anyhow::anyhow!("Failed to load config: {e}"))?;
    let identity_transport: IdentityTransport = config.identity_transport.into();

    let state = Arc::new(AppState {
        client: Client::new(),
        monolith_url: config.upstream_url,
        identity_url: config.identity_url,
        identity_grpc_url: config.identity_grpc_url,
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

    let addr: SocketAddr = format!("{}:{}", config.host, config.port).parse()?;
    tracing::info!("API Gateway listening on {}", addr);

    let listener = tokio::net::TcpListener::bind(&addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
