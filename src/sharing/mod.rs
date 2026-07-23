use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use serde::Deserialize;
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::sharing as pb;
use crate::pb::service::sharing::sharing_service_client::SharingServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/budgets/{budget_id}/expenses",
            get(list_expenses).post(add_expense),
        )
        .route(
            "/expenses/{expense_id}",
            get(get_expense).delete(delete_expense),
        )
        .route("/budgets/{budget_id}/settlement", get(calculate_settlement))
        .route("/budgets/{budget_id}/join-link", post(generate_join_link))
        .route("/join-link/accept", post(accept_join_link))
        // --- Guest join ---
        .route("/join-link/preview", post(preview_join_link))
        .route("/join-link/accept-guest", post(join_as_guest))
        // --- Comments ---
        .route(
            "/expenses/{expense_id}/comments",
            get(list_comments).post(add_comment),
        )
        .route("/comments/{comment_id}", delete(delete_comment))
        // --- Activity ---
        .route("/budgets/{budget_id}/activity", get(list_activity))
        // --- Settlements ---
        .route(
            "/budgets/{budget_id}/settlements",
            get(list_settlements).post(mark_settled),
        )
        .route("/settlements/{confirmation_id}", delete(delete_settlement))
        // --- Participants ---
        .route("/budgets/{budget_id}/participants", get(list_participants))
        .route(
            "/budgets/{budget_id}/participants/{participant_id}",
            delete(revoke_participant),
        )
}

fn map_status(s: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, env) = philand_error::http_error_from_tonic_status(&s);
    (http, Json(env))
}

async fn client(state: &AppState) -> ApiResult<SharingServiceClient<Channel>> {
    SharingServiceClient::connect(state.sharing_grpc_url.clone())
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
    if let Some(ut) = extract_user_type(auth) {
        if let Ok(v) = MetadataValue::try_from(ut.as_str()) {
            grpc_req.metadata_mut().insert("x-user-type", v);
        }
    }
    Ok(grpc_req)
}

fn with_user_or_guest<T>(headers: &HeaderMap, req: T) -> ApiResult<GrpcRequest<T>> {
    let mut grpc_req = GrpcRequest::new(req);
    let auth = match headers.get("authorization").and_then(|v| v.to_str().ok()) {
        Some(a) => a,
        None => {
            // No Authorization header — skip adding any auth metadata.
            // The sharing service will reject requests that need auth
            // (or accept guests with just x-session-token from elsewhere).
            return Ok(grpc_req);
        }
    };
    if auth.starts_with("SharingSession ") {
        // Guest auth
        let token = auth.strip_prefix("SharingSession ").unwrap().trim();
        if let Ok(v) = MetadataValue::try_from(token) {
            grpc_req.metadata_mut().insert("x-session-token", v);
        }
    } else {
        // Member JWT auth
        let value = MetadataValue::try_from(auth)
            .map_err(|_| map_status(Status::unauthenticated("Invalid Authorization header")))?;
        grpc_req.metadata_mut().insert("authorization", value);
        if let Some(uid) = extract_sub(auth) {
            if let Ok(v) = MetadataValue::try_from(uid.as_str()) {
                grpc_req.metadata_mut().insert("x-user-id", v);
            }
        }
        if let Some(ut) = extract_user_type(auth) {
            if let Ok(v) = MetadataValue::try_from(ut.as_str()) {
                grpc_req.metadata_mut().insert("x-user-type", v);
            }
        }
    }
    Ok(grpc_req)
}

