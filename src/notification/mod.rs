//! REST facade over the notification gRPC service.
//!
//! - `router()`       → mounted at `/api/notification` (any signed-in user)
//! - `admin_router()` → mounted at `/api/admin/notification` (super admin; the service
//!   re-checks the role). `/api/admin/*` is the path family super admins may call.
//!
//! The internal `Publish` RPC is NEVER exposed here: it is called service-to-service with
//! `x-internal-token`.
use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{get, post, put},
    Json, Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use tonic::{transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::notification as pb;
use crate::pb::service::notification::notification_service_client::NotificationServiceClient;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/inbox", get(list_inbox))
        .route("/unread-count", get(unread_count))
        .route("/read", post(mark_read))
        .route("/read-all", post(mark_all_read))
        .route("/archive", post(archive))
        .route("/banners", get(list_banners))
        .route("/preferences", get(get_preferences).put(update_preferences))
        .route("/budgets/{budget_id}/mute", put(set_budget_mute))
        // Public (no JWT): the signed token in an email is the credential.
        .route(
            "/unsubscribe",
            get(unsubscribe_page).post(unsubscribe_submit),
        )
}

pub fn admin_router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/broadcasts",
            get(admin_list_broadcasts).post(admin_create_broadcast),
        )
        .route(
            "/broadcasts/{broadcast_id}",
            axum::routing::patch(admin_update_broadcast),
        )
        .route(
            "/broadcasts/{broadcast_id}/publish",
            post(admin_publish_broadcast),
        )
        .route(
            "/broadcasts/{broadcast_id}/archive",
            post(admin_archive_broadcast),
        )
}

// ---------------------------------------------------------------------------
// public: email unsubscribe
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct UnsubscribeParams {
    #[serde(default)]
    token: String,
}

fn page(body: &str) -> axum::response::Html<String> {
    axum::response::Html(format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><meta name=\"viewport\" \
         content=\"width=device-width,initial-scale=1\"><title>Philandz</title></head>\
         <body style=\"font-family:Arial,Helvetica,sans-serif;max-width:480px;margin:48px auto;\
         padding:0 16px;color:#111\">{body}</body></html>"
    ))
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

/// GET only shows a confirmation button: link scanners that prefetch emails must not be able to
/// unsubscribe anyone.
async fn unsubscribe_page(Query(q): Query<UnsubscribeParams>) -> axum::response::Html<String> {
    page(&format!(
        "<h2>Ngừng nhận email? / Stop these emails?</h2>\
         <p>Bạn sẽ không nhận email thuộc loại thông báo này nữa. Thông báo trong ứng dụng không đổi.</p>\
         <p>You will stop receiving emails of this kind. In-app notifications are unchanged.</p>\
         <form method=\"post\" action=\"\"><input type=\"hidden\" name=\"token\" value=\"{}\">\
         <button type=\"submit\" style=\"padding:10px 18px;border-radius:6px;border:0;\
         background:#0f766e;color:#fff;font-size:16px\">Xác nhận / Confirm</button></form>",
        html_escape(&q.token)
    ))
}

async fn unsubscribe_submit(
    State(state): State<Arc<AppState>>,
    axum::Form(form): axum::Form<UnsubscribeParams>,
) -> (StatusCode, axum::response::Html<String>) {
    let result = async {
        let mut c = client(&state).await.map_err(|_| ())?;
        c.unsubscribe_email(GrpcRequest::new(pb::UnsubscribeEmailRequest {
            token: form.token,
        }))
        .await
        .map_err(|_| ())
    }
    .await;
    match result {
        Ok(_) => (
            StatusCode::OK,
            page(
                "<h2>Đã xong / Done</h2>\
                 <p>Bạn sẽ không còn nhận email loại này. Bạn có thể bật lại trong Cài đặt thông báo.</p>\
                 <p>You will no longer get these emails. You can turn them back on in Notification settings.</p>",
            ),
        ),
        Err(()) => (
            StatusCode::BAD_REQUEST,
            page(
                "<h2>Liên kết không hợp lệ / Invalid link</h2>\
                 <p>Liên kết đã hết hạn hoặc không đúng. Hãy đổi trong Cài đặt thông báo.</p>\
                 <p>This link is invalid or expired. Please change it in Notification settings.</p>",
            ),
        ),
    }
}

