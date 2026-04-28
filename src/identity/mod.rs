use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    routing::{delete, get, patch, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use tonic::{metadata::MetadataValue, transport::Channel, Request as GrpcRequest, Status};

use crate::pb::service::identity as pb;
use crate::pb::service::identity::identity_service_client::IdentityServiceClient;
use crate::pb::shared::organization::MemberStatus;
use crate::pb::shared::organization::OrgRole;
use crate::AppState;
use philand_error::ErrorEnvelope as ErrorResponse;

type ApiResult<T> = Result<T, (StatusCode, Json<ErrorResponse>)>;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/register", post(register))
        .route("/login", post(login))
        .route("/auth/google", post(login_with_google))
        .route("/logout", post(logout))
        .route("/refresh", post(refresh))
        .route("/update", post(change_password))
        .route("/forgot", post(forgot_password))
        .route("/reset", post(reset_password))
        .route("/profile", get(profile))
        .route("/profile", patch(update_profile))
        .route(
            "/organizations",
            get(list_organizations).post(admin_create_organization),
        )
        .route("/organizations/{org_id}/members", get(list_org_members))
        .route("/organizations/{org_id}/invitations", post(invite_member))
        .route("/invitations/{token}/accept", post(accept_invitation))
        .route(
            "/organizations/{org_id}/members/{user_id}/role",
            patch(change_org_member_role),
        )
        .route(
            "/organizations/{org_id}/members/{user_id}",
            delete(remove_org_member),
        )
        // Admin — user CRUD (super_admin only, 403 otherwise)
        .route("/users", get(admin_list_users).post(admin_create_user))
        .route(
            "/users/{user_id}",
            get(admin_get_user)
                .patch(admin_update_user)
                .delete(admin_delete_user),
        )
        // Admin — org CRUD (super_admin only, 403 otherwise)
        .route("/organizations/all", get(admin_list_organizations))
        .route(
            "/organizations/{org_id}/detail",
            get(admin_get_organization),
        )
        .route(
            "/organizations/{org_id}",
            patch(admin_update_organization).delete(admin_delete_organization),
        )
}

async fn health(State(state): State<Arc<AppState>>) -> ApiResult<&'static str> {
    let url = format!("{}/health", state.identity_url);
    let resp = state
        .client
        .get(url)
        .send()
        .await
        .map_err(|e| map_status(Status::unavailable(e.to_string())))?;

    if resp.status().is_success() {
        Ok("OK")
    } else {
        Err(map_status(Status::unavailable(
            "Identity health endpoint returned non-success",
        )))
    }
}

fn map_status(status: Status) -> (StatusCode, Json<ErrorResponse>) {
    let (http, envelope) = philand_error::http_error_from_tonic_status(&status);
    (http, Json(envelope))
}

async fn client(state: &AppState) -> ApiResult<IdentityServiceClient<Channel>> {
    IdentityServiceClient::connect(state.identity_grpc_url.clone())
        .await
        .map_err(|e| map_status(Status::internal(e.to_string())))
}

fn with_auth<T>(headers: &HeaderMap, req: T) -> ApiResult<GrpcRequest<T>> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or_else(|| map_status(Status::unauthenticated("Missing Authorization header")))?;

    let mut grpc_req = GrpcRequest::new(req);
    let value = MetadataValue::try_from(auth)
        .map_err(|_| map_status(Status::unauthenticated("Invalid Authorization header")))?;
    grpc_req.metadata_mut().insert("authorization", value);
    Ok(grpc_req)
}

#[derive(Deserialize)]
struct RegisterRequest {
    email: String,
    password: String,
    display_name: String,
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

#[derive(Deserialize)]
struct ChangePasswordRequest {
    current_password: String,
    new_password: String,
}

#[derive(Deserialize)]
struct ForgotPasswordRequest {
    email: String,
}

#[derive(Deserialize)]
struct ResetPasswordRequest {
    token: String,
    new_password: String,
}

#[derive(Deserialize)]
struct UpdateProfileRequest {
    display_name: Option<String>,
    avatar: Option<String>,
    bio: Option<String>,
    timezone: Option<String>,
    locale: Option<String>,
}

#[derive(Deserialize)]
struct InviteMemberRequest {
    invitee_email: String,
    org_role: String,
}

#[derive(Deserialize)]
struct ChangeOrgMemberRoleRequest {
    org_role: String,
}

#[derive(Deserialize)]
struct AdminUpdateUserRequest {
    display_name: Option<String>,
    user_type: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize)]
