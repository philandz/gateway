use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use tonic::{transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::entry as pb;
use crate::pb::service::entry::entry_service_client::EntryServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/budgets/{budget_id}/entries", get(list_entries))
        .route(
            "/budgets/{budget_id}/entries/bulk-import",
            post(bulk_import),
        )
        .route(
            "/budgets/{budget_id}/entries/recurring",
            post(create_recurring),
        )
        .route("/budgets/{budget_id}/entries/split", post(create_split))
        .route("/budgets/{budget_id}/summary", get(get_entry_summary))
        .route("/transfers", post(create_transfer))
        .route(
            "/transfers/{group_id}",
            patch(update_transfer).delete(delete_transfer),
        )
        .route("/entries", get(list_all_entries).post(create_entry))
        .route(
            "/entries/{entry_id}",
            get(get_entry).patch(update_entry).delete(delete_entry),
        )
        .route(
            "/entries/{entry_id}/recurrence",
            patch(update_recurrence).delete(cancel_recurrence),
        )
        .route("/entries/{entry_id}/split-legs", get(list_split_legs))
        .route(
            "/entries/{entry_id}/comments",
            get(list_comments).post(add_comment),
        )
        .route(
            "/comments/{comment_id}",
            patch(edit_comment).delete(delete_comment),
        )
        .route(
            "/entries/{entry_id}/attachments",
            get(list_attachments).post(attach_file),
        )
        .route("/attachments/{attachment_id}", delete(remove_attachment))
}

async fn health(State(state): State<Arc<AppState>>) -> ApiResult<&'static str> {
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        EntryServiceClient::connect(state.entry_grpc_url.clone()),
    )
    .await;

    match res {
        Ok(Ok(_)) => Ok("OK"),
        Ok(Err(e)) => Err(map_status(Status::unavailable(e.to_string()))),
        Err(_) => Err(map_status(Status::deadline_exceeded(
            "Entry gRPC connect timed out",
        ))),
    }
}

/// JSON label for an entry kind.
///
/// The REST DTO layer must NOT emit the raw protobuf enum — serde emits the
/// Rust variant name ("Expense"), while the clients compare against the
/// canonical lowercase labels ("expense").
fn entry_kind_to_string(v: i32) -> &'static str {
    match pb::EntryKind::try_from(v).unwrap_or(pb::EntryKind::Unspecified) {
        pb::EntryKind::Expense => "expense",
        pb::EntryKind::Income => "income",
        pb::EntryKind::TransferOut => "transfer_out",
        pb::EntryKind::TransferIn => "transfer_in",
        pb::EntryKind::Unspecified => "expense",
    }
}

fn map_status(s: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, env) = philand_error::http_error_from_tonic_status(&s);
    (http, Json(env))
}

async fn client(state: &AppState) -> ApiResult<EntryServiceClient<Channel>> {
    EntryServiceClient::connect(state.entry_grpc_url.clone())
        .await
        .map_err(|e| map_status(Status::internal(e.to_string())))
}

fn with_user<T>(state: &AppState, headers: &HeaderMap, req: T) -> ApiResult<GrpcRequest<T>> {
    // SECURITY: verify the bearer token before forwarding the `sub` claim —
    // the entry service trusts `x-user-id` at face value. The `user_type`
    // claim is forwarded too, so downstream services can grant super_admin
    // the Owner role without an explicit budget_members row (see
    // budget/src/manager/biz/mod.rs `resolve_role`).
    crate::auth::request_with_user(&state.jwt_secret, headers, req)
        .map_err(|e| map_status(Status::unauthenticated(e.message())))
}

// ---------------------------------------------------------------------------
// Request types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateEntryRequest {
    budget_id: String,
    category_id: Option<String>,
    kind: Option<i32>,
    amount: i64,
    description: Option<String>,
    entry_date: String,
    tags: Option<Vec<String>>,
    notes: Option<String>,
}

#[derive(Deserialize)]
struct UpdateEntryRequest {
    category_id: Option<String>,
    kind: Option<i32>,
    amount: Option<i64>,
    description: Option<String>,
    entry_date: Option<String>,
    tags: Option<Vec<String>>,
    notes: Option<String>,
}

