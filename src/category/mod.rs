use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, patch},
    Json, Router,
};
use serde::Deserialize;
use tonic::{transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::category as pb;
use crate::pb::service::category::category_service_client::CategoryServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route(
            "/budgets/{budget_id}/categories",
            get(list_categories).post(create_category),
        )
        .route(
            "/categories/{category_id}",
            get(get_category)
                .patch(update_category)
                .delete(delete_category),
        )
        .route("/categories/{category_id}/archive", patch(archive_category))
        .route("/budgets/{budget_id}/plan", get(get_plan).put(set_plan))
}

async fn health(State(state): State<Arc<AppState>>) -> ApiResult<&'static str> {
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        CategoryServiceClient::connect(state.category_grpc_url.clone()),
    )
    .await;

    match res {
        Ok(Ok(_)) => Ok("OK"),
        Ok(Err(e)) => Err(map_status(Status::unavailable(e.to_string()))),
        Err(_) => Err(map_status(Status::deadline_exceeded(
            "Category gRPC connect timed out",
        ))),
    }
}

/// JSON label for a category type.
///
/// The REST DTO layer must NOT emit the raw protobuf enum — serde emits the
/// Rust variant name ("Expense"), while the clients compare against the
/// canonical lowercase labels ("expense").
fn category_type_to_string(v: i32) -> &'static str {
    match pb::CategoryType::try_from(v).unwrap_or(pb::CategoryType::Unspecified) {
        pb::CategoryType::Expense => "expense",
        pb::CategoryType::Income => "income",
        pb::CategoryType::Unspecified => "expense",
    }
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

