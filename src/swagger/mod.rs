use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use utoipa_swagger_ui::{Config, SwaggerUi, Url};

use crate::AppState;

/// Placeholder OpenAPI spec served directly by the gateway.
/// As microservices come online, replace these with real downstream specs.
async fn gateway_openapi_spec() -> Json<Value> {
    Json(json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Philand API Gateway",
            "description": "Unified API surface for the Philand microservice mesh. Services will appear here as they are extracted from the monolith.",
            "version": "0.1.0"
        },
        "paths": {
            "/health": {
                "get": {
                    "summary": "Health check",
                    "responses": {
                        "200": { "description": "OK" }
                    }
                }
            }
        }
    }))
}

/// Serves identity OpenAPI spec.
///
/// Priority:
/// 1) Static generated spec (`gateway/src/swagger/specs/identity.json`) when rich enough.
/// 2) Live identity service `/api-docs/openapi.json` fallback (keeps Swagger usable in dev).
/// 3) Stub spec when neither is available.
async fn identity_openapi_spec(State(state): State<Arc<AppState>>) -> Json<Value> {
    if let Some(spec) = read_static_identity_spec() {
        if has_rich_schemas(&spec) {
            return Json(with_gateway_server(spec));
        }
    }

    let live_url = format!("{}/api-docs/openapi.json", state.identity_url);
    if let Ok(spec) = philand_http::get_json::<Value>(&state.client, &live_url).await {
        return Json(with_gateway_server(spec));
    }

    if let Some(spec) = read_static_identity_spec() {
        return Json(with_gateway_server(spec));
    }

    Json(identity_stub_spec())
}

fn read_static_identity_spec() -> Option<Value> {
    let raw = include_str!("specs/identity.json");
    serde_json::from_str::<Value>(raw).ok()
}

fn with_gateway_server(mut spec: Value) -> Value {
    spec["servers"] = json!([{ "url": "/api/identity" }]);
    spec
}

fn has_rich_schemas(spec: &Value) -> bool {
    spec.pointer("/components/schemas")
        .and_then(Value::as_object)
        .is_some_and(|schemas| !schemas.is_empty())
}

fn identity_stub_spec() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Philand Identity Service",
            "description": "Identity service spec (service currently unavailable — showing stub)",
            "version": "0.1.0"
        },
        "paths": {}
    })
}

/// Creates the Swagger UI router with specs for all available services.
pub fn router() -> Router<Arc<AppState>> {
    let swagger_urls = vec![
        Url::new("Gateway", "/gateway/api-docs/openapi.json"),
        Url::new("Identity", "/identity/api-docs/openapi.json"),
    ];

    let config = Config::new(swagger_urls);

    Router::new()
        .route("/gateway/api-docs/openapi.json", get(gateway_openapi_spec))
        .route(
            "/identity/api-docs/openapi.json",
            get(identity_openapi_spec),
        )
        .merge(SwaggerUi::new("/docs").config(config))
}
