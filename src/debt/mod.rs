//! Gateway REST routes for the Debt book (plan 2026-11-04), served at
//! `/api/budget/budgets/{budget_id}/debt/...`. Proto messages pass through as JSON; the path
//! carries `budget_id` / `account_id`, which win over anything in the body.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use tonic::transport::Channel;

use crate::pb::service::debt as pb;
use crate::pb::service::debt::debt_service_client::DebtServiceClient;
use crate::portfolio::{api_err, internal_error, make_req};
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/budgets/{budget_id}/debt/summary", get(summary))
        .route(
            "/budgets/{budget_id}/debt/accounts",
            get(list_accounts).post(create_account),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}",
            get(get_account).patch(update_account),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/close",
            post(close_account),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/reopen",
            post(reopen_account),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/settle",
            post(settle_account),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/rates",
            post(set_rate),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/rates/{rate_id}",
            delete(delete_rate),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/prepay-rules",
            post(set_rule),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/prepay-rules/{rule_id}",
            delete(delete_rule),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/schedule",
            get(schedule),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/quote",
            post(quote),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/payments",
            post(record_payment),
        )
        .route(
            "/budgets/{budget_id}/debt/accounts/{account_id}/adjust",
            post(adjust),
        )
        .route("/budgets/{budget_id}/debt/payments", get(list_payments))
        .route("/budgets/{budget_id}/debt/events", get(list_events))
}

async fn connect(state: &AppState) -> ApiResult<DebtServiceClient<Channel>> {
    DebtServiceClient::connect(state.budget_grpc_url.clone())
        .await
        .map_err(|e| internal_error(format!("connect: {e}")))
}

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    #[serde(default)]
    pub include_closed: Option<bool>,
    #[serde(default)]
    pub account_id: Option<String>,
    #[serde(default)]
    pub page: Option<i32>,
    #[serde(default)]
    pub page_size: Option<i32>,
    #[serde(default)]
    pub max_rows: Option<i32>,
}

async fn summary(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::DebtBookSummary>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::GetDebtBookSummaryRequest { budget_id },
    )?;
    Ok(Json(
        c.get_debt_book_summary(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

async fn list_accounts(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::ListDebtAccountsResponse>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::ListDebtAccountsRequest {
            budget_id,
            include_closed: q.include_closed.unwrap_or(false),
        },
    )?;
    Ok(Json(
        c.list_debt_accounts(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

async fn create_account(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(mut body): Json<pb::CreateDebtAccountRequest>,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    body.budget_id = budget_id;
    let req = make_req(&state, &headers, body)?;
    Ok(Json(
        c.create_debt_account(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

async fn get_account(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::DebtAccountRef {
            budget_id,
            account_id,
        },
    )?;
    Ok(Json(
        c.get_debt_account(req).await.map_err(api_err)?.into_inner(),
    ))
}

async fn update_account(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut body): Json<pb::UpdateDebtAccountRequest>,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    body.budget_id = budget_id;
    body.account_id = account_id;
    let req = make_req(&state, &headers, body)?;
    Ok(Json(
        c.update_debt_account(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

macro_rules! status_route {
    ($name:ident, $rpc:ident) => {
        async fn $name(
            State(state): State<Arc<AppState>>,
            Path((budget_id, account_id)): Path<(String, String)>,
            headers: HeaderMap,
        ) -> ApiResult<Json<pb::DebtAccount>> {
            let mut c = connect(&state).await?;
            let req = make_req(
                &state,
                &headers,
                pb::DebtAccountRef {
                    budget_id,
                    account_id,
                },
            )?;
            Ok(Json(c.$rpc(req).await.map_err(api_err)?.into_inner()))
        }
    };
}
status_route!(close_account, close_debt_account);
status_route!(reopen_account, reopen_debt_account);
status_route!(settle_account, settle_debt_account);

async fn set_rate(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(rate): Json<pb::RatePeriod>,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::SetRatePeriodRequest {
            budget_id,
            account_id,
            rate: Some(rate),
        },
    )?;
    Ok(Json(
        c.set_rate_period(req).await.map_err(api_err)?.into_inner(),
    ))
}

async fn delete_rate(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id, rate_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::DeleteRatePeriodRequest {
            budget_id,
            account_id,
            rate_id,
        },
    )?;
    Ok(Json(
        c.delete_rate_period(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

async fn set_rule(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(rule): Json<pb::PrepayRule>,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::SetPrepayRuleRequest {
            budget_id,
            account_id,
            rule: Some(rule),
        },
    )?;
    Ok(Json(
        c.set_prepay_rule(req).await.map_err(api_err)?.into_inner(),
    ))
}

async fn delete_rule(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id, rule_id)): Path<(String, String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::DeletePrepayRuleRequest {
            budget_id,
            account_id,
            rule_id,
        },
    )?;
    Ok(Json(
        c.delete_prepay_rule(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

async fn schedule(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::GetScheduleResponse>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::GetScheduleRequest {
            budget_id,
            account_id,
            max_rows: q.max_rows.unwrap_or(0),
        },
    )?;
    Ok(Json(
        c.get_schedule(req).await.map_err(api_err)?.into_inner(),
    ))
}

async fn quote(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut body): Json<pb::QuotePaymentRequest>,
) -> ApiResult<Json<pb::QuotePaymentResponse>> {
    let mut c = connect(&state).await?;
    body.budget_id = budget_id;
    body.account_id = account_id;
    let req = make_req(&state, &headers, body)?;
    Ok(Json(
        c.quote_payment(req).await.map_err(api_err)?.into_inner(),
    ))
}

async fn record_payment(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut body): Json<pb::RecordPaymentRequest>,
) -> ApiResult<Json<pb::RecordPaymentResponse>> {
    let mut c = connect(&state).await?;
    body.budget_id = budget_id;
    body.account_id = account_id;
    let req = make_req(&state, &headers, body)?;
    Ok(Json(
        c.record_payment(req).await.map_err(api_err)?.into_inner(),
    ))
}

async fn adjust(
    State(state): State<Arc<AppState>>,
    Path((budget_id, account_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(mut body): Json<pb::AdjustOutstandingRequest>,
) -> ApiResult<Json<pb::DebtAccount>> {
    let mut c = connect(&state).await?;
    body.budget_id = budget_id;
    body.account_id = account_id;
    let req = make_req(&state, &headers, body)?;
    Ok(Json(
        c.adjust_outstanding(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

async fn list_payments(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::ListDebtPaymentsResponse>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::ListDebtPaymentsRequest {
            budget_id,
            account_id: q.account_id.unwrap_or_default(),
            page: q.page.unwrap_or(0),
            page_size: q.page_size.unwrap_or(0),
        },
    )?;
    Ok(Json(
        c.list_debt_payments(req)
            .await
            .map_err(api_err)?
            .into_inner(),
    ))
}

async fn list_events(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(q): Query<ListQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<pb::ListDebtEventsResponse>> {
    let mut c = connect(&state).await?;
    let req = make_req(
        &state,
        &headers,
        pb::ListDebtEventsRequest {
            budget_id,
            account_id: q.account_id.unwrap_or_default(),
            page: q.page.unwrap_or(0),
            page_size: q.page_size.unwrap_or(0),
        },
    )?;
    Ok(Json(
        c.list_debt_events(req).await.map_err(api_err)?.into_inner(),
    ))
}
