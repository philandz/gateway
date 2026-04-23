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
        pub mod media {
            tonic::include_proto!("service.media");
        }
        pub mod budget {
            tonic::include_proto!("service.budget");
        }
        pub mod category {
            tonic::include_proto!("service.category");
        }
        pub mod entry {
            tonic::include_proto!("service.entry");
        }
        pub mod sharing {
            tonic::include_proto!("service.sharing");
        }
    }
    pub mod shared {
        pub mod user {
            tonic::include_proto!("shared.user");
        }
        pub mod organization {
            tonic::include_proto!("shared.organization");
        }
        pub mod media {
            tonic::include_proto!("shared.media");
        }
    }
}

pub mod budget;
pub mod category;
pub mod entry;
pub mod identity;
pub mod media;
pub mod middleware;
pub mod proxy;
pub mod sharing;
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
    /// Base URL for identity HTTP proxy fallback mode.
    pub identity_url: String,
    /// Base URL for media HTTP proxy fallback mode.
    pub media_url: String,
    /// Identity service gRPC endpoint.
    pub identity_grpc_url: String,
    /// Media service gRPC endpoint.
    pub media_grpc_url: String,
    /// Budget service gRPC endpoint.
    pub budget_grpc_url: String,
    /// Category service gRPC endpoint.
    pub category_grpc_url: String,
    /// Entry service gRPC endpoint.
    pub entry_grpc_url: String,
    /// Sharing service gRPC endpoint.
    pub sharing_grpc_url: String,
    pub identity_transport: IdentityTransport,
}
