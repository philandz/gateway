use std::sync::Arc;

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::category as pb;
use crate::pb::service::category::category_service_client::CategoryServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/budgets/{budget_id}/categories", get(list_categories).post(create_category))
        .route("/categories/{category_id}", get(get_category).patch(update_category).delete(delete_category))
        .route("/categories/{category_id}/archive", patch(archive_category))
}

fn map_status(status: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, envelope) = philand_error::http_error_from_tonic_status(&status);
    (http, Json(envelope))
}

async fn client(state: &AppState) -> ApiResult<CategoryServiceClient<Channel>> {
    CategoryServiceClient::connect(state.category_grpc_url.clone())
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

    if let Some(user_id) = extract_sub(auth) {
        if let Ok(v) = MetadataValue::try_from(user_id.as_str()) {
            grpc_req.metadata_mut().insert("x-user-id", v);
        }
    }
    Ok(grpc_req)
}

fn extract_sub(bearer: &str) -> Option<String> {
    let token = bearer.strip_prefix("Bearer ")?;
    let payload = token.splitn(3, '.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims.get("sub").and_then(|v| v.as_str()).map(|s| s.to_string())
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    match s.len() % 4 { 2 => s.push_str("=="), 3 => s.push('='), _ => {} }
    let bytes = s.as_bytes();
    const T: [u8; 128] = { let mut t = [0xffu8; 128]; let mut i = 0u8;
        while i < 26 { t[(b'A'+i) as usize] = i; i += 1; } i = 0;
        while i < 26 { t[(b'a'+i) as usize] = 26+i; i += 1; } i = 0;
        while i < 10 { t[(b'0'+i) as usize] = 52+i; i += 1; }
        t[b'+' as usize] = 62; t[b'/' as usize] = 63; t };
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut idx = 0;
    while idx + 3 < bytes.len() {
        let (b0,b1,b2,b3) = (bytes[idx] as usize,bytes[idx+1] as usize,bytes[idx+2] as usize,bytes[idx+3] as usize);
        if b0>=128||b1>=128 { return None; }
        let (v0,v1) = (T[b0],T[b1]); if v0==0xff||v1==0xff { return None; }
        out.push((v0<<2)|(v1>>4));
        if bytes[idx+2]!=b'=' { if b2>=128 { return None; } let v2=T[b2]; if v2==0xff { return None; } out.push((v1<<4)|(v2>>2));
            if bytes[idx+3]!=b'=' { if b3>=128 { return None; } let v3=T[b3]; if v3==0xff { return None; } out.push((v2<<6)|v3); } }
        idx += 4;
    }
    Some(out)
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateCategoryRequest {
    name: String,
    cat_type: Option<i32>,
    icon: Option<String>,
    color: Option<String>,
    planned_amount: Option<i64>,
}

#[derive(Deserialize)]
struct UpdateCategoryRequest {
    name: Option<String>,
    icon: Option<String>,
    color: Option<String>,
    planned_amount: Option<i64>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn list_categories(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c.list_categories(with_user(&headers, pb::ListCategoriesRequest { budget_id })?)
        .await.map_err(map_status)?.into_inner();
    let cats: Vec<serde_json::Value> = resp.categories.iter().map(map_cat).collect();
    Ok(Json(serde_json::json!({"categories": cats})))
}

async fn create_category(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateCategoryRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c.create_category(with_user(&headers, pb::CreateCategoryRequest {
        budget_id,
        name: body.name,
        cat_type: body.cat_type.unwrap_or(1),
        icon: body.icon.unwrap_or_default(),
        color: body.color.unwrap_or_default(),
        planned_amount: body.planned_amount,
    })?).await.map_err(map_status)?.into_inner();
    Ok((StatusCode::CREATED, Json(map_cat(&resp))))
}

async fn get_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c.get_category(with_user(&headers, pb::GetCategoryRequest { category_id })?)
        .await.map_err(map_status)?.into_inner();
    Ok(Json(map_cat(resp.category.as_ref().unwrap_or(&pb::Category::default()))))
}

async fn update_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateCategoryRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c.update_category(with_user(&headers, pb::UpdateCategoryRequest {
        category_id,
        name: body.name,
        icon: body.icon,
        color: body.color,
        planned_amount: body.planned_amount,
    })?).await.map_err(map_status)?.into_inner();
    Ok(Json(map_cat(&resp)))
}

async fn archive_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.archive_category(with_user(&headers, pb::ArchiveCategoryRequest { category_id })?)
        .await.map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Category archived"})))
}

async fn delete_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_category(with_user(&headers, pb::DeleteCategoryRequest { category_id })?)
        .await.map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Category deleted"})))
}

fn map_cat(c: &pb::Category) -> serde_json::Value {
    serde_json::json!({
        "id":             c.id,
        "budget_id":      c.budget_id,
        "name":           c.name,
        "cat_type":       c.cat_type,
        "icon":           c.icon,
        "color":          c.color,
        "planned_amount": c.planned_amount,
        "actual_spend":   c.actual_spend,
        "usage_pct":      c.usage_pct,
        "tx_count":       c.tx_count,
        "archived":       c.archived,
        "created_at":     c.created_at,
        "updated_at":     c.updated_at,
    })
}
