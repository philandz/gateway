//! Gateway ↔ notification gRPC: REST mapping, identity metadata and super-admin routing.
use gateway::pb::service::notification as pb;
use gateway::pb::service::notification::notification_service_server::{
    NotificationService, NotificationServiceServer,
};
use parking_lot::Mutex;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

const SECRET: &str = "test-secret";

fn jwt(user_type: &str) -> String {
    let claims = serde_json::json!({
        "sub": Uuid::new_v4().to_string(),
        "email": "u@philand.test",
        "org_id": "00000000-0000-0000-0000-000000000001",
        "user_type": user_type,
        "exp": 9_999_999_999u64,
    });
    jsonwebtoken::encode(
        &jsonwebtoken::Header::default(),
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .unwrap()
}

type Seen = Arc<Mutex<Vec<(String, String)>>>; // (x-user-type, rpc)

struct Fake(Seen);

fn ut<T>(r: &Request<T>) -> String {
    r.metadata()
        .get("x-user-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string()
}

#[tonic::async_trait]
impl NotificationService for Fake {
    async fn list_inbox(
        &self,
        r: Request<pb::ListInboxRequest>,
    ) -> Result<Response<pb::ListInboxResponse>, Status> {
        self.0.lock().push((ut(&r), "list_inbox".into()));
        Ok(Response::new(pb::ListInboxResponse {
            items: vec![pb::InboxItem {
                id: "n1".into(),
                kind: pb::NotificationKind::Personal as i32,
                category: pb::NotificationCategory::Reminder as i32,
                severity: pb::NotificationSeverity::Warning as i32,
                r#type: "debt.payment_due".into(),
                params_json: r#"{"amount":5}"#.into(),
                ..Default::default()
            }],
            next_cursor: String::new(),
        }))
    }
    async fn get_unread_count(
        &self,
        _: Request<pb::GetUnreadCountRequest>,
    ) -> Result<Response<pb::UnreadCount>, Status> {
        Ok(Response::new(pb::UnreadCount {
            personal: 2,
            system: 1,
            total: 3,
        }))
    }
    async fn mark_read(&self, _: Request<pb::MarkReadRequest>) -> Result<Response<()>, Status> {
        Ok(Response::new(()))
    }
    async fn mark_all_read(
        &self,
        _: Request<pb::MarkAllReadRequest>,
    ) -> Result<Response<()>, Status> {
        Ok(Response::new(()))
    }
    async fn archive(&self, _: Request<pb::ArchiveRequest>) -> Result<Response<()>, Status> {
        Ok(Response::new(()))
    }
    async fn list_active_banners(
        &self,
        _: Request<pb::ListActiveBannersRequest>,
    ) -> Result<Response<pb::ListActiveBannersResponse>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn get_preferences(
        &self,
        _: Request<pb::GetPreferencesRequest>,
    ) -> Result<Response<pb::Preferences>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn update_preferences(
        &self,
        _: Request<pb::UpdatePreferencesRequest>,
    ) -> Result<Response<pb::Preferences>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn set_budget_mute(
        &self,
        _: Request<pb::SetBudgetMuteRequest>,
    ) -> Result<Response<pb::Preferences>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn admin_create_broadcast(
        &self,
        _: Request<pb::AdminCreateBroadcastRequest>,
    ) -> Result<Response<pb::Broadcast>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn admin_update_broadcast(
        &self,
        _: Request<pb::AdminUpdateBroadcastRequest>,
    ) -> Result<Response<pb::Broadcast>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn admin_publish_broadcast(
        &self,
        _: Request<pb::AdminBroadcastRef>,
    ) -> Result<Response<pb::Broadcast>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn admin_archive_broadcast(
        &self,
        _: Request<pb::AdminBroadcastRef>,
    ) -> Result<Response<pb::Broadcast>, Status> {
        Ok(Response::new(Default::default()))
    }
    async fn admin_list_broadcasts(
        &self,
        r: Request<pb::AdminListBroadcastsRequest>,
    ) -> Result<Response<pb::AdminListBroadcastsResponse>, Status> {
        self.0.lock().push((ut(&r), "admin_list".into()));
        Ok(Response::new(pb::AdminListBroadcastsResponse {
            broadcasts: vec![pb::Broadcast {
                id: "b1".into(),
                status: pb::BroadcastStatus::Published as i32,
                audience: pb::BroadcastAudience::All as i32,
                ..Default::default()
            }],
            total: 1,
        }))
    }
    async fn publish(
        &self,
        _: Request<pb::PublishRequest>,
    ) -> Result<Response<pb::PublishResponse>, Status> {
        Ok(Response::new(Default::default()))
    }
}

async fn app() -> (axum::Router, Seen) {
    let seen: Seen = Arc::new(Mutex::new(vec![]));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().port();
    let svc = Fake(seen.clone());
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(NotificationServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });
    let dead = "http://127.0.0.1:1".to_string();
    let state = Arc::new(gateway::AppState {
        client: reqwest::Client::new(),
        monolith_url: dead.clone(),
        identity_url: dead.clone(),
        media_url: dead.clone(),
        identity_grpc_url: dead.clone(),
        media_grpc_url: dead.clone(),
        budget_grpc_url: dead.clone(),
        portfolio_grpc_url: dead.clone(),
        category_grpc_url: dead.clone(),
        insight_grpc_url: dead.clone(),
        entry_grpc_url: dead.clone(),
        sharing_grpc_url: dead,
        notification_grpc_url: format!("http://127.0.0.1:{addr}"),
        identity_transport: gateway::IdentityTransport::GrpcTranscode,
        jwt_secret: SECRET.to_string(),
    });
    // Same shape as main.rs: user + admin routers behind the super-admin guard.
    let api = axum::Router::new()
        .nest("/notification", gateway::notification::router())
        .nest("/admin/notification", gateway::notification::admin_router());
    // The guard must wrap the outer router: inside `.nest("/api")` the prefix is stripped.
    (
        axum::Router::new()
            .nest("/api", api)
            .layer(axum::middleware::from_fn_with_state(
                state.clone(),
                gateway::middleware::reject_super_admin_on_user_paths,
            ))
            .with_state(state),
        seen,
    )
}

async fn get(app: &axum::Router, uri: &str, token: Option<&str>) -> (u16, serde_json::Value) {
    use axum::body::Body;
    use axum::http::{header, Request as H};
    use tower::ServiceExt;
    let mut b = H::builder().method("GET").uri(uri);
    if let Some(t) = token {
        b = b.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    let resp = app
        .clone()
        .oneshot(b.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = resp.status().as_u16();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, serde_json::from_slice(&bytes).unwrap_or_default())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inbox_maps_enums_to_labels_and_forwards_identity() {
    let (app, seen) = app().await;
    let (s, body) = get(
        &app,
        "/api/notification/inbox?filter=unread",
        Some(&jwt("normal")),
    )
    .await;
    assert_eq!(s, 200);
    let item = &body["items"][0];
    assert_eq!(item["category"], "reminder");
    assert_eq!(item["severity"], "warning");
    assert_eq!(item["kind"], "personal");
    assert_eq!(item["params"]["amount"], 5);
    assert_eq!(item["is_read"], false);
    assert_eq!(seen.lock()[0].0, "normal");

    let (s, body) = get(&app, "/api/notification/unread-count", Some(&jwt("normal"))).await;
    assert_eq!((s, body["total"].as_i64()), (200, Some(3)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_without_a_valid_token_are_rejected_before_reaching_the_service() {
    let (app, seen) = app().await;
    let (s, _) = get(&app, "/api/notification/inbox", None).await;
    assert_eq!(s, 401);
    let (s, _) = get(&app, "/api/notification/inbox", Some("garbage")).await;
    assert_eq!(s, 401);
    assert!(seen.lock().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_filter_is_a_400_not_a_default() {
    let (app, _) = app().await;
    let (s, _) = get(
        &app,
        "/api/notification/inbox?filter=bogus",
        Some(&jwt("normal")),
    )
    .await;
    assert_eq!(s, 400);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn super_admin_can_use_the_admin_path() {
    let (app, seen) = app().await;
    let admin = jwt("super_admin");
    let (s, body) = get(&app, "/api/admin/notification/broadcasts", Some(&admin)).await;
    assert_eq!(s, 200);
    assert_eq!(body["broadcasts"][0]["status"], "published");
    assert_eq!(body["broadcasts"][0]["audience"], "all");
    assert_eq!(
        seen.lock()[0],
        ("super_admin".to_string(), "admin_list".to_string())
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn super_admin_is_rejected_on_user_inbox() {
    let (app, seen) = app().await;
    let (s, body) = get(&app, "/api/notification/inbox", Some(&jwt("super_admin"))).await;
    assert_eq!(s, 403);
    assert_eq!(body["code"], "FORBIDDEN");
    assert!(seen.lock().is_empty(), "request must not reach the service");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn publish_is_not_routable() {
    let (app, _) = app().await;
    let (s, _) = get(&app, "/api/notification/publish", Some(&jwt("normal"))).await;
    assert!(s == 404 || s == 405);
}
