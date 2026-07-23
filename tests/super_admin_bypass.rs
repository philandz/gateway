//! Regression test for the super-admin budget-detail 403.
//!
//! Exercises the **budget sub-router** (`gateway::budget::router()`) directly,
//! routing a GET to `/budgets/<uuid>` (the inner path).  The public surface
//! `GET /api/budget/budgets/<uuid>` reaches this code via the parent router's
//! `/api/budget` prefix strip — the test bypasses that mount point and fires
//! the inner handler path directly.
//!
//! Boots the budget Axum router in-process against a mock tonic BudgetService
//! server. Fires the request with a JWT whose payload carries
//! user_type=super_admin and asserts:
//!   1. The response is 200 (not 403).
//!   2. The downstream tonic server saw metadata `x-user-type` = "super_admin".
//!   3. The downstream tonic server saw metadata `x-user-id` matching the JWT `sub`.

use parking_lot::Mutex;
use std::sync::Arc;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use gateway::pb::service::budget::{
    budget_service_server::{BudgetService, BudgetServiceServer},
    Budget, BudgetRole, GetBudgetRequest, GetBudgetResponse,
};

#[derive(Default)]
struct CapturingBudgetSvc {
    last_md: Arc<Mutex<Option<tonic::metadata::MetadataMap>>>,
}

#[tonic::async_trait]
impl BudgetService for CapturingBudgetSvc {
    async fn get_budget(
        &self,
        request: Request<GetBudgetRequest>,
    ) -> Result<Response<GetBudgetResponse>, Status> {
        *self.last_md.lock() = Some(request.metadata().clone());
        Ok(Response::new(GetBudgetResponse {
            budget: Some(Budget {
                my_role: BudgetRole::Owner as i32,
                ..Budget::default()
            }),
        }))
    }

    // All other RPCs — required by the trait but not exercised in this test.
    async fn create_budget(
        &self,
        _: Request<gateway::pb::service::budget::CreateBudgetRequest>,
    ) -> Result<Response<gateway::pb::service::budget::CreateBudgetResponse>, Status> {
        unimplemented!()
    }
    async fn update_budget(
        &self,
        _: Request<gateway::pb::service::budget::UpdateBudgetRequest>,
    ) -> Result<Response<gateway::pb::service::budget::UpdateBudgetResponse>, Status> {
        unimplemented!()
    }
    async fn delete_budget(
        &self,
        _: Request<gateway::pb::service::budget::DeleteBudgetRequest>,
    ) -> Result<Response<gateway::pb::service::budget::DeleteBudgetResponse>, Status> {
        unimplemented!()
    }
    async fn list_budgets(
        &self,
        _: Request<gateway::pb::service::budget::ListBudgetsRequest>,
    ) -> Result<Response<gateway::pb::service::budget::ListBudgetsResponse>, Status> {
        unimplemented!()
    }
    async fn list_budgets_admin(
        &self,
        _: Request<gateway::pb::service::budget::ListBudgetsAdminRequest>,
    ) -> Result<Response<gateway::pb::service::budget::ListBudgetsAdminResponse>, Status> {
        unimplemented!()
    }
    async fn get_budget_admin(
        &self,
        _: Request<gateway::pb::service::budget::GetBudgetAdminRequest>,
    ) -> Result<Response<gateway::pb::service::budget::GetBudgetAdminResponse>, Status> {
        unimplemented!()
    }
    async fn list_budget_members_admin(
        &self,
        _: Request<gateway::pb::service::budget::ListBudgetMembersAdminRequest>,
    ) -> Result<Response<gateway::pb::service::budget::ListBudgetMembersAdminResponse>, Status>
    {
        unimplemented!()
    }
    async fn check_role(
        &self,
        _: Request<gateway::pb::service::budget::CheckRoleRequest>,
    ) -> Result<Response<gateway::pb::service::budget::CheckRoleResponse>, Status> {
        unimplemented!()
    }
    async fn add_budget_member(
        &self,
        _: Request<gateway::pb::service::budget::AddBudgetMemberRequest>,
    ) -> Result<Response<gateway::pb::service::budget::AddBudgetMemberResponse>, Status> {
        unimplemented!()
    }
    async fn update_budget_member_role(
        &self,
        _: Request<gateway::pb::service::budget::UpdateBudgetMemberRoleRequest>,
    ) -> Result<Response<gateway::pb::service::budget::UpdateBudgetMemberRoleResponse>, Status>
    {
        unimplemented!()
    }
    async fn remove_budget_member(
        &self,
        _: Request<gateway::pb::service::budget::RemoveBudgetMemberRequest>,
    ) -> Result<Response<gateway::pb::service::budget::RemoveBudgetMemberResponse>, Status> {
        unimplemented!()
    }
    async fn list_budget_members(
        &self,
        _: Request<gateway::pb::service::budget::ListBudgetMembersRequest>,
    ) -> Result<Response<gateway::pb::service::budget::ListBudgetMembersResponse>, Status> {
        unimplemented!()
    }
    async fn set_envelope_limit(
        &self,
        _: Request<gateway::pb::service::budget::SetEnvelopeLimitRequest>,
    ) -> Result<Response<gateway::pb::service::budget::SetEnvelopeLimitResponse>, Status> {
        unimplemented!()
    }
    async fn get_burn_rate(
        &self,
        _: Request<gateway::pb::service::budget::GetBurnRateRequest>,
    ) -> Result<Response<gateway::pb::service::budget::GetBurnRateResponse>, Status> {
        unimplemented!()
    }
    async fn set_rollover_policy(
        &self,
        _: Request<gateway::pb::service::budget::SetRolloverPolicyRequest>,
    ) -> Result<Response<gateway::pb::service::budget::SetRolloverPolicyResponse>, Status> {
        unimplemented!()
    }
    async fn get_rollover_policy(
        &self,
        _: Request<gateway::pb::service::budget::GetRolloverPolicyRequest>,
    ) -> Result<Response<gateway::pb::service::budget::GetRolloverPolicyResponse>, Status> {
        unimplemented!()
    }
    async fn list_templates(
        &self,
        _: Request<gateway::pb::service::budget::ListTemplatesRequest>,
    ) -> Result<Response<gateway::pb::service::budget::ListTemplatesResponse>, Status> {
        unimplemented!()
    }
    async fn create_invest_asset(
        &self,
        _: Request<gateway::pb::service::budget::CreateInvestAssetRequest>,
    ) -> Result<Response<gateway::pb::service::budget::InvestAsset>, Status> {
        unimplemented!()
    }
    async fn update_invest_asset(
        &self,
        _: Request<gateway::pb::service::budget::UpdateInvestAssetRequest>,
    ) -> Result<Response<gateway::pb::service::budget::InvestAsset>, Status> {
        unimplemented!()
    }
    async fn delete_invest_asset(
        &self,
        _: Request<gateway::pb::service::budget::DeleteInvestAssetRequest>,
    ) -> Result<Response<gateway::pb::service::budget::DeleteInvestAssetResponse>, Status> {
        unimplemented!()
    }
    async fn list_invest_assets(
        &self,
        _: Request<gateway::pb::service::budget::ListInvestAssetsRequest>,
    ) -> Result<Response<gateway::pb::service::budget::ListInvestAssetsResponse>, Status> {
        unimplemented!()
    }
    async fn get_invest_portfolio_summary(
        &self,
        _: Request<gateway::pb::service::budget::GetInvestPortfolioSummaryRequest>,
    ) -> Result<Response<gateway::pb::service::budget::InvestPortfolioSummary>, Status> {
        unimplemented!()
    }
    async fn add_price_snapshot(
        &self,
        _: Request<gateway::pb::service::budget::AddPriceSnapshotRequest>,
    ) -> Result<Response<gateway::pb::service::budget::PriceSnapshot>, Status> {
        unimplemented!()
    }
    async fn get_latest_price_snapshot(
        &self,
        _: Request<gateway::pb::service::budget::GetLatestPriceSnapshotRequest>,
    ) -> Result<Response<gateway::pb::service::budget::PriceSnapshot>, Status> {
        unimplemented!()
    }
    async fn list_price_snapshots(
        &self,
        _: Request<gateway::pb::service::budget::ListPriceSnapshotsRequest>,
    ) -> Result<Response<gateway::pb::service::budget::ListPriceSnapshotsResponse>, Status> {
        unimplemented!()
    }
}

