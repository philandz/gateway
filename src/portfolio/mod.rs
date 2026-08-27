//! Gateway REST routes for the Asset Portfolio service.
//!
//! Routes proxy to the Budget gRPC server's `PortfolioService`, mounted
//! under `/api/portfolios/budgets/{budget_id}/portfolio/...` per the Asset
//! Portfolio spec §11.4. Proto messages are passed through as JSON.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post},
    Json, Router,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::Deserialize;
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest};
use tracing::warn;

use crate::pb::service::portfolio as pb;
use crate::pb::service::portfolio::portfolio_service_client::PortfolioServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

/// Serialize a proto message to JSON, logging instead of silently
/// swallowing serialization errors.
fn to_json_or_log<T: serde::Serialize>(value: &T, ctx: &str) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or_else(|e| {
        warn!("portfolio JSON serialize failed for {ctx}: {e}");
        serde_json::Value::Null
    })
}

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/budgets/{budget_id}/portfolio/summary",
            get(get_portfolio_summary),
        )
        .route("/budgets/{budget_id}/portfolio/assets", get(list_assets))
        .route(
            "/budgets/{budget_id}/portfolio/assets/{asset_id}",
            get(get_asset).patch(update_asset_metadata),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/{asset_id}/archive",
            post(archive_asset),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/{asset_id}/observations",
            get(list_observations).post(record_price_observation),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/{asset_id}/activity",
            get(list_activity),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/savings-account",
            post(create_savings_account),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/fixed-deposit",
            post(create_fixed_deposit),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/gold-lot",
            post(create_gold_lot),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/stock-lot",
            post(create_stock_lot),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/etf-lot",
            post(create_etf_lot),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/crypto-lot",
            post(create_crypto_lot),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets/{asset_id}/stock-disposal",
            post(record_stock_disposal),
        )
}

#[derive(Debug, Deserialize)]
pub struct PageQuery {
    #[serde(default)]
    pub page: Option<i32>,
    #[serde(default)]
    pub page_size: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct LimitQuery {
    #[serde(default)]
    pub limit: Option<i32>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SourceQuery {
    /// Selects the data source: "portfolio" (new schema only),
    /// "legacy" (invest_assets fallback), or "auto" (default — try
    /// portfolio first, fall back to legacy when empty).
    #[serde(default)]
    pub source: Option<String>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn get_portfolio_summary(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    _q: Query<SourceQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    if _q.source.as_deref() == Some("legacy") {
        return legacy_portfolio_summary(&state, &budget_id, &headers).await;
    }
    let mut client = connect(&state).await?;
    let req = make_req(
        &headers,
        pb::GetPortfolioSummaryRequest {
            budget_id: budget_id.clone(),
        },
    )?;
    let resp = client
        .get_portfolio_summary(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(to_json_or_log(
        &resp.into_inner(),
        "GetPortfolioSummaryResponse",
    )))
}

async fn list_assets(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(p): Query<PageQuery>,
    _s: Query<SourceQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    if _s.source.as_deref() == Some("legacy") {
        return legacy_list_assets(&state, &budget_id, &headers).await;
    }
    let mut client = connect(&state).await?;
    let req = make_req(
        &headers,
        pb::ListAssetsRequest {
            budget_id: budget_id.clone(),
            page: p.page.unwrap_or(0),
            page_size: p.page_size.unwrap_or(0),
        },
    )?;
    let resp = client.list_assets(req).await.map_err(into_api_err)?;
    Ok(Json(to_json_or_log(
        &resp.into_inner(),
        "ListAssetsResponse",
    )))
}

/// Legacy monolith is gone (strangler complete); return 503 so frontend
/// sees a clear error instead of crashing on the missing upstream.
async fn legacy_portfolio_summary(
    _state: &Arc<AppState>,
    _budget_id: &str,
    _headers: &HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            code: "legacy_removed".into(),
            message: "Legacy invest_assets monolith is no longer available.".into(),
            details: vec![],
        }),
    ))
}

async fn legacy_list_assets(
    _state: &Arc<AppState>,
    _budget_id: &str,
    _headers: &HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    Err((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            code: "legacy_removed".into(),
            message: "Legacy invest_assets monolith is no longer available.".into(),
            details: vec![],
        }),
    ))
}

