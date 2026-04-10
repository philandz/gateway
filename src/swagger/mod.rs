use axum::{extract::State, routing::get, Json, Router};
use serde_json::{json, Value};
use std::sync::Arc;
use utoipa_swagger_ui::{Config, SwaggerUi, Url};

use crate::AppState;

/// Placeholder OpenAPI spec served directly by the gateway.
/// As microservices come online, replace these with real downstream specs.
async fn gateway_openapi_spec() -> Json<Value> {
    Json(with_bearer_auth(json!({
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
    })))
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
            return Json(with_bearer_auth(with_gateway_server(spec)));
        }
    }

    let live_url = format!("{}/api-docs/openapi.json", state.identity_url);
    if let Ok(spec) = philand_http::get_json::<Value>(&state.client, &live_url).await {
        return Json(with_bearer_auth(with_gateway_server(spec)));
    }

    if let Some(spec) = read_static_identity_spec() {
        return Json(with_bearer_auth(with_gateway_server(spec)));
    }

    Json(with_bearer_auth(identity_stub_spec()))
}

async fn media_openapi_spec() -> Json<Value> {
    Json(with_bearer_auth(media_stub_spec()))
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

fn media_stub_spec() -> Value {
    json!({
        "openapi": "3.0.3",
        "info": {
            "title": "Philand Media Service",
            "description": "Media upload and metadata APIs exposed via gateway",
            "version": "0.1.0"
        },
        "servers": [
            { "url": "/api/media" }
        ],
        "paths": {
            "/uploads/init": {
                "post": {
                    "summary": "Initialize media upload",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["file_name", "content_type", "size"],
                                    "properties": {
                                        "file_name": { "type": "string", "example": "avatar.jpg" },
                                        "content_type": { "type": "string", "example": "image/jpeg" },
                                        "size": { "type": "integer", "format": "int64", "example": 1024 }
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "201": { "description": "Upload initialized" },
                        "400": { "description": "Invalid request" },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/uploads/complete": {
                "post": {
                    "summary": "Complete media upload",
                    "requestBody": {
                        "required": true,
                        "content": {
                            "application/json": {
                                "schema": {
                                    "type": "object",
                                    "required": ["upload_id"],
                                    "properties": {
                                        "upload_id": { "type": "string", "example": "6956b4e0-ffd3-478f-910e-e8dc8e3527f8" }
                                    }
                                }
                            }
                        }
                    },
                    "responses": {
                        "200": { "description": "Upload completed" },
                        "400": { "description": "Invalid request" },
                        "401": { "description": "Unauthorized" }
                    }
                }
            },
            "/files/{id}": {
                "get": {
                    "summary": "Get media file metadata",
                    "parameters": [
                        {
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": { "type": "string" }
                        }
                    ],
                    "responses": {
                        "200": { "description": "Media metadata" },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "File not found" }
                    }
                }
            },
            "/files/{id}/download-url": {
                "get": {
                    "summary": "Get temporary download URL for media file",
                    "parameters": [
                        {
                            "name": "id",
                            "in": "path",
                            "required": true,
                            "schema": { "type": "string" }
                        }
                    ],
                    "responses": {
                        "200": {
                            "description": "Temporary presigned download URL",
                            "content": {
                                "application/json": {
                                    "schema": {
                                        "type": "object",
                                        "required": ["file_id", "download_url", "expires_at"],
                                        "properties": {
                                            "file_id": { "type": "string", "example": "99127277-2064-4956-8667-b1531641835d" },
                                            "download_url": { "type": "string", "format": "uri", "example": "https://s3.philand.io.vn/philand-v2-media/path/to/object?X-Amz-..." },
                                            "expires_at": { "type": "integer", "format": "int64", "example": 1775812768 }
                                        }
                                    }
                                }
                            }
                        },
                        "401": { "description": "Unauthorized" },
                        "404": { "description": "File not found" }
                    },
                    "security": [
                        { "bearerAuth": [] }
                    ]
                }
            }
        }
    })
}

fn with_bearer_auth(mut spec: Value) -> Value {
    spec["components"]["securitySchemes"]["bearerAuth"] = json!({
        "type": "http",
        "scheme": "bearer",
        "bearerFormat": "JWT"
    });

    if spec.get("security").is_none() || spec["security"].is_null() {
        spec["security"] = json!([
            { "bearerAuth": [] }
        ]);
    }

    spec
}

/// Creates the Swagger UI router with specs for all available services.
pub fn router() -> Router<Arc<AppState>> {
    let swagger_urls = vec![
        Url::new("Gateway", "/swagger/gateway/openapi.json"),
        Url::new("Identity", "/swagger/identity/openapi.json"),
        Url::new("Media", "/swagger/media/openapi.json"),
    ];

    let config = Config::new(swagger_urls);

    Router::new()
        .route("/swagger/gateway/openapi.json", get(gateway_openapi_spec))
        .route("/swagger/identity/openapi.json", get(identity_openapi_spec))
        .route("/swagger/media/openapi.json", get(media_openapi_spec))
        .merge(SwaggerUi::new("/swagger").config(config))
}