fn with_user<T>(state: &AppState, headers: &HeaderMap, req: T) -> ApiResult<GrpcRequest<T>> {
    // SECURITY: verify the bearer token before forwarding the `sub` claim —
    // the category service trusts `x-user-id` at face value.
    crate::auth::request_with_user(&state.jwt_secret, headers, req)
        .map_err(|e| map_status(Status::unauthenticated(e.message())))
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateCategoryRequest {
    name: String,
    #[serde(alias = "cat_type")]
    kind: Option<i32>,
    icon: Option<String>,
    color: Option<String>,
    planned_amount: Option<i64>,
}

#[cfg(test)]
mod tests {
    use super::CreateCategoryRequest;

    #[test]
    fn create_category_accepts_canonical_kind() {
        let body: CreateCategoryRequest = serde_json::from_value(serde_json::json!({
            "name": "Salary",
            "kind": 2
        }))
        .unwrap();
        assert_eq!(body.kind, Some(2));
    }

    #[test]
    fn create_category_accepts_legacy_cat_type_alias() {
        let body: CreateCategoryRequest = serde_json::from_value(serde_json::json!({
            "name": "Salary",
            "cat_type": 2
        }))
        .unwrap();
        assert_eq!(body.kind, Some(2));
    }
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
    let resp = c
        .list_categories(with_user(
            &state,
            &headers,
            pb::ListCategoriesRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
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
    let resp = c
        .create_category(with_user(
            &state,
            &headers,
            pb::CreateCategoryRequest {
                budget_id,
                name: body.name,
                kind: body.kind.unwrap_or(1),
                icon: body.icon.unwrap_or_default(),
                color: body.color.unwrap_or_default(),
                planned_amount: body.planned_amount,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_cat(&resp))))
}

async fn get_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_category(with_user(
            &state,
            &headers,
            pb::GetCategoryRequest { category_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_cat(
        resp.category.as_ref().unwrap_or(&pb::Category::default()),
    )))
}

async fn update_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateCategoryRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .update_category(with_user(
            &state,
            &headers,
            pb::UpdateCategoryRequest {
                category_id,
                name: body.name,
                icon: body.icon,
                color: body.color,
                planned_amount: body.planned_amount,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_cat(&resp)))
}

async fn archive_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.archive_category(with_user(
        &state,
        &headers,
        pb::ArchiveCategoryRequest { category_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Category archived"})))
}

async fn delete_category(
    State(state): State<Arc<AppState>>,
    Path(category_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_category(with_user(
        &state,
        &headers,
        pb::DeleteCategoryRequest { category_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Category deleted"})))
}

fn map_cat(c: &pb::Category) -> serde_json::Value {
    let base = c.base.as_ref();
    serde_json::json!({
        "base": base.map(|b| serde_json::json!({
            "id": b.id.as_str(),
            "created_at": b.created_at,
            "updated_at": b.updated_at,
            "deleted_at": b.deleted_at,
            "created_by": b.created_by.as_str(),
            "updated_by": b.updated_by.as_str(),
            "owner_id": b.owner_id.as_str(),
            "status": b.status,
        })).unwrap_or(serde_json::Value::Null),
        "budget_id":      c.budget_id,
        "name":           c.name,
        "kind":           category_type_to_string(c.kind),
        "icon":           c.icon,
        "color":          c.color,
        "planned_amount": c.planned_amount,
        "actual_spend":   c.actual_spend,
        "usage_pct":      c.usage_pct,
        "tx_count":       c.tx_count,
        "archived":       c.archived,
    })
}

// ---------------------------------------------------------------------------
// Period plan
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct PlanQuery {
    #[serde(default)]
    period_offset: Option<i32>,
}

#[derive(Deserialize)]
struct PlanItemBody {
    category_id: String,
    planned_amount: i64,
    /// "this_period" | "default"
    scope: String,
}

#[derive(Deserialize)]
struct SetPlanBody {
    #[serde(default)]
    period_offset: Option<i32>,
    #[serde(default)]
    items: Vec<PlanItemBody>,
    #[serde(default)]
    unallocated_planned: Option<i64>,
}

fn parse_scope(s: &str) -> Result<pb::PlanScope, (StatusCode, Json<ErrorResponse>)> {
    match s.to_ascii_lowercase().as_str() {
        "this_period" => Ok(pb::PlanScope::ThisPeriod),
        "default" => Ok(pb::PlanScope::Default),
        _ => Err(map_status(Status::invalid_argument(
            "scope must be this_period or default",
        ))),
    }
}

fn map_plan(p: &pb::BudgetPlan) -> serde_json::Value {
    let cats: Vec<serde_json::Value> = p
        .categories
        .iter()
        .map(|c| {
            serde_json::json!({
                "category_id":     c.category_id,
                "name":            c.name,
                "kind":            category_type_to_string(c.kind),
                "icon":            c.icon,
                "color":           c.color,
                "default_planned": c.default_planned,
                "period_override": c.period_override,
                "planned":         c.planned,
                "archived":        c.archived,
            })
        })
        .collect();
    serde_json::json!({
        "budget_id":           p.budget_id,
        "period_from":         p.period_from,
        "period_to":           p.period_to,
        "categories":          cats,
        "unallocated_planned": p.unallocated_planned,
        "total_planned":       p.total_planned,
    })
}

async fn get_plan(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(q): Query<PlanQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_plan(with_user(
            &state,
            &headers,
            pb::GetPlanRequest {
                budget_id,
                period_offset: q.period_offset.unwrap_or(0),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_plan(&resp)))
}

async fn set_plan(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<SetPlanBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = body
        .items
        .iter()
        .map(|i| {
            Ok(pb::PlanItem {
                category_id: i.category_id.clone(),
                planned_amount: i.planned_amount,
                scope: parse_scope(&i.scope)? as i32,
            })
        })
        .collect::<Result<Vec<_>, (StatusCode, Json<ErrorResponse>)>>()?;
    let mut c = client(&state).await?;
    let resp = c
        .set_period_plan(with_user(
            &state,
            &headers,
            pb::SetPeriodPlanRequest {
                budget_id,
                period_offset: body.period_offset.unwrap_or(0),
                items,
                unallocated_planned: body.unallocated_planned,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_plan(&resp)))
}

#[cfg(test)]
mod plan_tests {
    use super::*;

    #[test]
    fn scope_names_map_to_enum() {
        assert_eq!(
            parse_scope("this_period").ok(),
            Some(pb::PlanScope::ThisPeriod)
        );
        assert_eq!(parse_scope("DEFAULT").ok(), Some(pb::PlanScope::Default));
        assert!(parse_scope("forever").is_err());
    }

    #[test]
    fn plan_json_exposes_override_as_null_when_absent() {
        let plan = pb::BudgetPlan {
            budget_id: "b".into(),
            categories: vec![pb::CategoryPlan {
                category_id: "c".into(),
                planned: 5,
                ..Default::default()
            }],
            ..Default::default()
        };
        let v = map_plan(&plan);
        assert!(v["categories"][0]["period_override"].is_null());
        assert_eq!(v["categories"][0]["planned"], 5);
    }
}