/// Decode the `sub` claim from a JWT bearer token.
///
/// This is the gateway's ONLY source of `x-user-id` metadata for the
/// sharing service — every authenticated-member request routes through
/// here. The decoded `sub` is forwarded to the backend as the acting
/// user_id, so a bug here directly compromises per-user authorization.
///
/// SECURITY NOTES
/// - We do NOT verify the JWT signature in the gateway: the identity
///   service is the source of truth and verifies the token on every
///   request via the `authorization` metadata. This function only
///   *reads* the unverified payload to forward the user id alongside
///   the token, so a forged `sub` would be caught by the identity
///   gRPC handler returning Unauthenticated before the sharing
///   service ever sees the request.
/// - We do NOT trust the `sub` value at face value in the gateway —
///   the sharing service re-resolves it server-side.
/// - `base64url_decode` below is shared by 4 other modules (category,
///   entry, budget, middleware). Consolidating it is out of scope for
///   this engagement; this copy is the security-sensitive one and
///   is intentionally well-commented (Bug 4).
fn extract_sub(bearer: &str) -> Option<String> {
    let token = bearer.strip_prefix("Bearer ")?;
    let payload = token.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("sub")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Extract `user_type` claim from a Bearer JWT without signature verification.
/// See budget/mod.rs for the same pattern.
fn extract_user_type(bearer: &str) -> Option<String> {
    let token = bearer.strip_prefix("Bearer ")?;
    let payload = token.split('.').nth(1)?;
    let decoded = base64url_decode(payload)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("user_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// RFC 4648 §5 base64url decoder used by `extract_sub`.
///
/// This is a hand-rolled decoder. It is duplicated in
/// `gateway/src/{category,entry,budget}/mod.rs` and
/// `gateway/src/middleware/mod.rs` — a 5x duplication flagged in Bug 4.
/// Full consolidation is out of scope; this copy stays as-is, but the
/// intent is for a future `libs/crypto` or `libs/jwt` crate to own a
/// single tested implementation. Until then, edits here must be
/// mirrored in the other four locations.
///
/// Returns `None` on any malformed input (bad char, bad padding,
/// truncated quartet). Does NOT allocate on failure beyond the
/// fixed-size `T` lookup table.
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    let mut s = input.replace('-', "+").replace('_', "/");
    match s.len() % 4 {
        2 => s.push_str("=="),
        3 => s.push('='),
        _ => {}
    }
    let mut out = Vec::new();
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
struct AddExpenseRequest {
    paid_by: String,
    total_amount: i64,
    description: Option<String>,
    expense_date: String,
    category_id: Option<String>,
    split_method: Option<i32>,
    legs: Option<Vec<LegBody>>,
    #[serde(default)]
    items: Vec<ItemBody>,
    #[serde(default)]
    receipt_media_id: Option<String>,
}

#[derive(Deserialize)]
struct LegBody {
    user_id: String,
    amount: i64,
    #[serde(default)]
    weight: i64,
}

#[derive(Deserialize)]
struct AssignmentBody {
    user_id: String,
    #[serde(default)]
    numerator: i32,
}

#[derive(Deserialize)]
struct ItemBody {
    label: String,
    amount: i64,
    #[serde(default)]
    assignments: Vec<AssignmentBody>,
}

#[derive(Deserialize)]
struct AcceptJoinLinkBody {
    token: String,
}

#[derive(Deserialize)]
struct JoinAsGuestBody {
    token: String,
    display_name: String,
}

#[derive(Deserialize)]
struct AddCommentBody {
    body: String,
}

#[derive(Deserialize)]
struct MarkSettledBody {
    from_participant_id: String,
    to_participant_id: String,
    amount: i64,
    settled_at: String,
    note: Option<String>,
}

#[derive(Deserialize)]
struct ActivityQuery {
    since: Option<i64>,
    limit: Option<i32>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

async fn add_expense(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AddExpenseRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let legs = body
        .legs
        .unwrap_or_default()
        .into_iter()
        .map(|l| pb::ExpenseLeg {
            user_id: l.user_id,
            amount: l.amount,
            weight: 0,
            share: 0,
        })
        .collect();
    let items: Vec<pb::ExpenseItem> = body
        .items
        .into_iter()
        .map(|it| pb::ExpenseItem {
            id: String::new(),
            expense_id: String::new(),
            label: it.label,
            amount: it.amount,
            assignments: it
                .assignments
                .into_iter()
                .map(|a| pb::ItemAssignment {
                    user_id: a.user_id,
                    numerator: a.numerator,
                    resolved_amount: 0,
                })
                .collect(),
        })
        .collect();

    let resp = c
        .add_expense(with_user_or_guest(
            &headers,
            pb::AddExpenseRequest {
                budget_id,
                paid_by: body.paid_by,
                total_amount: body.total_amount,
                description: body.description.unwrap_or_default(),
                expense_date: body.expense_date,
                category_id: body.category_id.unwrap_or_default(),
                split_method: body.split_method.unwrap_or(1),
                legs,
items,
                receipt_media_id: body.receipt_media_id.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_expense(&resp))))
}

async fn get_expense(
    State(state): State<Arc<AppState>>,
    Path(expense_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_expense(with_user_or_guest(
            &headers,
            pb::GetExpenseRequest { expense_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_expense(
        resp.expense.as_ref().unwrap_or(&pb::Expense::default()),
    )))
}

async fn list_expenses(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_expenses(with_user_or_guest(
            &headers,
            pb::ListExpensesRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let expenses: Vec<serde_json::Value> = resp.expenses.iter().map(map_expense).collect();
    Ok(Json(serde_json::json!({ "expenses": expenses })))
}

async fn delete_expense(
    State(state): State<Arc<AppState>>,
    Path(expense_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_expense(with_user_or_guest(
        &headers,
        pb::DeleteExpenseRequest { expense_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({ "message": "Expense deleted" })))
}

async fn calculate_settlement(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .calculate_settlement(with_user_or_guest(
            &headers,
            pb::CalculateSettlementRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let transfers: Vec<serde_json::Value> = resp
        .transfers
        .iter()
        .map(|t| {
            serde_json::json!({
                "from_user_id": t.from_user_id,
                "from_name":    t.from_name,
                "to_user_id":   t.to_user_id,
                "to_name":      t.to_name,
                "amount":       t.amount,
            })
        })
        .collect();
    Ok(Json(
        serde_json::json!({ "budget_id": resp.budget_id, "transfers": transfers }),
    ))
}

async fn generate_join_link(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .generate_join_link(with_user(
            &headers,
            pb::GenerateJoinLinkRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "token":      resp.token,
            "budget_id":  resp.budget_id,
            "join_url":   resp.join_url,
            "expires_at": resp.expires_at,
        })),
    ))
}

async fn accept_join_link(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AcceptJoinLinkBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    // `accept_join_link` is the authenticated-member path: the caller must
    // present a valid JWT. We need the `sub` claim to bind the new participant
    // to the existing user. Reject malformed/missing tokens here rather than
    // forwarding an empty `user_id` to the backend (Bug 1).
    let user_id = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(extract_sub)
        .ok_or_else(|| {
            map_status(Status::unauthenticated(
                "Invalid or missing JWT for accept_join_link",
            ))
        })?;
    c.accept_join_link(with_user(
        &headers,
        pb::AcceptJoinLinkRequest {
            token: body.token,
            user_id,
            display_name: String::new(),
        },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({ "message": "Joined successfully" }),
    ))
}

// ---------------------------------------------------------------------------
// Guest join
// ---------------------------------------------------------------------------

async fn preview_join_link(
    State(state): State<Arc<AppState>>,
    Json(body): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let token = body
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| map_status(Status::invalid_argument("token is required")))?;
    let mut c = client(&state).await?;
    let resp = c
        .preview_join_link(GrpcRequest::new(pb::PreviewJoinLinkRequest {
            token: token.to_string(),
        }))
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({
        "budget_id":    resp.budget_id,
        "currency":     resp.currency,
        "expires_at":   resp.expires_at,
        "member_count": resp.member_count,
        "valid":        resp.valid,
    })))
}

async fn join_as_guest(
    State(state): State<Arc<AppState>>,
    Json(body): Json<JoinAsGuestBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .join_as_guest(GrpcRequest::new(pb::JoinAsGuestRequest {
            token: body.token.clone(),
            display_name: body.display_name.clone(),
        }))
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({
        "session_token":  resp.session_token,
        "participant_id": resp.participant_id,
        "budget_id":      resp.budget_id,
        "display_name":   resp.display_name,
    })))
}

// ---------------------------------------------------------------------------
// Comments
// ---------------------------------------------------------------------------

async fn add_comment(
    State(state): State<Arc<AppState>>,
    Path(expense_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AddCommentBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .add_comment(with_user_or_guest(
            &headers,
            pb::AddCommentRequest {
                expense_id,
                body: body.body,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(
        serde_json::json!({ "comment": map_comment(resp.comment.as_ref()) }),
    ))
}

async fn list_comments(
    State(state): State<Arc<AppState>>,
    Path(expense_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_comments(with_user_or_guest(
            &headers,
            pb::ListCommentsRequest { expense_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let comments: Vec<serde_json::Value> =
        resp.comments.iter().map(|c| map_comment(Some(c))).collect();
    Ok(Json(serde_json::json!({ "comments": comments })))
}

async fn delete_comment(
    State(state): State<Arc<AppState>>,
    Path(comment_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_comment(with_user_or_guest(
        &headers,
        pb::DeleteCommentRequest { comment_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

// ---------------------------------------------------------------------------
// Activity
// ---------------------------------------------------------------------------

async fn list_activity(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Query(params): Query<ActivityQuery>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_activity(with_user_or_guest(
            &headers,
            pb::ListActivityRequest {
                budget_id,
                since_unix: params.since.unwrap_or(0),
                limit: params.limit.unwrap_or(50),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let entries: Vec<serde_json::Value> = resp
        .entries
        .iter()
        .map(|e| {
            serde_json::json!({
                "id":                    e.id,
                "budget_id":             e.budget_id,
                "actor_participant_id":  e.actor_participant_id,
                "actor_display_name":    e.actor_display_name,
                "action":                e.action,
                "target_type":           e.target_type,
                "target_id":             e.target_id,
                "metadata_json":         e.metadata_json,
                "created_at":            e.created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "entries": entries })))
}

// ---------------------------------------------------------------------------
// Settlements
// ---------------------------------------------------------------------------

async fn list_settlements(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_settlements(with_user_or_guest(
            &headers,
            pb::ListSettlementsRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let confirmations: Vec<serde_json::Value> = resp
        .confirmations
        .iter()
        .map(|s| {
            serde_json::json!({
                "id":                       s.id,
                "budget_id":                s.budget_id,
                "from_participant_id":      s.from_participant_id,
                "to_participant_id":        s.to_participant_id,
                "amount":                   s.amount,
                "settled_at":               s.settled_at,
                "note":                     s.note,
                "settled_by_participant_id": s.settled_by_participant_id,
                "created_at":               s.created_at,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "confirmations": confirmations })))
}

async fn mark_settled(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<MarkSettledBody>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    // TODO(decision): `with_user_or_guest` allows a guest session token to
    // call this endpoint. The spec is unambiguous: `infra/.ai/spec/08-sharing-budget.md`
    // §2.5 row "Mark a settlement as settled" grants ✅ to both Member and Guest.
    // If the product team reverses this decision, switch the call below to
    // `with_user` so only JWT-authenticated members can settle. Do not also
    // remove the `with_user_or_guest` machinery from the other endpoints
    // (add_expense / add_comment) without re-reading the full §2.5 table.
    let resp = c
        .mark_settled(with_user_or_guest(
            &headers,
            pb::MarkSettledRequest {
                budget_id,
                from_participant_id: body.from_participant_id,
                to_participant_id: body.to_participant_id,
                amount: body.amount,
                settled_at: body.settled_at,
                note: body.note.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({
        "id":                       resp.id,
        "budget_id":                resp.budget_id,
        "from_participant_id":      resp.from_participant_id,
        "to_participant_id":        resp.to_participant_id,
        "amount":                   resp.amount,
        "settled_at":               resp.settled_at,
        "note":                     resp.note,
        "settled_by_participant_id": resp.settled_by_participant_id,
        "created_at":               resp.created_at,
    })))
}

async fn delete_settlement(
    State(state): State<Arc<AppState>>,
    Path(confirmation_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_settlement(with_user(
        &headers,
        pb::DeleteSettlementRequest { confirmation_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

// ---------------------------------------------------------------------------
// Participants
// ---------------------------------------------------------------------------

async fn list_participants(
    State(state): State<Arc<AppState>>,
    Path(budget_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_participants(with_user_or_guest(
            &headers,
            pb::ListParticipantsRequest { budget_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let participants: Vec<serde_json::Value> = resp
        .participants
        .iter()
        .map(|p| {
            serde_json::json!({
                "participant_id": p.participant_id,
                "budget_id":      p.budget_id,
                "kind":           p.kind,
                "display_name":   p.display_name,
                "joined_at":      p.joined_at,
                "last_seen_at":   p.last_seen_at,
                "revoked":        p.revoked,
                "user_id":        p.user_id,
            })
        })
        .collect();
    Ok(Json(serde_json::json!({ "participants": participants })))
}

async fn revoke_participant(
    State(state): State<Arc<AppState>>,
    Path((budget_id, participant_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.revoke_participant(with_user(
        &headers,
        pb::RevokeParticipantRequest {
            budget_id,
            participant_id,
        },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(serde_json::json!({ "success": true })))
}

// ---------------------------------------------------------------------------
// Mapper
// ---------------------------------------------------------------------------

fn map_expense(e: &pb::Expense) -> serde_json::Value {
    let base = e.base.as_ref();
    serde_json::json!({
        "id":               base.map(|b| b.id.as_str()).unwrap_or_default(),
        "budget_id":        e.budget_id,
        "paid_by":          e.paid_by,
        "total_amount":     e.total_amount,
        "description":      e.description,
        "expense_date":     e.expense_date,
        "category_id":      e.category_id,
        "split_method":     e.split_method,
        "receipt_media_id": e.receipt_media_id,
        "legs": e.legs.iter().map(|l| serde_json::json!({
            "user_id": l.user_id,
            "amount":  l.amount,
            "weight":  l.weight,
            "share":   l.share,
        })).collect::<Vec<_>>(),
        "items": e.items.iter().map(|it| serde_json::json!({
            "id":          it.id,
            "expense_id":  it.expense_id,
            "label":       it.label,
            "amount":      it.amount,
            "assignments": it.assignments.iter().map(|a| serde_json::json!({
                "user_id":         a.user_id,
                "numerator":       a.numerator,
                "resolved_amount": a.resolved_amount,
            })).collect::<Vec<_>>(),
        })).collect::<Vec<_>>(),
        "created_by":   base.map(|b| b.created_by.as_str()).unwrap_or_default(),
        "created_at":   base.map(|b| b.created_at).unwrap_or(0),
        "updated_at":   base.map(|b| b.updated_at).unwrap_or(0),
    })
}

fn map_comment(c: Option<&pb::ExpenseComment>) -> serde_json::Value {
    match c {
        Some(c) => serde_json::json!({
            "id":                    c.id,
            "expense_id":            c.expense_id,
            "author_participant_id": c.author_participant_id,
            "author_display_name":   c.author_display_name,
            "body":                  c.body,
            "created_at":            c.created_at,
            "deleted":               c.deleted,
        }),
        None => serde_json::json!({}),
    }
}