struct AdminUpdateOrganizationRequest {
    name: Option<String>,
    status: Option<String>,
}

#[derive(Deserialize, Default)]
struct AdminListQueryParams {
    q: Option<String>,
    status: Option<String>,
    user_type: Option<String>,
    sort_by: Option<String>,
    sort_dir: Option<String>,
    page: Option<i32>,
    page_size: Option<i32>,
}

impl AdminListQueryParams {
    fn to_proto(&self) -> pb::ListParams {
        pb::ListParams {
            query: self.q.clone(),
            status: self.status.clone(),
            sort_by: self.sort_by.clone(),
            sort_dir: self.sort_dir.clone(),
            page: self.page,
            page_size: self.page_size,
        }
    }
}

#[derive(Deserialize)]
struct AdminCreateUserRequest {
    email: String,
    password: String,
    display_name: String,
    user_type: Option<String>,
}

#[derive(Deserialize)]
struct AdminCreateOrganizationRequest {
    name: String,
    owner_user_id: String,
}

#[derive(Serialize)]
struct OrgMemberResponse {
    user_id: String,
    email: String,
    display_name: String,
    role: String,
    status: String,
    joined_at: i64,
}

fn role_to_string(role: i32) -> String {
    match OrgRole::try_from(role).unwrap_or(OrgRole::OrNone) {
        OrgRole::OrOwner => "owner",
        OrgRole::OrAdmin => "admin",
        OrgRole::OrMember => "member",
        OrgRole::OrNone => "none",
    }
    .to_string()
}

fn member_status_to_string(status: i32) -> String {
    match MemberStatus::try_from(status).unwrap_or(MemberStatus::MsNone) {
        MemberStatus::MsActive => "active",
        MemberStatus::MsInvited => "invited",
        MemberStatus::MsNone => "none",
    }
    .to_string()
}

fn map_base(base: Option<&crate::pb::common::base::Base>) -> serde_json::Value {
    base.map_or(serde_json::Value::Null, |b| {
        serde_json::json!({
            "id": b.id,
            "created_at": b.created_at,
            "updated_at": b.updated_at,
            "deleted_at": b.deleted_at,
            "created_by": b.created_by,
            "updated_by": b.updated_by,
            "owner_id": b.owner_id,
            "status": b.status,
        })
    })
}

fn map_user(user: Option<&crate::pb::shared::user::User>) -> serde_json::Value {
    user.map_or(serde_json::Value::Null, |u| {
        serde_json::json!({
            "base": map_base(u.base.as_ref()),
            "email": u.email,
            "display_name": u.display_name,
            "avatar": u.avatar,
            "bio": u.bio,
            "timezone": u.timezone,
            "locale": u.locale,
            "user_type": u.user_type,
        })
    })
}

fn map_org_summary(org: &pb::OrganizationSummary) -> serde_json::Value {
    serde_json::json!({
        "id": org.id,
        "name": org.name,
        "role": org.role,
    })
}

fn map_organization(org: &crate::pb::shared::organization::Organization) -> serde_json::Value {
    let base = org.base.as_ref();
    serde_json::json!({
        "id": base.map(|b| b.id.as_str()).unwrap_or(""),
        "name": org.name,
        "status": base.map(|b| b.status).unwrap_or(0),
        "created_at": base.map(|b| b.created_at).unwrap_or(0),
        "updated_at": base.map(|b| b.updated_at).unwrap_or(0),
    })
}

