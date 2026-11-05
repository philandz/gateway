//! Bearer-token verification at the gateway boundary.
//!
//! The gateway is the single external entrypoint, so it is the only place that
//! can decide whether a request really comes from the user it claims to come
//! from. Every downstream service authenticates a caller from the `x-user-id`
//! gRPC metadata that the gateway injects — those services have no JWT
//! verification code of their own. Therefore the gateway MUST verify the
//! signature (and expiry) of every bearer token before it forwards the
//! `sub` claim.
//!
//! The verification mirrors the identity service's `verify_jwt`
//! (`identity/src/manager/biz/token.rs`): HS256, shared `JWT_SECRET`,
//! `Validation::default()` (which requires and checks `exp`).

use axum::http::HeaderMap;
use jsonwebtoken::{decode, DecodingKey, Validation};
use serde::Deserialize;
use tonic::metadata::MetadataValue;
use tonic::Request;

/// Claims carried by the Philand access token.
///
/// Mirrors `identity::manager::biz::token::Claims`. `email`, `org_id` and
/// `user_type` default to an empty string so tokens issued before a claim was
/// introduced still verify.
#[derive(Debug, Clone, Deserialize, serde::Serialize)]
pub struct Claims {
    /// User id — the value the gateway forwards as `x-user-id`.
    pub sub: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub org_id: String,
    /// `"normal"` or `"super_admin"`.
    #[serde(default)]
    pub user_type: String,
    /// Expiry (unix seconds). Checked by `jsonwebtoken`.
    pub exp: usize,
}

impl Claims {
    /// The user id, or an empty string when the token carries none.
    pub fn user_id(&self) -> &str {
        &self.sub
    }

    /// The user type, or an empty string when the token carries none.
    pub fn kind(&self) -> &str {
        &self.user_type
    }

    /// True when the token belongs to a platform super admin.
    pub fn is_super_admin(&self) -> bool {
        self.user_type == "super_admin"
    }
}

/// Verify a `Bearer <token>` authorization header against the shared secret.
///
/// Returns `None` when the header is not a bearer token, when the signature
/// does not match, when the token is expired, or when the secret is empty.
/// A caller that gets `None` MUST NOT trust any claim from the token.
pub fn verify_bearer(bearer: &str, secret: &str) -> Option<Claims> {
    if secret.is_empty() {
        return None;
    }
    let token = bearer.strip_prefix("Bearer ")?.trim();
    if token.is_empty() {
        return None;
    }
    decode::<Claims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &Validation::default(),
    )
    .ok()
    .map(|data| data.claims)
}

/// Why a request could not be authenticated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthError {
    /// No `Authorization` header at all.
    Missing,
    /// The header is present but the token is not a valid, unexpired JWT.
    Invalid,
}

impl AuthError {
    /// Human-readable message for the HTTP/gRPC error body.
    pub fn message(self) -> &'static str {
        match self {
            AuthError::Missing => "Missing Authorization header",
            AuthError::Invalid => "Invalid or expired token",
        }
    }
}

/// Verify the bearer token in `headers` and build the outbound gRPC request
/// carrying the caller's identity.
///
/// The `authorization` header is forwarded unchanged so downstream services
/// can still re-check it, but `x-user-id` / `x-user-type` are only set from a
/// token whose signature and expiry have been verified.
pub fn request_with_user<T>(
    secret: &str,
    headers: &HeaderMap,
    req: T,
) -> Result<Request<T>, AuthError> {
    let auth = headers
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .ok_or(AuthError::Missing)?;
    let claims = verify_bearer(auth, secret).ok_or(AuthError::Invalid)?;

    let mut grpc_req = Request::new(req);
    let value = MetadataValue::try_from(auth).map_err(|_| AuthError::Invalid)?;
    grpc_req.metadata_mut().insert("authorization", value);
    if let Ok(v) = MetadataValue::try_from(claims.user_id()) {
        grpc_req.metadata_mut().insert("x-user-id", v);
    }
    if !claims.kind().is_empty() {
        if let Ok(v) = MetadataValue::try_from(claims.kind()) {
            grpc_req.metadata_mut().insert("x-user-type", v);
        }
    }
    Ok(grpc_req)
}

#[cfg(test)]
mod tests {
    use super::*;
    use jsonwebtoken::{encode, EncodingKey, Header};

    const SECRET: &str = "test-secret";

    fn issue(sub: &str, user_type: &str, exp: usize) -> String {
        let claims = Claims {
            sub: sub.to_string(),
            email: "a@b.c".to_string(),
            org_id: "org-1".to_string(),
            user_type: user_type.to_string(),
            exp,
        };
        encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(SECRET.as_bytes()),
        )
        .expect("encode")
    }

    #[test]
    fn accepts_a_valid_token() {
        let token = issue("user-1", "normal", 4_000_000_000);
        let claims = verify_bearer(&format!("Bearer {token}"), SECRET).expect("must verify");
        assert_eq!(claims.user_id(), "user-1");
        assert_eq!(claims.kind(), "normal");
        assert!(!claims.is_super_admin());
    }

    #[test]
    fn rejects_a_token_signed_with_another_secret() {
        let token = issue("user-1", "normal", 4_000_000_000);
        assert!(verify_bearer(&format!("Bearer {token}"), "other-secret").is_none());
    }

    #[test]
    fn rejects_an_expired_token() {
        // exp in the past — `Validation::default()` checks `exp`.
        let token = issue("user-1", "normal", 1);
        assert!(verify_bearer(&format!("Bearer {token}"), SECRET).is_none());
    }

    #[test]
    fn rejects_a_forged_unsigned_payload() {
        // base64url of {"sub":"user-1","user_type":"super_admin","exp":4000000000}
        let payload =
            "eyJzdWIiOiJ1c2VyLTEiLCJ1c2VyX3R5cGUiOiJzdXBlcl9hZG1pbiIsImV4cCI6NDAwMDAwMDAwMH0";
        let token = format!("{payload}.sig");
        assert!(verify_bearer(&format!("Bearer {token}"), SECRET).is_none());
    }

    #[test]
    fn rejects_a_non_bearer_header() {
        assert!(verify_bearer("SharingSession abc", SECRET).is_none());
        assert!(verify_bearer("Basic abc", SECRET).is_none());
    }

    #[test]
    fn rejects_an_empty_secret() {
        let token = issue("user-1", "normal", 4_000_000_000);
        assert!(verify_bearer(&format!("Bearer {token}"), "").is_none());
    }
}
