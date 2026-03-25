pub mod pb {
    pub mod common {
        pub mod base {
            tonic::include_proto!("common.base");
        }
    }
    pub mod service {
        pub mod identity {
            tonic::include_proto!("service.identity");
        }
    }
    pub mod shared {
        pub mod user {
            tonic::include_proto!("shared.user");
        }
        pub mod organization {
            tonic::include_proto!("shared.organization");
        }
    }
}

pub mod identity;
pub mod proxy;
pub mod swagger;

use reqwest::Client;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IdentityTransport {
    ProxyHttp,
    GrpcTranscode,
}

impl IdentityTransport {
    pub fn from_env(value: &str) -> Self {
        match value {
            "grpc_transcode" => Self::GrpcTranscode,
            _ => Self::ProxyHttp,
        }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub client: Client,
    pub monolith_url: String,
    /// Base URL for identity HTTP transport (used in `proxy_http` fallback mode).
    pub identity_url: String,
    /// Identity service gRPC endpoint URL (e.g., "http://127.0.0.1:50051")
    pub identity_grpc_url: String,
    pub identity_transport: IdentityTransport,
}