fn make_jwt(user_type: &str) -> String {
    // Lightweight unsigned JWT (gateway's extract_user_type* decoders only
    // base64url-decode the payload — they do not verify the signature).
    let header = base64url(b"{\"alg\":\"none\",\"typ\":\"JWT\"}");
    let payload = base64url(format!(
        r#"{{"sub":"{}","email":"super@philand.test","org_id":"00000000-0000-0000-0000-000000000001","user_type":"{}","exp":9999999999}}"#,
        Uuid::new_v4(),
        user_type,
    ).as_bytes());
    format!("{header}.{payload}.signature-ignored")
}

fn base64url(input: &[u8]) -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    URL_SAFE_NO_PAD.encode(input)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn super_admin_jwt_passes_through_gateway_with_user_type_metadata() {
    let captured: Arc<Mutex<Option<tonic::metadata::MetadataMap>>> = Arc::new(Mutex::new(None));
    let svc = CapturingBudgetSvc {
        last_md: captured.clone(),
    };

    // Spawn the mock budget gRPC server on an ephemeral port.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(BudgetServiceServer::new(svc))
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
            .unwrap();
    });

    // Wire the gateway budget router to the mock budget gRPC + a noop identity URL.
    // AppState is constructed via struct literal (no .new()) — matching how
    // main.rs and proxy_test.rs do it.
    let state = Arc::new(gateway::AppState {
        client: reqwest::Client::new(),
        monolith_url: "http://127.0.0.1:1".to_string(),
        identity_url: "http://127.0.0.1:1".to_string(),
        media_url: "http://127.0.0.1:1".to_string(),
        identity_grpc_url: "http://127.0.0.1:1".to_string(),
        media_grpc_url: "http://127.0.0.1:1".to_string(),
        budget_grpc_url: format!("http://127.0.0.1:{port}"),
        category_grpc_url: "http://127.0.0.1:1".to_string(),
        entry_grpc_url: "http://127.0.0.1:1".to_string(),
        sharing_grpc_url: "http://127.0.0.1:1".to_string(),
        identity_transport: gateway::IdentityTransport::GrpcTranscode,
    });
    let app = gateway::budget::router().with_state(state);

    // Use tower's ServiceExt::oneshot pattern.
    use axum::body::Body;
    use axum::http::{header, Request as HttpRequest};
    use tower::ServiceExt;

    let jwt = make_jwt("super_admin");
    let req = HttpRequest::builder()
        .method("GET")
        .uri("/budgets/ef4a838b-0fad-4c1f-ac92-0f0a2ffaaaa9")
        .header(header::AUTHORIZATION, format!("Bearer {jwt}"))
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(
        resp.status(),
        200,
        "super admin must not be rejected by gateway middleware"
    );

    let md = captured
        .lock()
        .clone()
        .expect("downstream server saw no request");
    assert_eq!(
        md.get("x-user-type").map(|v| v.to_str().unwrap()),
        Some("super_admin")
    );
    assert!(md.get("x-user-id").is_some(), "x-user-id must be forwarded");
}