fn map_invitation(invitation: Option<&pb::OrganizationInvitation>) -> serde_json::Value {
    invitation.map_or(serde_json::Value::Null, |i| {
        serde_json::json!({
            "id": i.id,
            "org_id": i.org_id,
            "inviter_id": i.inviter_id,
            "invitee_email": i.invitee_email,
            "org_role": i.org_role,
            "status": i.status,
            "expires_at": i.expires_at,
            "created_at": i.created_at,
        })
    })
}

fn parse_role(role: &str) -> ApiResult<i32> {
    let parsed = match role.trim().to_lowercase().as_str() {
        "owner" => OrgRole::OrOwner,
        "admin" => OrgRole::OrAdmin,
        "member" => OrgRole::OrMember,
        _ => {
            return Err(map_status(Status::invalid_argument(
                "org_role must be one of: owner, admin, member",
            )));
        }
    };
    Ok(parsed as i32)
}

fn parse_user_type(user_type: &str) -> ApiResult<i32> {
    let parsed = match user_type.trim().to_lowercase().as_str() {
        "normal" => crate::pb::shared::user::UserType::UtNormal,
        "super_admin" => crate::pb::shared::user::UserType::UtSuperAdmin,
        _ => {
            return Err(map_status(Status::invalid_argument(
                "user_type must be one of: normal, super_admin",
            )));
        }
    };
    Ok(parsed as i32)
}

fn parse_base_status(status: &str) -> ApiResult<i32> {
    let parsed = match status.trim().to_lowercase().as_str() {
        "active" => crate::pb::common::base::BaseStatus::BsActive,
        "disabled" => crate::pb::common::base::BaseStatus::BsDisabled,
        _ => {
            return Err(map_status(Status::invalid_argument(
                "status must be one of: active, disabled",
            )));
        }
    };
    Ok(parsed as i32)
}

async fn register(
    State(state): State<Arc<AppState>>,
    Json(body): Json<RegisterRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .register(GrpcRequest::new(pb::RegisterRequest {
            email: body.email,
            password: body.password,
            display_name: body.display_name,
        }))
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "user": map_user(resp.user.as_ref()) })),
    ))
}

async fn login(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .login(GrpcRequest::new(pb::LoginRequest {
            email: body.email,
            password: body.password,
        }))
        .await
        .map_err(map_status)?
        .into_inner();
    let organizations: Vec<serde_json::Value> = resp
        .organizations
        .into_iter()
        .map(|o| map_org_summary(&o))
        .collect();
    Ok(Json(serde_json::json!({
        "access_token": resp.access_token,
        "user_type": resp.user_type,
        "organizations": organizations,
    })))
}

#[derive(Deserialize)]
struct LoginWithGoogleRequest {
    id_token: String,
}

async fn login_with_google(
    State(state): State<Arc<AppState>>,
    Json(body): Json<LoginWithGoogleRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .login_with_google(GrpcRequest::new(pb::LoginWithGoogleRequest {
            id_token: body.id_token,
        }))
        .await
        .map_err(map_status)?
        .into_inner();
    let organizations: Vec<serde_json::Value> = resp
        .organizations
        .into_iter()
        .map(|o| map_org_summary(&o))
        .collect();
    Ok(Json(serde_json::json!({
        "access_token": resp.access_token,
        "user_type": resp.user_type,
        "organizations": organizations,
    })))
}

async fn logout(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.logout(with_auth(&headers, pb::LogoutRequest {})?)
        .await
        .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message":"Logged out successfully"}),
    ))
}

async fn refresh(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .refresh_token(with_auth(&headers, pb::RefreshTokenRequest {})?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({"access_token": resp.access_token})))
}

async fn change_password(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<ChangePasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.change_password(with_auth(
        &headers,
        pb::ChangePasswordRequest {
            current_password: body.current_password,
            new_password: body.new_password,
        },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message":"Password changed successfully"}),
    ))
}

async fn forgot_password(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ForgotPasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .forgot_password(GrpcRequest::new(pb::ForgotPasswordRequest {
            email: body.email,
        }))
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({"message": resp.message})))
}

async fn reset_password(
    State(state): State<Arc<AppState>>,
    Json(body): Json<ResetPasswordRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.reset_password(GrpcRequest::new(pb::ResetPasswordRequest {
        token: body.token,
        new_password: body.new_password,
    }))
    .await
    .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message":"Password has been reset successfully"}),
    ))
}