// ---------------------------------------------------------------------------
// plumbing
// ---------------------------------------------------------------------------

fn map_status(status: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, envelope) = philand_error::http_error_from_tonic_status(&status);
    (http, Json(envelope))
}

fn bad_request(msg: &str) -> (StatusCode, Json<ErrorResponse>) {
    map_status(Status::invalid_argument(msg))
}

async fn client(state: &AppState) -> ApiResult<NotificationServiceClient<Channel>> {
    NotificationServiceClient::connect(state.notification_grpc_url.clone())
        .await
        .map_err(|e| map_status(Status::unavailable(e.to_string())))
}

fn with_user<T>(state: &AppState, headers: &HeaderMap, req: T) -> ApiResult<GrpcRequest<T>> {
    // SECURITY: verify the bearer token before forwarding the `sub` claim —
    // the notification service trusts `x-user-id` / `x-user-type` at face value.
    crate::auth::request_with_user(&state.jwt_secret, headers, req)
        .map_err(|e| map_status(Status::unauthenticated(e.message())))
}

async fn health(State(state): State<Arc<AppState>>) -> ApiResult<&'static str> {
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        NotificationServiceClient::connect(state.notification_grpc_url.clone()),
    )
    .await;
    match res {
        Ok(Ok(_)) => Ok("OK"),
        Ok(Err(e)) => Err(map_status(Status::unavailable(e.to_string()))),
        Err(_) => Err(map_status(Status::deadline_exceeded(
            "Notification gRPC connect timed out",
        ))),
    }
}

// ---------------------------------------------------------------------------
// enum <-> label mapping (the REST DTO uses lowercase labels, never raw ints)
// ---------------------------------------------------------------------------

fn category_label(v: i32) -> &'static str {
    match pb::NotificationCategory::try_from(v).unwrap_or(pb::NotificationCategory::Unspecified) {
        pb::NotificationCategory::Activity => "activity",
        pb::NotificationCategory::Membership => "membership",
        pb::NotificationCategory::Reminder => "reminder",
        pb::NotificationCategory::Alert => "alert",
        pb::NotificationCategory::Security => "security",
        pb::NotificationCategory::System => "system",
        pb::NotificationCategory::Unspecified => "unspecified",
    }
}

fn category_parse(s: &str) -> Option<i32> {
    Some(match s {
        "activity" => pb::NotificationCategory::Activity,
        "membership" => pb::NotificationCategory::Membership,
        "reminder" => pb::NotificationCategory::Reminder,
        "alert" => pb::NotificationCategory::Alert,
        "security" => pb::NotificationCategory::Security,
        "system" => pb::NotificationCategory::System,
        _ => return None,
    } as i32)
}

fn severity_label(v: i32) -> &'static str {
    match pb::NotificationSeverity::try_from(v).unwrap_or(pb::NotificationSeverity::Unspecified) {
        pb::NotificationSeverity::Warning => "warning",
        pb::NotificationSeverity::Critical => "critical",
        _ => "info",
    }
}

fn severity_parse(s: &str) -> Option<i32> {
    Some(match s {
        "info" => pb::NotificationSeverity::Info,
        "warning" => pb::NotificationSeverity::Warning,
        "critical" => pb::NotificationSeverity::Critical,
        _ => return None,
    } as i32)
}