#[derive(Deserialize, Default)]
struct ListEntriesQuery {
    q: Option<String>,
    kind: Option<String>,
    category_id: Option<String>,
    category_ids: Option<String>,
    member_ids: Option<String>,
    date_from: Option<String>,
    date_to: Option<String>,
    amount_min: Option<i64>,
    amount_max: Option<i64>,
    tags: Option<String>,
    sort_by: Option<String>,
    sort_dir: Option<String>,
    page: Option<i32>,
    page_size: Option<i32>,
    budget_ids: Option<String>,
    include_summary: Option<bool>,
}

#[derive(Deserialize)]
struct BulkImportRow {
    entry_date: String,
    amount: i64,
    kind: Option<i32>,
    description: Option<String>,
    category_id: Option<String>,
    tags: Option<Vec<String>>,
    notes: Option<String>,
}

#[derive(Deserialize)]
struct BulkImportRequest {
    rows: Vec<BulkImportRow>,
}

#[derive(Deserialize)]
struct CommentBody {
    body: String,
}
#[derive(Deserialize)]
struct AttachBody {
    file_id: String,
    file_name: Option<String>,
}

#[derive(Deserialize)]
struct CreateRecurringRequest {
    category_id: Option<String>,
    kind: Option<i32>,
    amount: i64,
    description: Option<String>,
    entry_date: String,
    tags: Option<Vec<String>>,
    notes: Option<String>,
    recurrence_rule: String,
}

#[derive(Deserialize)]
struct UpdateRecurrenceRequest {
    recurrence_rule: Option<String>,
}

#[derive(Deserialize)]
struct SplitLegBody {
    budget_id: Option<String>,
    category_id: Option<String>,
    amount: i64,
    description: Option<String>,
}

