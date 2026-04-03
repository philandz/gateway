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
        philand_configs::IdentityTransportMode::from_env_value(value).into()
    }
}

impl From<philand_configs::IdentityTransportMode> for IdentityTransport {
    fn from(value: philand_configs::IdentityTransportMode) -> Self {
        match value {
            philand_configs::IdentityTransportMode::ProxyHttp => Self::ProxyHttp,
            philand_configs::IdentityTransportMode::GrpcTranscode => Self::GrpcTranscode,
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