fn channel_label(v: i32) -> &'static str {
    match pb::NotificationChannel::try_from(v).unwrap_or(pb::NotificationChannel::Unspecified) {
        pb::NotificationChannel::InApp => "in_app",
        pb::NotificationChannel::Email => "email",
        pb::NotificationChannel::Telegram => "telegram",
        pb::NotificationChannel::Unspecified => "unspecified",
    }
}

fn channel_parse(s: &str) -> Option<i32> {
    Some(match s {
        "in_app" => pb::NotificationChannel::InApp,
        "email" => pb::NotificationChannel::Email,
        "telegram" => pb::NotificationChannel::Telegram,
        _ => return None,
    } as i32)
}

fn kind_label(v: i32) -> &'static str {
    match pb::NotificationKind::try_from(v).unwrap_or(pb::NotificationKind::Unspecified) {
        pb::NotificationKind::System => "system",
        _ => "personal",
    }
}

fn kind_parse(s: &str) -> Option<i32> {
    Some(match s {
        "personal" => pb::NotificationKind::Personal,
        "system" => pb::NotificationKind::System,
        _ => return None,
    } as i32)
}

fn audience_label(v: i32) -> &'static str {
    match pb::BroadcastAudience::try_from(v).unwrap_or(pb::BroadcastAudience::Unspecified) {
        pb::BroadcastAudience::Users => "users",
        pb::BroadcastAudience::Orgs => "orgs",
        _ => "all",
    }
}

fn audience_parse(s: &str) -> Option<i32> {
    Some(match s {
        "all" => pb::BroadcastAudience::All,
        "users" => pb::BroadcastAudience::Users,
        "orgs" => pb::BroadcastAudience::Orgs,
        _ => return None,
    } as i32)
}

fn status_label(v: i32) -> &'static str {
    match pb::BroadcastStatus::try_from(v).unwrap_or(pb::BroadcastStatus::Unspecified) {
        pb::BroadcastStatus::Draft => "draft",
        pb::BroadcastStatus::Scheduled => "scheduled",
        pb::BroadcastStatus::Published => "published",
        pb::BroadcastStatus::Archived => "archived",
        pb::BroadcastStatus::Unspecified => "unspecified",
    }
}

fn status_parse(s: &str) -> Option<i32> {
    Some(match s {
        "draft" => pb::BroadcastStatus::Draft,
        "scheduled" => pb::BroadcastStatus::Scheduled,
        "published" => pb::BroadcastStatus::Published,
        "archived" => pb::BroadcastStatus::Archived,
        _ => return None,
    } as i32)
}

fn map_item(i: &pb::InboxItem) -> Value {
    // `params_json` is a JSON object serialised by the producer; hand it to the web as an
    // object so the client does not need a second parse. Fall back to {} if malformed.
    let params: Value = serde_json::from_str(&i.params_json).unwrap_or_else(|_| json!({}));
    json!({
        "id": i.id,
        "kind": kind_label(i.kind),
        "category": category_label(i.category),
        "severity": severity_label(i.severity),
        "type": i.r#type,
        "params": params,
        "title": i.title,
        "body": i.body,
        "fallback_text": i.fallback_text,
        "link": i.link,
        "actor_user_id": i.actor_user_id,
        "org_id": i.org_id,
        "budget_id": i.budget_id,
        "group_count": i.group_count,
        "created_at": i.created_at,
        "updated_at": i.updated_at,
        "read_at": i.read_at,
        "is_read": i.read_at > 0,
    })
}

fn map_prefs(p: &pb::Preferences) -> Value {
    let cells: Vec<Value> = p
        .cells
        .iter()
        .map(|c| {
            json!({
                "category": category_label(c.category),
                "channel": channel_label(c.channel),
                "enabled": c.enabled,
                "locked": c.locked,
            })
        })
        .collect();
    json!({"cells": cells, "muted_budget_ids": p.muted_budget_ids})
}

