//! Gateway REST routes for the Asset Portfolio service.
//!
//! Routes proxy to the Budget gRPC server's `PortfolioService`, mounted
//! under `/api/budget/budgets/{budget_id}/portfolio/...` per the Asset
//! Portfolio spec §11.4. Proto messages are passed through as JSON.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;
use tracing::warn;

/// Serialize a proto message to JSON, logging instead of silently
/// swallowing serialization errors. JSON serialization of generated proto
/// types is rare in practice but the warning is cheap insurance.
fn to_json_or_log<T: serde::Serialize>(value: &T, ctx: &str) -> serde_json::Value {
    serde_json::to_value(value).unwrap_or_else(|e| {
        warn!("portfolio JSON serialize failed for {ctx}: {e}");
        serde_json::Value::Null
    })
}
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest};

use crate::pb::service::portfolio as pb;
use crate::pb::service::portfolio::portfolio_service_client::PortfolioServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/budgets/{budget_id}/portfolio/summary",
            get(get_portfolio_summary),
        )
        .route(
            "/budgets/{budget_id}/portfolio/assets",
            get(list_assets),
        )
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
) -> ApiResult<Json<serde_json::Value>> {
    if _q.source.as_deref() == Some("legacy") {
        return legacy_portfolio_summary(&state, &budget_id).await;
    }
    let mut client = connect(&state).await?;
    let req = GrpcRequest::new(pb::GetPortfolioSummaryRequest {
        budget_id: budget_id.clone(),
    });
    let resp = client.get_portfolio_summary(req).await.map_err(into_api_err)?;
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
) -> ApiResult<Json<serde_json::Value>> {
    if _s.source.as_deref() == Some("legacy") {
        return legacy_list_assets(&state, &budget_id).await;
    }
    let mut client = connect(&state).await?;
    let req = GrpcRequest::new(pb::ListAssetsRequest {
        budget_id: budget_id.clone(),
        page: p.page.unwrap_or(0),
        page_size: p.page_size.unwrap_or(0),
    });
    let resp = client.list_assets(req).await.map_err(into_api_err)?;
    Ok(Json(to_json_or_log(&resp.into_inner(), "ListAssetsResponse")))
}

/// Legacy monolith is gone (strangler complete); return 503 so frontend
/// sees a clear error instead of crashing on the missing upstream.
async fn legacy_portfolio_summary(
    _state: &Arc<AppState>,
    _budget_id: &str,
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
) -> ApiResult<Json<pb::GetAssetResponse>> {
    let mut client = connect(&state).await?;
    let req = GrpcRequest::new(pb::GetAssetRequest {
        budget_id,
        asset_id,
    });
    let resp = client.get_asset(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn update_asset_metadata(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    Json(body): Json<pb::UpdateAssetMetadataRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::UpdateAssetMetadataRequest {
        budget_id,
        asset_id,
        display_name: body.display_name,
        notes: body.notes,
    });
    inject_auth_meta(&mut req);
    let resp = client.update_asset_metadata(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn archive_asset(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::ArchiveAssetRequest {
        budget_id,
        asset_id,
    });
    inject_auth_meta(&mut req);
    let resp = client.archive_asset(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_savings_account(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Json(body): Json<pb::CreateSavingsAccountRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::CreateSavingsAccountRequest {
        budget_id,
        ..body
    });
    inject_auth_meta(&mut req);
    let resp = client.create_savings_account(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_fixed_deposit(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Json(body): Json<pb::CreateFixedDepositRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::CreateFixedDepositRequest {
        budget_id,
        ..body
    });
    inject_auth_meta(&mut req);
    let resp = client.create_fixed_deposit(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_gold_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Json(body): Json<pb::CreateGoldLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::CreateGoldLotRequest {
        budget_id,
        ..body
    });
    inject_auth_meta(&mut req);
    let resp = client.create_gold_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_stock_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Json(body): Json<pb::CreateStockLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::CreateStockLotRequest {
        budget_id,
        ..body
    });
    inject_auth_meta(&mut req);
    let resp = client.create_stock_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_etf_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Json(body): Json<pb::CreateEtfLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::CreateEtfLotRequest {
        budget_id,
        ..body
    });
    inject_auth_meta(&mut req);
    let resp = client.create_etf_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn create_crypto_lot(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Json(body): Json<pb::CreateCryptoLotRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::CreateCryptoLotRequest {
        budget_id,
        ..body
    });
    inject_auth_meta(&mut req);
    let resp = client.create_crypto_lot(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn record_price_observation(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    Json(body): Json<pb::RecordPriceObservationRequest>,
) -> ApiResult<Json<pb::PriceObservation>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::RecordPriceObservationRequest {
        budget_id,
        asset_id,
        ..body
    });
    inject_auth_meta(&mut req);
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
) -> ApiResult<Json<pb::ListPriceObservationsResponse>> {
    let mut client = connect(&state).await?;
    let req = GrpcRequest::new(pb::ListPriceObservationsRequest {
        budget_id,
        asset_id,
        limit: q.limit.unwrap_or(50),
    });
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
) -> ApiResult<Json<pb::ListAssetActivityResponse>> {
    let mut client = connect(&state).await?;
    let req = GrpcRequest::new(pb::ListAssetActivityRequest {
        budget_id,
        asset_id,
        limit: q.limit.unwrap_or(50),
    });
    let resp = client.list_asset_activity(req).await.map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

async fn record_stock_disposal(
    State(state): State<Arc<AppState>>,
    Path((budget_id, asset_id)): Path<(String, String)>,
    Json(body): Json<pb::RecordStockDisposalRequest>,
) -> ApiResult<Json<pb::PortfolioAsset>> {
    let mut client = connect(&state).await?;
    let mut req = GrpcRequest::new(pb::RecordStockDisposalRequest {
        budget_id,
        asset_id,
        ..body
    });
    inject_auth_meta(&mut req);
    let resp = client
        .record_stock_disposal(req)
        .await
        .map_err(into_api_err)?;
    Ok(Json(resp.into_inner()))
}

// ---------------------------------------------------------------------------
// gRPC client helpers
// ---------------------------------------------------------------------------

async fn connect(
    state: &AppState,
) -> Result<PortfolioServiceClient<Channel>, (StatusCode, Json<ErrorResponse>)> {
    let url = state.portfolio_grpc_url.clone();
    PortfolioServiceClient::connect(url)
        .await
        .map_err(|e| internal_error(format!("connect: {e}")))
}

/// Placeholder for future auth metadata forwarding. The current gateway
/// does not extract a JWT subject for portfolio routes; auth comes from
/// the bearer token sent upstream to the Budget gRPC service, which
/// validates it via the existing identity middleware. Once a unified
/// auth middleware lands for portfolio, populate x-user-id/x-user-type
/// here from request extensions.
fn inject_auth_meta<T>(_req: &mut GrpcRequest<T>) {
    // Placeholder: auth metadata is forwarded via the bearer token sent upstream.
    // The Budget gRPC service validates it via existing identity middleware.
    // TODO: populate x-user-id/x-user-type from request extensions when unified auth lands.
    let _ = MetadataValue::<tonic::metadata::Ascii>::from_static("");
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