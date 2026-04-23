use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::Deserialize;
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::entry as pb;
use crate::pb::service::entry::entry_service_client::EntryServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
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

fn map_status(s: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, env) = philand_error::http_error_from_tonic_status(&s);
    (http, Json(env))
}

async fn client(state: &AppState) -> ApiResult<EntryServiceClient<Channel>> {
    EntryServiceClient::connect(state.entry_grpc_url.clone())
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
    if let Some(uid) = extract_sub(auth) {
        if let Ok(v) = MetadataValue::try_from(uid.as_str()) {
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
    claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn base64url_decode(input: &str) -> Option<Vec<u8>> {
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
        while i < 26 {
            t[(b'A' + i) as usize] = i;
            i += 1;
        }
        i = 0;
        while i < 26 {
            t[(b'a' + i) as usize] = 26 + i;
            i += 1;
        }
        i = 0;
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
        .get_entry(with_user(&headers, pb::GetEntryRequest { entry_id })?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_entry(
        resp.entry.as_ref().unwrap_or(&pb::Entry::default()),
    )))
}

async fn list_entries(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    Query(q): Query<ListEntriesQuery>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let params = pb::ListParams {
        q: q.q,
        kind: q.kind,
        category_id: q.category_id,
        date_from: q.date_from,
        date_to: q.date_to,
        amount_min: q.amount_min,
        amount_max: q.amount_max,
        tags: q.tags,
        sort_by: q.sort_by,
        sort_dir: q.sort_dir,
        page: q.page,
        page_size: q.page_size,
    };
    let resp = c
        .list_entries(with_user(
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
    Ok(Json(serde_json::json!({"entries": entries, "meta": meta})))
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
    let params = pb::ListParams {
        q: q.q,
        kind: q.kind,
        category_id: q.category_id,
        date_from: q.date_from,
        date_to: q.date_to,
        amount_min: q.amount_min,
        amount_max: q.amount_max,
        tags: q.tags,
        sort_by: q.sort_by,
        sort_dir: q.sort_dir,
        page: q.page,
        page_size: q.page_size,
    };
    let resp = c
        .list_entries(with_user(
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
    Ok(Json(serde_json::json!({"entries": entries, "meta": meta})))
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
    c.delete_entry(with_user(&headers, pb::DeleteEntryRequest { entry_id })?)
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
        .list_comments(with_user(&headers, pb::ListCommentsRequest { entry_id })?)
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
        .list_split_legs(with_user(&headers, pb::ListSplitLegsRequest { entry_id })?)
        .await
        .map_err(map_status)?
        .into_inner();
    let legs: Vec<serde_json::Value> = resp.legs.iter().map(map_entry).collect();
    Ok(Json(
        serde_json::json!({"split_group_id": resp.split_group_id, "legs": legs}),
    ))
}

// ---------------------------------------------------------------------------
// Mappers
// ---------------------------------------------------------------------------

fn map_entry(e: &pb::Entry) -> serde_json::Value {
    serde_json::json!({
        "id": e.id, "budget_id": e.budget_id, "category_id": e.category_id,
        "kind": e.kind, "amount": e.amount, "description": e.description,
        "entry_date": e.entry_date, "tags": e.tags, "notes": e.notes,
        "is_recurring": e.is_recurring, "has_attachment": e.has_attachment,
        "recurrence_rule": e.recurrence_rule, "next_occurrence": e.next_occurrence,
        "split_group_id": e.split_group_id, "split_total": e.split_total,
        "created_by": e.created_by, "created_at": e.created_at, "updated_at": e.updated_at,
    })
}

fn map_comment(c: &pb::Comment) -> serde_json::Value {
    serde_json::json!({
        "id": c.id, "entry_id": c.entry_id, "body": c.body,
        "created_by": c.created_by, "created_at": c.created_at, "updated_at": c.updated_at,
    })
}

fn map_attachment(a: &pb::Attachment) -> serde_json::Value {
    serde_json::json!({
        "id": a.id, "entry_id": a.entry_id, "file_id": a.file_id,
        "file_name": a.file_name, "created_by": a.created_by, "created_at": a.created_at,
    })
}
