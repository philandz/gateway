use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, put},
    Json, Router,
};
use serde::Deserialize;
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::budget as pb;
use crate::pb::service::budget::budget_service_client::BudgetServiceClient;
use crate::AppState;
use axum::routing::post;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/budgets", get(list_budgets).post(create_budget))
        .route(
            "/budgets/{budget_id}",
            get(get_budget).patch(update_budget).delete(delete_budget),
        )
        .route(
            "/budgets/{budget_id}/members",
            get(list_members).post(add_member),
        )
        .route(
            "/budgets/{budget_id}/members/{user_id}/role",
            patch(update_member_role),
        )
        .route(
            "/budgets/{budget_id}/members/{user_id}",
            delete(remove_member),
        )
        .route("/budgets/{budget_id}/envelope", put(set_envelope_limit))
        .route("/budgets/{budget_id}/burn-rate", get(get_burn_rate))
        .route(
            "/budgets/{budget_id}/rollover",
            put(set_rollover_policy).get(get_rollover_policy),
        )
        .route("/templates", get(list_templates))
        // Invest assets
        .route(
            "/budgets/{budget_id}/invest/assets",
            get(list_invest_assets).post(create_invest_asset),
        )
        .route(
            "/budgets/{budget_id}/invest/assets/{asset_id}",
            patch(update_invest_asset).delete(delete_invest_asset),
        )
        .route(
            "/budgets/{budget_id}/invest/portfolio",
            get(get_invest_portfolio_summary),
        )
        // Price snapshots
        .route(
            "/invest/assets/{asset_id}/snapshots",
            get(list_price_snapshots).post(add_price_snapshot),
        )
        .route(
            "/invest/assets/{asset_id}/snapshots/latest",
            get(get_latest_price_snapshot),
        )
}

fn map_status(status: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, envelope) = philand_error::http_error_from_tonic_status(&status);
    (http, Json(envelope))
}

async fn client(state: &AppState) -> ApiResult<BudgetServiceClient<Channel>> {
    BudgetServiceClient::connect(state.budget_grpc_url.clone())
        .await
        .map_err(|e| map_status(Status::internal(e.to_string())))
}

fn with_user<T>(headers: &HeaderMap, req: T) -> ApiResult<GrpcRequest<T>> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| map_status(Status::unauthenticated("Missing Authorization header")))?;

    let mut grpc_req = GrpcRequest::new(req);
    let value = MetadataValue::try_from(auth)
        .map_err(|_| map_status(Status::unauthenticated("Invalid Authorization header")))?;
    grpc_req.metadata_mut().insert("authorization", value);

    // Decode JWT payload to extract sub (user_id) and inject as x-user-id
    if let Some(user_id) = extract_sub_from_bearer(auth) {
        if let Ok(v) = MetadataValue::try_from(user_id.as_str()) {
            grpc_req.metadata_mut().insert("x-user-id", v);
        }
    }

    Ok(grpc_req)
}

