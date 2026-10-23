//! Gateway REST routes for the Insight service (read-only reports).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde::Deserialize;
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::insight as pb;
use crate::pb::service::insight::insight_service_client::InsightServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new().route(
        "/budgets/{budget_id}/period-summary",
        get(get_period_summary),
    )
}

#[derive(Deserialize, Default)]
struct PeriodQuery {
    #[serde(default)]
    offset: Option<i32>,
}

fn map_status(status: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, envelope) = philand_error::http_error_from_tonic_status(&status);
    (http, Json(envelope))
}

async fn client(state: &AppState) -> ApiResult<InsightServiceClient<Channel>> {
    InsightServiceClient::connect(state.insight_grpc_url.clone())
        .await
        .map_err(|e| map_status(Status::unavailable(e.to_string())))
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
    let sub = auth
        .strip_prefix("Bearer ")
        .and_then(sub_from_jwt)
        .ok_or_else(|| map_status(Status::unauthenticated("Invalid token")))?;
    let v = MetadataValue::try_from(sub.as_str())
        .map_err(|_| map_status(Status::unauthenticated("Invalid token subject")))?;
    grpc_req.metadata_mut().insert("x-user-id", v);
    Ok(grpc_req)
}

/// The gateway has already authenticated the bearer; this only reads the `sub` claim.
fn sub_from_jwt(token: &str) -> Option<String> {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine as _;
    let payload = token.split('.').nth(1)?;
    let bytes = URL_SAFE_NO_PAD.decode(payload).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    v.get("sub")?.as_str().map(str::to_string)
}

fn status_name(s: i32) -> &'static str {
    match pb::ProgressStatus::try_from(s) {
        Ok(pb::ProgressStatus::Warn) => "warn",
        Ok(pb::ProgressStatus::Over) => "over",
        _ => "ok",
    }
}

fn map_summary(s: &pb::PeriodSummary) -> serde_json::Value {
    let cats: Vec<_> = s
        .categories
        .iter()
        .map(|c| {
            serde_json::json!({
                "category_id": c.category_id,
                "name":        c.name,
                "icon":        c.icon,
                "color":       c.color,
                "planned":     c.planned,
                "carry_in":    c.carry_in,
                "available":   c.available,
                "spent":       c.spent,
                "pct":         c.pct,
                "status":      status_name(c.status),
            })
        })
        .collect();
    let daily: Vec<_> = s
        .daily
        .iter()
        .map(|d| serde_json::json!({"date": d.date, "income": d.income, "expense": d.expense}))
        .collect();
    serde_json::json!({
        "budget_id":         s.budget_id,
        "period_from":       s.period_from,
        "period_to":         s.period_to,
        "days_left":         s.days_left,
        "income":            s.income,
        "expense":           s.expense,
        "transfer_in":       s.transfer_in,
        "transfer_out":      s.transfer_out,
        "total_available":   s.total_available,
        "total_spent":       s.total_spent,
        "remaining":         s.remaining,
        "safe_daily_spend":  s.safe_daily_spend,
        "projected_pct":     s.projected_pct,
        "unallocated_spent": s.unallocated_spent,
        "rollover_policy":   s.rollover_policy,
        "categories":        cats,
        "daily":             daily,
    })
}

async fn get_period_summary(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(q): Query<PeriodQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_period_summary(with_user(
            &headers,
            pb::PeriodSummaryRequest {
                budget_id,
                offset: q.offset.unwrap_or(0),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_summary(&resp)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_names() {
        assert_eq!(status_name(pb::ProgressStatus::Over as i32), "over");
        assert_eq!(status_name(pb::ProgressStatus::Warn as i32), "warn");
        assert_eq!(status_name(0), "ok");
    }

    #[test]
    fn sub_is_read_from_jwt_payload() {
        // header.payload.sig with payload {"sub":"u1"}
        assert_eq!(
            sub_from_jwt("e30.eyJzdWIiOiJ1MSJ9.x").as_deref(),
            Some("u1")
        );
        assert_eq!(sub_from_jwt("garbage"), None);
    }
}
