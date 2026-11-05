use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
    Json,
};
use std::sync::Arc;

use crate::AppState;

/// Paths that super admin is explicitly allowed to call. Everything else
/// under `/api/` is rejected so a super-admin token can't accidentally
/// reach a per-org endpoint that expects an `org_id` claim.
///
/// Sources of allowed paths:
///   - `/api/identity/*` — identity-service admin endpoints
///   - `/api/admin/*` — reserved for a future admin router
///   - `/api/budget/budgets/admin` (and other budget/admin paths) — super-admin
///     platform-wide budget view (lives under the budget service because
///     that's where the ListBudgetsAdmin RPC lives).
///   - Per-org paths shaped `/api/{budget,category,entry,sharing}/budgets/{uuid}/...`
///     — super admin is the break-glass recovery role. The gateway's
///     `with_user` propagates `x-user-type` so downstream services grant Owner.
fn is_super_admin_allowed_path(path: &str) -> bool {
    if path.starts_with("/api/identity/") || path.starts_with("/api/admin/") {
        return true;
    }
    // Budget-service admin paths — explicit list so we never accidentally
    // expose a per-org endpoint to super_admin.
    if matches!(
        path,
        "/api/budget/budgets/admin" | "/api/budget/members/admin" | "/api/budget/templates/admin"
    ) {
        return true;
    }
    // Per-budget routes across budget / category / entry / sharing: super
    // admin can read/edit any budget to recover from cases where the owner
    // is unavailable (locked out, deleted, etc.). The path segment after
    // `/budgets/` is the budget UUID — we distinguish from the literal
    // `admin` segment by requiring a 36-char (UUID-shaped) value.
    // Sub-routes (`/members`, `/envelope`, `/rollover`, `/invest/...`,
    // `/entries/...`, `/categories/...`) inherit the allowance automatically
    // because we only check the prefix.
    for prefix in [
        "/api/budget/budgets/",
        "/api/category/budgets/",
        "/api/entry/budgets/",
        "/api/sharing/budgets/",
    ] {
        if let Some(rest) = path.strip_prefix(prefix) {
            let segment = rest.split('/').next().unwrap_or("");
            if segment.len() == 36 && segment.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
                return true;
            }
        }
    }
    false
}

/// Rejects Super Admin JWTs on non-admin paths.
/// Super Admins must use `/api/identity/*` or the explicit admin paths above.
///
/// SECURITY: the `user_type` claim is read from a VERIFIED token. Reading it
/// from an unverified payload would let any caller forge `super_admin` and
/// reach the break-glass per-budget routes allowed by
/// `is_super_admin_allowed_path`.
pub async fn reject_super_admin_on_user_paths(
    State(state): State<Arc<AppState>>,
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let path = request.uri().path().to_string();

    let is_user_path = path.starts_with("/api/") && !is_super_admin_allowed_path(&path);

    if is_user_path {
        if let Some(auth) = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
        {
            if let Some(claims) = crate::auth::verify_bearer(auth, &state.jwt_secret) {
                if claims.is_super_admin() {
                    return Err((
                        StatusCode::FORBIDDEN,
                        Json(serde_json::json!({
                            "code": "FORBIDDEN",
                            "message": "Super Admin must use admin endpoints"
                        })),
                    ));
                }
            }
        }
    }

    Ok(next.run(request).await)
}