#[derive(Deserialize)]
struct CreateSplitRequest {
    kind: Option<i32>,
    total_amount: i64,
    description: Option<String>,
    entry_date: String,
    tags: Option<Vec<String>>,
    notes: Option<String>,
    legs: Vec<SplitLegBody>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn create_entry(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateEntryRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .create_entry(with_user(
            &state,
            &headers,
            pb::CreateEntryRequest {
                budget_id: body.budget_id,
                category_id: body.category_id.unwrap_or_default(),
                kind: body.kind.unwrap_or(1),
                amount: body.amount,
                description: body.description.unwrap_or_default(),
                entry_date: body.entry_date,
                tags: body.tags.unwrap_or_default(),
                notes: body.notes.unwrap_or_default(),
                recurrence_rule: None,
                split_group_id: None,
                split_total: None,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_entry(&resp))))
}

async fn get_entry(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_entry(with_user(
            &state,
            &headers,
            pb::GetEntryRequest { entry_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_entry(
        resp.entry.as_ref().unwrap_or(&pb::Entry::default()),
    )))
}

/// List body; `summary` is present only when the caller asked for it.
fn list_body(
    entries: Vec<serde_json::Value>,
    meta: serde_json::Value,
    summary: Option<&pb::ListSummary>,
) -> serde_json::Value {
    let mut body = serde_json::json!({"entries": entries, "meta": meta});
    if let Some(s) = summary {
        body["summary"] = serde_json::json!({
            "count": s.count, "income": s.income, "expense": s.expense,
            "transfer_in": s.transfer_in, "transfer_out": s.transfer_out,
        });
    }
    body
}

async fn list_entries(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(q): Query<ListEntriesQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;

    // Parse category_ids: prefer plural form, fall back to singular if plural is absent
    let category_ids: Vec<String> = if let Some(ids) = &q.category_ids {
        if !ids.is_empty() {
            ids.split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        } else {
            vec![]
        }
    } else if let Some(id) = &q.category_id {
        if id.is_empty() {
            vec![]
        } else {
            vec![id.clone()]
        }
    } else {
        vec![]
    };

    // Parse member_ids: split on comma
    let member_ids: Vec<String> = q
        .member_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    let params = pb::ListParams {
        q: q.q,
        kind: q.kind,
        category_id: q.category_id,
        category_ids,
        member_ids,
        date_from: q.date_from,
        date_to: q.date_to,
        amount_min: q.amount_min,
        amount_max: q.amount_max,
        tags: q.tags,
        sort_by: q.sort_by,
        sort_dir: q.sort_dir,
        page: q.page,
        page_size: q.page_size,
        include_summary: q.include_summary,
    };
    let resp = c
        .list_entries(with_user(
            &state,
            &headers,
            pb::ListEntriesRequest {
                budget_id: Some(budget_id),
                params: Some(params),
                budget_ids: vec![],
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let entries: Vec<serde_json::Value> = resp.entries.iter().map(map_entry).collect();
    let meta = resp.meta.as_ref().map_or(
        serde_json::json!({"page":1,"page_size":20,"total_pages":1,"total_rows":0}),
        |m| serde_json::json!({"page":m.page,"page_size":m.page_size,"total_pages":m.total_pages,"total_rows":m.total_rows}),
    );
    Ok(Json(list_body(entries, meta, resp.summary.as_ref())))
}

async fn list_all_entries(
    State(state): State<Arc<AppState>>,
    Query(q): Query<ListEntriesQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let budget_ids: Vec<String> = q
        .budget_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    // Parse category_ids: prefer plural form, fall back to singular if plural is absent
    let category_ids: Vec<String> = if let Some(ids) = &q.category_ids {
        if !ids.is_empty() {
            ids.split(',')
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
                .collect()
        } else {
            vec![]
        }
    } else if let Some(id) = &q.category_id {
        if id.is_empty() {
            vec![]
        } else {
            vec![id.clone()]
        }
    } else {
        vec![]
    };

    // Parse member_ids: split on comma
    let member_ids: Vec<String> = q
        .member_ids
        .as_deref()
        .unwrap_or("")
        .split(',')
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    let params = pb::ListParams {
        q: q.q,
        kind: q.kind,
        category_id: q.category_id,
        category_ids,
        member_ids,
        date_from: q.date_from,
        date_to: q.date_to,
        amount_min: q.amount_min,
        amount_max: q.amount_max,
        tags: q.tags,
        sort_by: q.sort_by,
        sort_dir: q.sort_dir,
        page: q.page,
        page_size: q.page_size,
        include_summary: q.include_summary,
    };
    let resp = c
        .list_entries(with_user(
            &state,
            &headers,
            pb::ListEntriesRequest {
                budget_id: None,
                params: Some(params),
                budget_ids,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let entries: Vec<serde_json::Value> = resp.entries.iter().map(map_entry).collect();
    let meta = resp.meta.as_ref().map_or(
        serde_json::json!({"page":1,"page_size":20,"total_pages":1,"total_rows":0}),
        |m| serde_json::json!({"page":m.page,"page_size":m.page_size,"total_pages":m.total_pages,"total_rows":m.total_rows}),
    );
    Ok(Json(list_body(entries, meta, resp.summary.as_ref())))
}

async fn update_entry(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateEntryRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .update_entry(with_user(
            &state,
            &headers,
            pb::UpdateEntryRequest {
                entry_id,
                category_id: body.category_id,
                kind: body.kind,
                amount: body.amount,
                description: body.description,
                entry_date: body.entry_date,
                tags: body.tags.unwrap_or_default(),
                notes: body.notes,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_entry(&resp)))
}

async fn delete_entry(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_entry(with_user(
        &state,
        &headers,
        pb::DeleteEntryRequest { entry_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Entry deleted"})))
}

async fn bulk_import(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<BulkImportRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let rows = body
        .rows
        .into_iter()
        .map(|r| pb::BulkImportRow {
            entry_date: r.entry_date,
            amount: r.amount,
            kind: r.kind.unwrap_or(1),
            description: r.description.unwrap_or_default(),
            category_id: r.category_id.unwrap_or_default(),
            tags: r.tags.unwrap_or_default(),
            notes: r.notes.unwrap_or_default(),
        })
        .collect();
    let resp = c
        .bulk_import_entries(with_user(
            &state,
            &headers,
            pb::BulkImportEntriesRequest { budget_id, rows },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let results: Vec<serde_json::Value> = resp.results.iter().map(|r| serde_json::json!({
        "row_index": r.row_index, "success": r.success, "error": r.error, "entry_id": r.entry_id,
    })).collect();
    Ok(Json(
        serde_json::json!({"imported_count": resp.imported_count, "error_count": resp.error_count, "results": results}),
    ))
}

async fn add_comment(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommentBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .add_comment(with_user(
            &state,
            &headers,
            pb::AddCommentRequest {
                entry_id,
                body: body.body,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_comment(&resp))))
}

async fn edit_comment(
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CommentBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .edit_comment(with_user(
            &state,
            &headers,
            pb::EditCommentRequest {
                comment_id,
                body: body.body,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_comment(&resp)))
}

async fn delete_comment(
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_comment(with_user(
        &state,
        &headers,
        pb::DeleteCommentRequest { comment_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Comment deleted"})))
}

async fn list_comments(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_comments(with_user(
            &state,
            &headers,
            pb::ListCommentsRequest { entry_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let comments: Vec<serde_json::Value> = resp.comments.iter().map(map_comment).collect();
    Ok(Json(serde_json::json!({"comments": comments})))
}

async fn attach_file(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AttachBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .attach_file(with_user(
            &state,
            &headers,
            pb::AttachFileRequest {
                entry_id,
                file_id: body.file_id,
                file_name: body.file_name.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_attachment(&resp))))
}

async fn remove_attachment(
    State(state): State<Arc<AppState>>,
    Path(attachment_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.remove_attachment(with_user(
        &state,
        &headers,
        pb::RemoveAttachmentRequest { attachment_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Attachment removed"})))
}

async fn list_attachments(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_attachments(with_user(
            &state,
            &headers,
            pb::ListAttachmentsRequest { entry_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let attachments: Vec<serde_json::Value> = resp.attachments.iter().map(map_attachment).collect();
    Ok(Json(serde_json::json!({"attachments": attachments})))
}

// ---------------------------------------------------------------------------
// Recurring handlers
// ---------------------------------------------------------------------------

async fn create_recurring(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateRecurringRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .create_recurring_entry(with_user(
            &state,
            &headers,
            pb::CreateRecurringEntryRequest {
                budget_id,
                category_id: body.category_id.unwrap_or_default(),
                kind: body.kind.unwrap_or(1),
                amount: body.amount,
                description: body.description.unwrap_or_default(),
                entry_date: body.entry_date,
                tags: body.tags.unwrap_or_default(),
                notes: body.notes.unwrap_or_default(),
                recurrence_rule: body.recurrence_rule,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_entry(&resp))))
}

async fn update_recurrence(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateRecurrenceRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .update_recurrence_rule(with_user(
            &state,
            &headers,
            pb::UpdateRecurrenceRuleRequest {
                entry_id,
                recurrence_rule: body.recurrence_rule.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_entry(&resp)))
}

async fn cancel_recurrence(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .cancel_recurrence(with_user(
            &state,
            &headers,
            pb::CancelRecurrenceRequest { entry_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_entry(&resp)))
}

// ---------------------------------------------------------------------------
// Split handlers
// ---------------------------------------------------------------------------

async fn create_split(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<CreateSplitRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let legs = body
        .legs
        .into_iter()
        .map(|l| pb::SplitLeg {
            entry_id: String::new(),
            budget_id: l.budget_id.unwrap_or_default(),
            category_id: l.category_id.unwrap_or_default(),
            amount: l.amount,
            description: l.description.unwrap_or_default(),
        })
        .collect();
    let resp = c
        .create_split_entry(with_user(
            &state,
            &headers,
            pb::CreateSplitEntryRequest {
                budget_id,
                kind: body.kind.unwrap_or(1),
                total_amount: body.total_amount,
                description: body.description.unwrap_or_default(),
                entry_date: body.entry_date,
                tags: body.tags.unwrap_or_default(),
                notes: body.notes.unwrap_or_default(),
                legs,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let legs_json: Vec<serde_json::Value> = resp.legs.iter().map(map_entry).collect();
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "split_group_id": resp.split_group_id,
            "legs": legs_json,
        })),
    ))
}

async fn list_split_legs(
    State(state): State<Arc<AppState>>,
    Path(entry_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_split_legs(with_user(
            &state,
            &headers,
            pb::ListSplitLegsRequest { entry_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let legs: Vec<serde_json::Value> = resp.legs.iter().map(map_entry).collect();
    Ok(Json(
        serde_json::json!({"split_group_id": resp.split_group_id, "legs": legs}),
    ))
}

// ---------------------------------------------------------------------------
// Summary
// ---------------------------------------------------------------------------

async fn get_entry_summary(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_entry_summary(with_user(
            &state,
            &headers,
            pb::GetEntrySummaryRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    // Map all four snake_case fields unchanged
    Ok(Json(serde_json::json!({
        "budget_id": resp.budget_id,
        "total_income": resp.total_income,
        "total_expense": resp.total_expense,
        "current_balance": resp.current_balance,
    })))
}

// ---------------------------------------------------------------------------
// Mappers
// ---------------------------------------------------------------------------

fn map_entry(e: &pb::Entry) -> serde_json::Value {
    let base = e.base.as_ref();
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
        "budget_id": e.budget_id,
        "category_id": e.category_id,
        "kind": entry_kind_to_string(e.kind),
        "amount": e.amount,
        "description": e.description,
        "entry_date": e.entry_date,
        "tags": e.tags,
        "notes": e.notes,
        "is_recurring": e.is_recurring,
        "has_attachment": e.has_attachment,
        "recurrence_rule": e.recurrence_rule,
        "next_occurrence": e.next_occurrence,
        "split_group_id": e.split_group_id,
        "split_total": e.split_total,
        "transfer_group_id": e.transfer_group_id,
        "counterparty_budget_id": e.counterparty_budget_id,
        "counterparty_kind": e.counterparty_kind,
    })
}

fn map_comment(c: &pb::Comment) -> serde_json::Value {
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
        "entry_id": c.entry_id,
        "body": c.body,
    })
}

fn map_attachment(a: &pb::Attachment) -> serde_json::Value {
    let base = a.base.as_ref();
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
        "entry_id": a.entry_id,
        "file_id": a.file_id,
        "file_name": a.file_name,
    })
}

#[cfg(test)]
mod tests {
    mod list_entries_query;
}

// ---------------------------------------------------------------------------
// Transfers (plan R1)
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct CreateTransferBody {
    source_budget_id: String,
    destination_budget_id: String,
    amount: i64,
    entry_date: String,
    description: Option<String>,
    notes: Option<String>,
    idempotency_key: Option<String>,
}

#[derive(Deserialize)]
struct UpdateTransferBody {
    amount: Option<i64>,
    entry_date: Option<String>,
    description: Option<String>,
    notes: Option<String>,
}

fn map_transfer(t: &pb::Transfer) -> serde_json::Value {
    serde_json::json!({
        "group_id": t.group_id,
        "out_entry": t.out_entry.as_ref().map(map_entry),
        "in_entry": t.in_entry.as_ref().map(map_entry),
    })
}

async fn create_transfer(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<CreateTransferBody>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .create_transfer(with_user(
            &state,
            &headers,
            pb::CreateTransferRequest {
                source_budget_id: body.source_budget_id,
                destination_budget_id: body.destination_budget_id,
                amount: body.amount,
                entry_date: body.entry_date,
                description: body.description.unwrap_or_default(),
                notes: body.notes,
                idempotency_key: body.idempotency_key.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_transfer(&resp))))
}

async fn update_transfer(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<UpdateTransferBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .update_transfer(with_user(
            &state,
            &headers,
            pb::UpdateTransferRequest {
                group_id,
                amount: body.amount,
                entry_date: body.entry_date,
                description: body.description,
                notes: body.notes,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_transfer(&resp)))
}

async fn delete_transfer(
    State(state): State<Arc<AppState>>,
    Path(group_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_transfer(with_user(
        &state,
        &headers,
        pb::DeleteTransferRequest { group_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({"message": "Transfer deleted"})))
}

#[cfg(test)]
mod transfer_tests {
    use super::*;

    #[test]
    fn map_transfer_exposes_both_legs_and_link_fields() {
        let leg = |kind: i32| pb::Entry {
            kind,
            amount: 500,
            transfer_group_id: Some("g".into()),
            counterparty_kind: Some("entry".into()),
            ..Default::default()
        };
        let t = pb::Transfer {
            group_id: "g".into(),
            out_entry: Some(leg(pb::EntryKind::TransferOut as i32)),
            in_entry: Some(leg(pb::EntryKind::TransferIn as i32)),
        };
        let v = map_transfer(&t);
        assert_eq!(v["group_id"], "g");
        assert_eq!(v["out_entry"]["transfer_group_id"], "g");
        assert_eq!(v["in_entry"]["counterparty_kind"], "entry");
        assert_eq!(v["out_entry"]["amount"], 500);
    }
}