fn map_broadcast(b: &pb::Broadcast) -> Value {
    json!({
        "id": b.id,
        "audience": audience_label(b.audience),
        "audience_user_ids": b.audience_user_ids,
        "audience_org_ids": b.audience_org_ids,
        "severity": severity_label(b.severity),
        "title_vi": b.title_vi, "body_vi": b.body_vi,
        "title_en": b.title_en, "body_en": b.body_en,
        "link": b.link,
        "status": status_label(b.status),
        "publish_at": b.publish_at,
        "expires_at": b.expires_at,
        "show_banner": b.show_banner,
        "created_by": b.created_by,
        "created_at": b.created_at,
        "updated_at": b.updated_at,
        "read_count": b.read_count,
    })
}

// ---------------------------------------------------------------------------
// request DTOs
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
struct InboxQuery {
    filter: Option<String>,
    kind: Option<String>,
    cursor: Option<String>,
    limit: Option<i32>,
    locale: Option<String>,
}

#[derive(Deserialize)]
struct LocaleQuery {
    locale: Option<String>,
}

#[derive(Deserialize, Default)]
struct IdsBody {
    #[serde(default)]
    item_ids: Vec<String>,
    #[serde(default)]
    broadcast_ids: Vec<String>,
}

#[derive(Deserialize, Default)]
struct ReadAllBody {
    kind: Option<String>,
}

#[derive(Deserialize)]
struct PrefCellBody {
    category: String,
    channel: String,
    enabled: bool,
}

#[derive(Deserialize)]
struct UpdatePrefsBody {
    #[serde(default)]
    cells: Vec<PrefCellBody>,
}

#[derive(Deserialize)]
struct MuteBody {
    muted: bool,
}

#[derive(Deserialize)]
struct BroadcastBody {
    audience: Option<String>,
    #[serde(default)]
    audience_user_ids: Vec<String>,
    #[serde(default)]
    audience_org_ids: Vec<String>,
    severity: Option<String>,
    #[serde(default)]
    title_vi: String,
    #[serde(default)]
    body_vi: String,
    #[serde(default)]
    title_en: String,
    #[serde(default)]
    body_en: String,
    #[serde(default)]
    link: String,
    #[serde(default)]
    publish_at: i64,
    #[serde(default)]
    expires_at: i64,
    #[serde(default)]
    show_banner: bool,
}

#[derive(Deserialize, Default)]
struct AdminListQuery {
    status: Option<String>,
    page: Option<i32>,
    limit: Option<i32>,
}

fn to_input(b: BroadcastBody) -> ApiResult<pb::BroadcastInput> {
    let audience = match b.audience.as_deref() {
        None => pb::BroadcastAudience::All as i32,
        Some(s) => audience_parse(s).ok_or_else(|| bad_request("invalid audience"))?,
    };
    let severity = match b.severity.as_deref() {
        None => pb::NotificationSeverity::Info as i32,
        Some(s) => severity_parse(s).ok_or_else(|| bad_request("invalid severity"))?,
    };
    Ok(pb::BroadcastInput {
        audience,
        audience_user_ids: b.audience_user_ids,
        audience_org_ids: b.audience_org_ids,
        severity,
        title_vi: b.title_vi,
        body_vi: b.body_vi,
        title_en: b.title_en,
        body_en: b.body_en,
        link: b.link,
        publish_at: b.publish_at,
        expires_at: b.expires_at,
        show_banner: b.show_banner,
    })
}

// ---------------------------------------------------------------------------
// user handlers
// ---------------------------------------------------------------------------