/// Extract `sub` claim from a Bearer JWT without signature verification.
fn extract_sub_from_bearer(bearer: &str) -> Option<String> {
    let token = bearer.strip_prefix("Bearer ")?;
    let payload_b64 = token.splitn(3, '.').nth(1)?;
    let decoded = base64url_decode_jwt(payload_b64)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn base64url_decode_jwt(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    match s.len() % 4 {
        2 => s.push_str("=="),
        3 => s.push('='),
        _ => {}
    }
    let bytes = s.as_bytes();
    const T: [u8; 128] = {
        let mut t = [0xffu8; 128];
        let mut i = 0u8;
        // A-Z = 0-25
        while i < 26 {
            t[(b'A' + i) as usize] = i;
            i += 1;
        }
        i = 0;
        // a-z = 26-51
        while i < 26 {
            t[(b'a' + i) as usize] = 26 + i;
            i += 1;
        }
        i = 0;
        // 0-9 = 52-61
        while i < 10 {
            t[(b'0' + i) as usize] = 52 + i;
            i += 1;
        }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut idx = 0;
    while idx + 3 < bytes.len() {
        let (b0, b1, b2, b3) = (
            bytes[idx] as usize,
            bytes[idx + 1] as usize,
            bytes[idx + 2] as usize,
            bytes[idx + 3] as usize,
        );
        if b0 >= 128 || b1 >= 128 {
            return None;
        }
        let (v0, v1) = (T[b0], T[b1]);
        if v0 == 0xff || v1 == 0xff {
            return None;
        }
        out.push((v0 << 2) | (v1 >> 4));
        if bytes[idx + 2] != b'=' {
            if b2 >= 128 {
                return None;
            }
            let v2 = T[b2];
            if v2 == 0xff {
                return None;
            }
            out.push((v1 << 4) | (v2 >> 2));
            if bytes[idx + 3] != b'=' {
                if b3 >= 128 {
                    return None;
                }
                let v3 = T[b3];
                if v3 == 0xff {
                    return None;
                }
                out.push((v2 << 6) | v3);
            }
        }
        idx += 4;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Request / Response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateBudgetRequest {
    org_id: String,
    name: String,
    budget_type: Option<serde_json::Value>, // accepts "standard" or 1
    currency: Option<String>,
    template_id: Option<String>,
}

#[derive(Deserialize)]
struct UpdateBudgetRequest {
    name: Option<String>,
    budget_type: Option<serde_json::Value>, // accepts "standard" or 1
}

#[derive(Deserialize)]
struct ListBudgetsQuery {
    org_id: String,
}

#[derive(Deserialize)]
struct AddMemberRequest {
    user_id: String,
    role: Option<i32>,
}

#[derive(Deserialize)]
struct UpdateMemberRoleRequest {
    role: i32,
}

#[derive(Deserialize)]
struct SetEnvelopeLimitRequest {
    monthly_limit: i64,
}

#[derive(Deserialize)]
struct SetRolloverPolicyRequest {
    policy: i32,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn create_budget(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateBudgetRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .create_budget(with_user(
            &headers,
            pb::CreateBudgetRequest {
                org_id: body.org_id,
                name: body.name,
                budget_type: parse_budget_type(body.budget_type.as_ref()),
                currency: body.currency.unwrap_or_else(|| "VND".to_string()),
                template_id: body.template_id.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_budget(resp.budget.as_ref()))))
}

async fn get_budget(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_budget(with_user(&headers, pb::GetBudgetRequest { budget_id })?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_budget(resp.budget.as_ref())))
}

async fn update_budget(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateBudgetRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    // Fetch current to fill defaults
    let current = c
        .get_budget(with_user(
            &headers,
            pb::GetBudgetRequest {
                budget_id: budget_id.clone(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let current_budget = current.budget.as_ref();
    let name = body
        .name
        .unwrap_or_else(|| current_budget.map(|b| b.name.clone()).unwrap_or_default());
    let budget_type = if body.budget_type.is_some() {
        parse_budget_type(body.budget_type.as_ref())
    } else {
        current_budget.map(|b| b.budget_type).unwrap_or(1)
    };
    let resp = c
        .update_budget(with_user(
            &headers,
            pb::UpdateBudgetRequest {
                budget_id,
                name,
                budget_type,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_budget(resp.budget.as_ref())))
}

async fn delete_budget(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_budget(with_user(&headers, pb::DeleteBudgetRequest { budget_id })?)
        .await
        .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message": "Budget deleted successfully"}),
    ))
}

async fn list_budgets(
    State(state): State<Arc<AppState>>,
    Query(params): Query<ListBudgetsQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_budgets(with_user(
            &headers,
            pb::ListBudgetsRequest {
                org_id: params.org_id,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let budgets: Vec<serde_json::Value> =
        resp.budgets.iter().map(|b| map_budget(Some(b))).collect();
    Ok(Json(serde_json::json!({"budgets": budgets})))
}

async fn list_members(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_budget_members(with_user(
            &headers,
            pb::ListBudgetMembersRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let members: Vec<serde_json::Value> = resp.members.iter().map(map_member).collect();
    Ok(Json(serde_json::json!({"members": members})))
}

async fn add_member(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AddMemberRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .add_budget_member(with_user(
            &headers,
            pb::AddBudgetMemberRequest {
                budget_id,
                user_id: body.user_id,
                role: body.role.unwrap_or(4), // Viewer default
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"member": map_member(resp.member.as_ref().unwrap())})),
    ))
}

async fn update_member_role(
    State(state): State<Arc<AppState>>,
    Path((budget_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<UpdateMemberRoleRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .update_budget_member_role(with_user(
            &headers,
            pb::UpdateBudgetMemberRoleRequest {
                budget_id,
                user_id,
                role: body.role,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(
        serde_json::json!({"member": map_member(resp.member.as_ref().unwrap())}),
    ))
}

async fn remove_member(
    State(state): State<Arc<AppState>>,
    Path((budget_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.remove_budget_member(with_user(
        &headers,
        pb::RemoveBudgetMemberRequest { budget_id, user_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message": "Member removed successfully"}),
    ))
}

async fn set_envelope_limit(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SetEnvelopeLimitRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .set_envelope_limit(with_user(
            &headers,
            pb::SetEnvelopeLimitRequest {
                budget_id,
                monthly_limit: body.monthly_limit,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_envelope(resp.envelope.as_ref())))
}

async fn get_burn_rate(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_burn_rate(with_user(&headers, pb::GetBurnRateRequest { budget_id })?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_envelope(resp.envelope.as_ref())))
}

async fn set_rollover_policy(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SetRolloverPolicyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .set_rollover_policy(with_user(
            &headers,
            pb::SetRolloverPolicyRequest {
                budget_id,
                policy: body.policy,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({"policy": resp.policy})))
}

async fn get_rollover_policy(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_rollover_policy(with_user(
            &headers,
            pb::GetRolloverPolicyRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({"policy": resp.policy})))
}

async fn list_templates(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_templates(with_user(&headers, pb::ListTemplatesRequest {})?)
        .await
        .map_err(map_status)?
        .into_inner();
    let templates: Vec<serde_json::Value> = resp
        .templates
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id,
                "name": t.name,
                "description": t.description,
                "budget_type": t.budget_type,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({"templates": templates})))
}

// ---------------------------------------------------------------------------
// Invest asset handlers
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateInvestAssetRequest {
    asset_type: Option<i32>,
    name: String,
    principal: Option<i64>,
    annual_rate: Option<f64>,
    interest_type: Option<String>,
    start_date: Option<String>,
    maturity_date: Option<String>,
    bank_name: Option<String>,
    quantity: Option<f64>,
    unit: Option<String>,
    cost_basis_per_unit: Option<i64>,
    ticker: Option<String>,
    exchange: Option<String>,
    avg_cost_per_share: Option<i64>,
    purchase_date: Option<String>,
    notes: Option<String>,
}

#[derive(Deserialize)]
struct UpdateInvestAssetRequest {
    name: Option<String>,
    annual_rate: Option<f64>,
    maturity_date: Option<String>,
    bank_name: Option<String>,
    quantity: Option<f64>,
    unit: Option<String>,
    cost_basis_per_unit: Option<i64>,
    avg_cost_per_share: Option<i64>,
    notes: Option<String>,
}

#[derive(Deserialize)]
struct AddPriceSnapshotRequest {
    price: i64,
    snapshot_date: Option<String>,
}

#[derive(Deserialize)]
struct ListSnapshotsQuery {
    limit: Option<i32>,
}

async fn create_invest_asset(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateInvestAssetRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .create_invest_asset(with_user(
            &headers,
            pb::CreateInvestAssetRequest {
                budget_id,
                asset_type: body.asset_type.unwrap_or(1),
                name: body.name,
                principal: body.principal,
                annual_rate: body.annual_rate,
                interest_type: body.interest_type,
                start_date: body.start_date,
                maturity_date: body.maturity_date,
                bank_name: body.bank_name,
                quantity: body.quantity,
                unit: body.unit,
                cost_basis_per_unit: body.cost_basis_per_unit,
                ticker: body.ticker,
                exchange: body.exchange,
                avg_cost_per_share: body.avg_cost_per_share,
                purchase_date: body.purchase_date,
                notes: body.notes,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_invest_asset(&resp))))
}

async fn update_invest_asset(
    State(state): State<Arc<AppState>>,
    Path((_budget_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<UpdateInvestAssetRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .update_invest_asset(with_user(
            &headers,
            pb::UpdateInvestAssetRequest {
                asset_id,
                name: body.name,
                annual_rate: body.annual_rate,
                maturity_date: body.maturity_date,
                bank_name: body.bank_name,
                quantity: body.quantity,
                unit: body.unit,
                cost_basis_per_unit: body.cost_basis_per_unit,
                avg_cost_per_share: body.avg_cost_per_share,
                notes: body.notes,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_invest_asset(&resp)))
}

async fn delete_invest_asset(
    State(state): State<Arc<AppState>>,
    Path((_budget_id, asset_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_invest_asset(with_user(
        &headers,
        pb::DeleteInvestAssetRequest { asset_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Asset deleted"})))
}

async fn list_invest_assets(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_invest_assets(with_user(
            &headers,
            pb::ListInvestAssetsRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let assets: Vec<serde_json::Value> = resp.assets.iter().map(map_invest_asset).collect();
    Ok(Json(serde_json::json!({"assets": assets})))
}

async fn get_invest_portfolio_summary(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_invest_portfolio_summary(with_user(
            &headers,
            pb::GetInvestPortfolioSummaryRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let assets: Vec<serde_json::Value> = resp.assets.iter().map(map_invest_asset).collect();
    Ok(Json(serde_json::json!({
        "budget_id": resp.budget_id,
        "total_current_value": resp.total_current_value,
        "total_cost_basis": resp.total_cost_basis,
        "total_unrealized_pnl": resp.total_unrealized_pnl,
        "total_pnl_pct": resp.total_pnl_pct,
        "assets": assets,
    })))
}

async fn add_price_snapshot(
    State(state): State<Arc<AppState>>,
    Path(asset_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AddPriceSnapshotRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .add_price_snapshot(with_user(
            &headers,
            pb::AddPriceSnapshotRequest {
                asset_id,
                price: body.price,
                snapshot_date: body.snapshot_date.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_snapshot(&resp))))
}

async fn get_latest_price_snapshot(
    State(state): State<Arc<AppState>>,
    Path(asset_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_latest_price_snapshot(with_user(
            &headers,
            pb::GetLatestPriceSnapshotRequest { asset_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_snapshot(&resp)))
}

async fn list_price_snapshots(
    State(state): State<Arc<AppState>>,
    Path(asset_id): Path<String>,
    Query(q): Query<ListSnapshotsQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_price_snapshots(with_user(
            &headers,
            pb::ListPriceSnapshotsRequest {
                asset_id,
                limit: q.limit.unwrap_or(90),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let snapshots: Vec<serde_json::Value> = resp.snapshots.iter().map(map_snapshot).collect();
    Ok(Json(serde_json::json!({"snapshots": snapshots})))
}

// ---------------------------------------------------------------------------
// Mappers
// ---------------------------------------------------------------------------

fn map_budget(budget: Option<&pb::Budget>) -> serde_json::Value {
    budget.map_or(serde_json::Value::Null, |b| {
        let base = b.base.as_ref();
        serde_json::json!({
            "id":          base.map(|x| &x.id).map_or("", |s| s),
            "org_id":      b.org_id,
            "name":        b.name,
            "budget_type": b.budget_type,
            "currency":    b.currency,
            "my_role":     b.my_role,
            "created_at":  base.map(|x| x.created_at).unwrap_or(0),
            "updated_at":  base.map(|x| x.updated_at).unwrap_or(0),
        })
    })
}

fn map_member(member: &pb::BudgetMember) -> serde_json::Value {
    serde_json::json!({
        "budget_id":    member.budget_id,
        "user_id":      member.user_id,
        "display_name": member.display_name,
        "email":        member.email,
        "role":         member.role,
    })
}

/// Convert a budget_type value that may be a string ("standard") or integer (1) to proto i32.
fn parse_budget_type(v: Option<&serde_json::Value>) -> i32 {
    match v {
        Some(serde_json::Value::Number(n)) => n.as_i64().unwrap_or(1) as i32,
        Some(serde_json::Value::String(s)) => match s.as_str() {
            "standard" => 1,
            "saving" => 2,
            "debt" => 3,
            "invest" => 4,
            "sharing" => 5,
            _ => 1,
        },
        _ => 1,
    }
}

fn map_envelope(envelope: Option<&pb::EnvelopeLimit>) -> serde_json::Value {
    envelope.map_or(serde_json::Value::Null, |e| {
        serde_json::json!({
            "budget_id":      e.budget_id,
            "monthly_limit":  e.monthly_limit,
            "current_spend":  e.current_spend,
            "burn_rate_pct":  e.burn_rate_pct,
            "limit_exceeded": e.limit_exceeded,
        })
    })
}

fn map_invest_asset(a: &pb::InvestAsset) -> serde_json::Value {
    serde_json::json!({
        "id":                  a.id,
        "budget_id":           a.budget_id,
        "asset_type":          a.asset_type,
        "name":                a.name,
        "status":              a.status,
        "principal":           a.principal,
        "annual_rate":         a.annual_rate,
        "interest_type":       a.interest_type,
        "start_date":          a.start_date,
        "maturity_date":       a.maturity_date,
        "bank_name":           a.bank_name,
        "quantity":            a.quantity,
        "unit":                a.unit,
        "cost_basis_per_unit": a.cost_basis_per_unit,
        "ticker":              a.ticker,
        "exchange":            a.exchange,
        "avg_cost_per_share":  a.avg_cost_per_share,
        "purchase_date":       a.purchase_date,
        "notes":               a.notes,
        "current_value":       a.current_value,
        "cost_basis":          a.cost_basis,
        "unrealized_pnl":      a.unrealized_pnl,
        "pnl_pct":             a.pnl_pct,
        "last_updated":        a.last_updated,
    })
}

fn map_snapshot(s: &pb::PriceSnapshot) -> serde_json::Value {
    serde_json::json!({
        "id":            s.id,
        "asset_id":      s.asset_id,
        "price":         s.price,
        "source":        s.source,
        "snapshot_date": s.snapshot_date,
    })
}