async fn profile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_profile(with_auth(&headers, pb::GetProfileRequest {})?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(
        serde_json::json!({ "user": map_user(resp.user.as_ref()) }),
    ))
}

async fn update_profile(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<UpdateProfileRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .update_profile(with_auth(
            &headers,
            pb::UpdateProfileRequest {
                display_name: body.display_name,
                avatar: body.avatar,
                bio: body.bio,
                timezone: body.timezone,
                locale: body.locale,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(
        serde_json::json!({ "user": map_user(resp.user.as_ref()) }),
    ))
}

async fn list_organizations(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_organizations(with_auth(&headers, pb::ListOrganizationsRequest {})?)
        .await
        .map_err(map_status)?
        .into_inner();
    let organizations: Vec<serde_json::Value> =
        resp.organizations.iter().map(map_org_summary).collect();
    Ok(Json(serde_json::json!({"organizations": organizations})))
}

async fn list_org_members(
    State(state): State<Arc<AppState>>,
    Path(org_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .list_org_members(with_auth(&headers, pb::ListOrgMembersRequest { org_id })?)
        .await
        .map_err(map_status)?
        .into_inner();

    let members: Vec<OrgMemberResponse> = resp
        .members
        .into_iter()
        .map(|m| OrgMemberResponse {
            user_id: m.user_id,
            email: m.email,
            display_name: m.display_name,
            role: role_to_string(m.role),
            status: member_status_to_string(m.status),
            joined_at: m.joined_at,
        })
        .collect();

    Ok(Json(serde_json::json!({"members": members})))
}

async fn invite_member(
    State(state): State<Arc<AppState>>,
    Path(org_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<InviteMemberRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .invite_member(with_auth(
            &headers,
            pb::InviteMemberRequest {
                org_id,
                invitee_email: body.invitee_email,
                org_role: parse_role(&body.org_role)?,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(serde_json::json!({
        "invitation": map_invitation(resp.invitation.as_ref()),
        "invite_token": resp.invite_token,
    })))
}

async fn accept_invitation(
    State(state): State<Arc<AppState>>,
    Path(token): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .accept_invitation(GrpcRequest::new(pb::AcceptInvitationRequest { token }))
        .await
        .map_err(map_status)?
        .into_inner();
    Ok(Json(
        serde_json::json!({"org_id": resp.org_id, "role": resp.role}),
    ))
}

async fn change_org_member_role(
    State(state): State<Arc<AppState>>,
    Path((org_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<ChangeOrgMemberRoleRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.change_org_member_role(with_auth(
        &headers,
        pb::ChangeOrgMemberRoleRequest {
            org_id,
            user_id,
            org_role: parse_role(&body.org_role)?,
        },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message":"Role updated successfully"}),
    ))
}

async fn remove_org_member(
    State(state): State<Arc<AppState>>,
    Path((org_id, user_id)): Path<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.remove_org_member(with_auth(
        &headers,
        pb::RemoveOrgMemberRequest { org_id, user_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message":"Member removed successfully"}),
    ))
}

async fn admin_list_users(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AdminListQueryParams>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let proto_params = params.to_proto();
    let resp = c
        .list_users(with_auth(
            &headers,
            pb::ListUsersRequest {
                params: Some(proto_params),
                user_type: params.user_type,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    let users: Vec<serde_json::Value> = resp.users.iter().map(|u| map_user(Some(u))).collect();
    let meta = resp.meta.as_ref().map_or(
        serde_json::json!({"page":1,"page_size":20,"total_pages":1,"total_rows":0}),
        |m| serde_json::json!({"page":m.page,"page_size":m.page_size,"total_pages":m.total_pages,"total_rows":m.total_rows}),
    );
    Ok(Json(serde_json::json!({"users": users, "meta": meta})))
}

async fn admin_create_user(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AdminCreateUserRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let user_type = body.user_type.as_deref().map(parse_user_type).transpose()?;
    let resp = c
        .create_user(with_auth(
            &headers,
            pb::CreateUserRequest {
                email: body.email,
                password: body.password,
                display_name: body.display_name,
                user_type,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({"user": map_user(resp.user.as_ref())})),
    ))
}

async fn admin_get_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_user(with_auth(&headers, pb::GetUserRequest { user_id })?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(
        serde_json::json!({"user": map_user(resp.user.as_ref())}),
    ))
}

async fn admin_update_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AdminUpdateUserRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;

    let user_type = body.user_type.as_deref().map(parse_user_type).transpose()?;
    let status = body.status.as_deref().map(parse_base_status).transpose()?;

    let resp = c
        .update_user(with_auth(
            &headers,
            pb::UpdateUserRequest {
                user_id,
                display_name: body.display_name,
                user_type,
                status,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(
        serde_json::json!({"user": map_user(resp.user.as_ref())}),
    ))
}

async fn admin_delete_user(
    State(state): State<Arc<AppState>>,
    Path(user_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_user(with_auth(&headers, pb::DeleteUserRequest { user_id })?)
        .await
        .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message": "User deleted successfully"}),
    ))
}

async fn admin_list_organizations(
    State(state): State<Arc<AppState>>,
    Query(params): Query<AdminListQueryParams>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let proto_params = params.to_proto();
    let resp = c
        .list_organizations_admin(with_auth(
            &headers,
            pb::ListOrganizationsAdminRequest {
                params: Some(proto_params),
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    let organizations: Vec<serde_json::Value> = resp
        .organizations
        .into_iter()
        .map(|o| map_organization(&o))
        .collect();
    let meta = resp.meta.as_ref().map_or(
        serde_json::json!({"page":1,"page_size":20,"total_pages":1,"total_rows":0}),
        |m| serde_json::json!({"page":m.page,"page_size":m.page_size,"total_pages":m.total_pages,"total_rows":m.total_rows}),
    );
    Ok(Json(
        serde_json::json!({"organizations": organizations, "meta": meta}),
    ))
}

async fn admin_create_organization(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Json(body): Json<AdminCreateOrganizationRequest>,
) -> ApiResult<(StatusCode, Json<serde_json::Value>)> {
    let mut c = client(&state).await?;
    let resp = c
        .create_organization_admin(with_auth(
            &headers,
            pb::CreateOrganizationAdminRequest {
                name: body.name,
                owner_user_id: body.owner_user_id,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();
    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "organization": resp.organization.as_ref().map_or(serde_json::Value::Null, map_organization)
        })),
    ))
}

async fn admin_get_organization(
    State(state): State<Arc<AppState>>,
    Path(org_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    let resp = c
        .get_organization_admin(with_auth(
            &headers,
            pb::GetOrganizationAdminRequest { org_id },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(serde_json::json!({
        "organization": resp.organization.as_ref().map_or(serde_json::Value::Null, map_organization)
    })))
}

async fn admin_update_organization(
    State(state): State<Arc<AppState>>,
    Path(org_id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<AdminUpdateOrganizationRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;

    let status = body.status.as_deref().map(parse_base_status).transpose()?;

    let resp = c
        .update_organization_admin(with_auth(
            &headers,
            pb::UpdateOrganizationAdminRequest {
                org_id,
                name: body.name,
                status,
            },
        )?)
        .await
        .map_err(map_status)?
        .into_inner();

    Ok(Json(serde_json::json!({
        "organization": resp.organization.as_ref().map_or(serde_json::Value::Null, map_organization)
    })))
}

async fn admin_delete_organization(
    State(state): State<Arc<AppState>>,
    Path(org_id): Path<String>,
    headers: HeaderMap,
) -> ApiResult<Json<serde_json::Value>> {
    let mut c = client(&state).await?;
    c.delete_organization_admin(with_auth(
        &headers,
        pb::DeleteOrganizationAdminRequest { org_id },
    )?)
    .await
    .map_err(map_status)?;
    Ok(Json(
        serde_json::json!({"message": "Organization deleted successfully"}),
    ))
}