async fn list_inbox(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<InboxQuery>,
) -> ApiResult<Json<Value>> {
    let filter = match q.filter.as_deref() {
        None | Some("all") => pb::InboxFilter::All,
        Some("unread") => pb::InboxFilter::Unread,
        Some(_) => return Err(bad_request("invalid filter")),
    } as i32;
    let kind = match q.kind.as_deref() {
        None | Some("") => pb::NotificationKind::Unspecified as i32,
        Some(s) => kind_parse(s).ok_or_else(|| bad_request("invalid kind"))?,
    };
    let mut c = client(&state).await?;
    let resp = c
        .list_inbox(with_user(
            &state,
            &headers,
            pb::ListInboxRequest {
                filter,
                kind,
                cursor: q.cursor.unwrap_or_default(),
                limit: q.limit.unwrap_or(0),
                locale: q.locale.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let items: Vec<Value> = resp.items.iter().map(map_item).collect();
    Ok(Json(
        json!({"items": items, "next_cursor": resp.next_cursor}),
    ))
}

async fn unread_count(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let mut c = client(&state).await?;
    let r = c
        .get_unread_count(with_user(&state, &headers, pb::GetUnreadCountRequest {})?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(
        json!({"personal": r.personal, "system": r.system, "total": r.total}),
    ))
}

async fn mark_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<IdsBody>,
) -> ApiResult<StatusCode> {
    let mut c = client(&state).await?;
    c.mark_read(with_user(
        &state,
        &headers,
        pb::MarkReadRequest {
            item_ids: body.item_ids,
            broadcast_ids: body.broadcast_ids,
        },
    )?)
    .await
    .map_err(map_status)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn mark_all_read(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Option<Json<ReadAllBody>>,
) -> ApiResult<StatusCode> {
    let kind = match body.and_then(|b| b.0.kind) {
        None => pb::NotificationKind::Unspecified as i32,
        Some(s) => kind_parse(&s).ok_or_else(|| bad_request("invalid kind"))?,
    };
    let mut c = client(&state).await?;
    c.mark_all_read(with_user(
        &state,
        &headers,
        pb::MarkAllReadRequest { kind },
    )?)
    .await
    .map_err(map_status)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn archive(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<IdsBody>,
) -> ApiResult<StatusCode> {
    let mut c = client(&state).await?;
    c.archive(with_user(
        &state,
        &headers,
        pb::ArchiveRequest {
            item_ids: body.item_ids,
            broadcast_ids: body.broadcast_ids,
        },
    )?)
    .await
    .map_err(map_status)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn list_banners(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<LocaleQuery>,
) -> ApiResult<Json<Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_active_banners(with_user(
            &state,
            &headers,
            pb::ListActiveBannersRequest {
                locale: q.locale.unwrap_or_default(),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let banners: Vec<Value> = resp
        .banners
        .iter()
        .map(|b| {
            json!({
                "broadcast_id": b.broadcast_id,
                "severity": severity_label(b.severity),
                "title": b.title, "body": b.body, "link": b.link,
                "dismissible": b.dismissible,
                "expires_at": b.expires_at,
            })
        })
        .collect();
    Ok(Json(json!({"banners": banners})))
}

async fn get_preferences(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<Value>> {
    let mut c = client(&state).await?;
    let p = c
        .get_preferences(with_user(&state, &headers, pb::GetPreferencesRequest {})?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_prefs(&p)))
}

async fn update_preferences(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UpdatePrefsBody>,
) -> ApiResult<Json<Value>> {
    let mut cells = Vec::with_capacity(body.cells.len());
    for c in body.cells {
        cells.push(pb::PreferenceCell {
            category: category_parse(&c.category).ok_or_else(|| bad_request("invalid category"))?,
            channel: channel_parse(&c.channel).ok_or_else(|| bad_request("invalid channel"))?,
            enabled: c.enabled,
            locked: false,
        });
    }
    let mut c = client(&state).await?;
    let p = c
        .update_preferences(with_user(
            &state,
            &headers,
            pb::UpdatePreferencesRequest { cells },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_prefs(&p)))
}

async fn set_budget_mute(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(budget_id): Path<String>,
    Json(body): Json<MuteBody>,
) -> ApiResult<Json<Value>> {
    let mut c = client(&state).await?;
    let p = c
        .set_budget_mute(with_user(
            &state,
            &headers,
            pb::SetBudgetMuteRequest {
                budget_id,
                muted: body.muted,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_prefs(&p)))
}

// ---------------------------------------------------------------------------
// admin handlers
// ---------------------------------------------------------------------------

async fn admin_list_broadcasts(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(q): Query<AdminListQuery>,
) -> ApiResult<Json<Value>> {
    let status = match q.status.as_deref() {
        None | Some("") => pb::BroadcastStatus::Unspecified as i32,
        Some(s) => status_parse(s).ok_or_else(|| bad_request("invalid status"))?,
    };
    let mut c = client(&state).await?;
    let r = c
        .admin_list_broadcasts(with_user(
            &state,
            &headers,
            pb::AdminListBroadcastsRequest {
                status,
                page: q.page.unwrap_or(1),
                limit: q.limit.unwrap_or(20),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    let items: Vec<Value> = r.broadcasts.iter().map(map_broadcast).collect();
    Ok(Json(json!({"broadcasts": items, "total": r.total})))
}

async fn admin_create_broadcast(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<BroadcastBody>,
) -> ApiResult<(StatusCode, Json<Value>)> {
    let input = to_input(body)?;
    let mut c = client(&state).await?;
    let b = c
        .admin_create_broadcast(with_user(
            &state,
            &headers,
            pb::AdminCreateBroadcastRequest { input: Some(input) },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((StatusCode::CREATED, Json(map_broadcast(&b))))
}

async fn admin_update_broadcast(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(broadcast_id): Path<String>,
    Json(body): Json<BroadcastBody>,
) -> ApiResult<Json<Value>> {
    let input = to_input(body)?;
    let mut c = client(&state).await?;
    let b = c
        .admin_update_broadcast(with_user(
            &state,
            &headers,
            pb::AdminUpdateBroadcastRequest {
                broadcast_id,
                input: Some(input),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_broadcast(&b)))
}

async fn admin_publish_broadcast(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(broadcast_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut c = client(&state).await?;
    let b = c
        .admin_publish_broadcast(with_user(
            &state,
            &headers,
            pb::AdminBroadcastRef { broadcast_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_broadcast(&b)))
}

async fn admin_archive_broadcast(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Path(broadcast_id): Path<String>,
) -> ApiResult<Json<Value>> {
    let mut c = client(&state).await?;
    let b = c
        .admin_archive_broadcast(with_user(
            &state,
            &headers,
            pb::AdminBroadcastRef { broadcast_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(map_broadcast(&b)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enum_labels_round_trip() {
        for s in [
            "activity",
            "membership",
            "reminder",
            "alert",
            "security",
            "system",
        ] {
            assert_eq!(category_label(category_parse(s).unwrap()), s);
        }
        for s in ["info", "warning", "critical"] {
            assert_eq!(severity_label(severity_parse(s).unwrap()), s);
        }
        for s in ["in_app", "email", "telegram"] {
            assert_eq!(channel_label(channel_parse(s).unwrap()), s);
        }
        for s in ["all", "users", "orgs"] {
            assert_eq!(audience_label(audience_parse(s).unwrap()), s);
        }
        for s in ["draft", "scheduled", "published", "archived"] {
            assert_eq!(status_label(status_parse(s).unwrap()), s);
        }
        assert!(category_parse("nope").is_none());
    }

    #[test]
    fn item_params_become_an_object() {
        let item = pb::InboxItem {
            params_json: r#"{"amount":5}"#.into(),
            ..Default::default()
        };
        assert_eq!(map_item(&item)["params"]["amount"], 5);
        let bad = pb::InboxItem {
            params_json: "not json".into(),
            ..Default::default()
        };
        assert_eq!(map_item(&bad)["params"], json!({}));
    }

    #[test]
    fn broadcast_body_rejects_unknown_audience() {
        let b: BroadcastBody =
            serde_json::from_value(json!({"audience": "everyone", "title_vi": "x"})).unwrap();
        assert!(to_input(b).is_err());
    }
}