async fn get_asset(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::GetAssetResponse>> {
    let mut client = connect(&state).await?;
    let req = make_req(
        &headers,
        pb::GetAssetRequest {
            budget_id,
            asset_id,
        },
    )?;
    let resp = client.get_asset(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn update_asset_metadata(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<pb::UpdateAssetMetadataRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let req = make_req(
        &headers,
        pb::UpdateAssetMetadataRequest {
            budget_id,
            asset_id,
            display_name: body.display_name,
            notes: body.notes,
        },
    )?;
    let resp = client
        .update_asset_metadata(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn archive_asset(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let req = make_req(
        &headers,
        pb::ArchiveAssetRequest {
            budget_id,
            asset_id,
        },
    )?;
    let resp = client.archive_asset(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_savings_account(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<pb::CreateSavingsAccountRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(&headers, pb::CreateSavingsAccountRequest { ..body })?;
    let resp = client
        .create_savings_account(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_fixed_deposit(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<pb::CreateFixedDepositRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(&headers, pb::CreateFixedDepositRequest { ..body })?;
    let resp = client
        .create_fixed_deposit(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_gold_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<pb::CreateGoldLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(&headers, pb::CreateGoldLotRequest { ..body })?;
    let resp = client.create_gold_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_stock_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<pb::CreateStockLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(&headers, pb::CreateStockLotRequest { ..body })?;
    let resp = client.create_stock_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_etf_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<pb::CreateEtfLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(&headers, pb::CreateEtfLotRequest { ..body })?;
    let resp = client.create_etf_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_crypto_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<pb::CreateCryptoLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(&headers, pb::CreateCryptoLotRequest { ..body })?;
    let resp = client.create_crypto_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn record_price_observation(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut body): Json<pb::RecordPriceObservationRequest>,
) -> ApiResult<Json<pb::PriceObservation>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(
        &headers,
        pb::RecordPriceObservationRequest { asset_id, ..body },
    )?;
    let resp = client
        .record_price_observation(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn list_observations(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    Query(q): Query<LimitQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::ListPriceObservationsResponse>> {
    let mut client = connect(&state).await?;
    let req = make_req(
        &headers,
        pb::ListPriceObservationsRequest {
            budget_id,
            asset_id,
            limit: q.limit.unwrap_or(50),
        },
    )?;
    let resp = client
        .list_price_observations(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn list_activity(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    Query(q): Query<LimitQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::ListAssetActivityResponse>> {
    let mut client = connect(&state).await?;
    let req = make_req(
        &headers,
        pb::ListAssetActivityRequest {
            budget_id,
            asset_id,
            limit: q.limit.unwrap_or(50),
        },
    )?;
    let resp = client
        .list_asset_activity(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn record_stock_disposal(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut body): Json<pb::RecordStockDisposalRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(
        &headers,
        pb::RecordStockDisposalRequest { asset_id, ..body },
    )?;
    let resp = client
        .record_stock_disposal(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

// ---------------------------------------------------------------------------
// gRPC client + auth helpers
// ---------------------------------------------------------------------------

async fn connect(
    state: &AppState,
) -> Result<PortfolioServiceClient<Channel>, (StatusCode, Json<ErrorResponse>)> {
    let url = state.portfolio_grpc_url.clone();
    PortfolioServiceClient::connect(url)
        .await
        .map_err(|e| internal_error(format!("connect: {e}")))
}

/// Forward inbound HTTP bearer to the gRPC upstream, decoding the JWT to
/// populate x-user-id / x-user-type / x-service-actor metadata. Mirrors
/// `gateway::budget::with_user` so the budget PortfolioService impl
/// receives the same auth context as the rest of the budget routes.
fn make_req<T>(
    headers: &HeaderMap,
    body: T,
) -> Result<GrpcRequest<T>, (StatusCode, Json<ErrorResponse>)> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| unauth("Missing Authorization header"))?;

    let mut grpc_req = GrpcRequest::new(body);
    let value =
        MetadataValue::try_from(auth).map_err(|_| unauth("Invalid Authorization header"))?;
    grpc_req.metadata_mut().insert("authorization", value);

    // Strip any inbound service-actor header — clients never escalate.
    grpc_req.metadata_mut().remove("x-service-actor");

    if let Some(user_id) = decode_sub(auth) {
        if let Ok(v) = MetadataValue::try_from(user_id.as_str()) {
            grpc_req.metadata_mut().insert("x-user-id", v);
        }
    }
    if let Some(user_type) = decode_user_type(auth) {
        if let Ok(v) = MetadataValue::try_from(user_type.as_str()) {
            grpc_req.metadata_mut().insert("x-user-type", v);
        }
        if user_type == "super_admin" {
            if let Ok(v) = MetadataValue::try_from("true") {
                grpc_req.metadata_mut().insert("x-service-actor", v);
            }
        }
    }

    Ok(grpc_req)
}

fn unauth(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::UNAUTHORIZED,
        Json(ErrorResponse {
            code: "unauthenticated".into(),
            message: msg.to_string(),
            details: vec![],
        }),
    )
}

/// Decode the `sub` claim from a Bearer JWT without signature verification.
/// Matches the format the identity service signs with `JWT_SECRET`.
fn decode_sub(bearer: &str) -> Option<String> {
    let token = bearer.strip_prefix("Bearer ")?;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = base64_url_decode(parts[1])?;
    let v: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    v.get("sub").and_then(|v| v.as_str()).map(String::from)
}

fn decode_user_type(bearer: &str) -> Option<String> {
    let token = bearer.strip_prefix("Bearer ")?;
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return None;
    }
    let payload = base64_url_decode(parts[1])?;
    let v: serde_json::Value = serde_json::from_slice(&payload).ok()?;
    v.get("user_type")
        .and_then(|v| v.as_str())
        .map(String::from)
}

fn base64_url_decode(s: &str) -> Option<Vec<u8>> {
    URL_SAFE_NO_PAD.decode(s).ok()
}

fn into_api_err(e: tonic::Status) -> (StatusCode, Json<ErrorResponse>) {
    internal_error(format!("{e}"))
}

fn internal_error(msg: String) -> (StatusCode, Json<ErrorResponse>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(ErrorResponse {
            code: "INTERNAL".to_string(),
            message: msg,
            details: vec![],
        }),
    )
}
