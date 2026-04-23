use axum::{extract::Request, http::StatusCode, middleware::Next, response::Response, Json};

/// Rejects Super Admin JWTs on non-admin paths.
/// Super Admins must use /api/identity/* (admin endpoints) only.
pub async fn reject_super_admin_on_user_paths(
    request: Request,
    next: Next,
) -> Result<Response, (StatusCode, Json<serde_json::Value>)> {
    let path = request.uri().path().to_string();

    // Only apply to /api/* paths that are NOT identity (admin endpoints live there)
    let is_user_path = path.starts_with("/api/")
        && !path.starts_with("/api/identity/")
        && !path.starts_with("/api/admin/");

    if is_user_path {
        if let Some(auth) = request
            .headers()
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
        {
            if let Some(user_type) = extract_user_type_from_jwt(auth) {
                if user_type == "super_admin" {
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

/// Extracts `user_type` claim from a JWT without full verification.
/// The gateway already validated the signature upstream; this is a thin claim read.
fn extract_user_type_from_jwt(token: &str) -> Option<String> {
    let payload_b64 = token.split('.').nth(1)?;
    let decoded = base64url_decode(payload_b64)?;
    let claims: serde_json::Value = serde_json::from_slice(&decoded).ok()?;
    claims
        .get("user_type")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Minimal base64url decode (no padding required, URL-safe alphabet).
fn base64url_decode(input: &str) -> Option<Vec<u8>> {
    // Convert base64url → base64 standard
    let mut s = input.replace('-', "+").replace('_', "/");
    // Add padding
    match s.len() % 4 {
        2 => s.push_str("=="),
        3 => s.push('='),
        _ => {}
    }
    base64_decode_standard(&s)
}

fn base64_decode_standard(input: &str) -> Option<Vec<u8>> {
    const TABLE: &[u8; 128] = b"\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\
        \xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\x3e\xff\xff\xff\x3f\
        \x34\x35\x36\x37\x38\x39\x3a\x3b\x3c\x3d\xff\xff\xff\xff\xff\xff\
        \xff\x00\x01\x02\x03\x04\x05\x06\x07\x08\x09\x0a\x0b\x0c\x0d\x0e\
        \x0f\x10\x11\x12\x13\x14\x15\x16\x17\x18\x19\xff\xff\xff\xff\xff\
        \xff\x1a\x1b\x1c\x1d\x1e\x1f\x20\x21\x22\x23\x24\x25\x26\x27\x28\
        \x29\x2a\x2b\x2c\x2d\x2e\x2f\x30\x31\x32\x33\xff\xff\xff\xff\xff";

    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i + 3 < bytes.len() {
        let b0 = bytes[i] as usize;
        let b1 = bytes[i + 1] as usize;
        let b2 = bytes[i + 2] as usize;
        let b3 = bytes[i + 3] as usize;
        if b0 >= 128 || b1 >= 128 || b2 >= 128 || b3 >= 128 {
            return None;
        }
        let v0 = TABLE[b0];
        let v1 = TABLE[b1];
        let v2 = TABLE[b2];
        let v3 = TABLE[b3];
        if v0 == 0xff || v1 == 0xff {
            return None;
        }
        out.push((v0 << 2) | (v1 >> 4));
        if bytes[i + 2] != b'=' {
            if v2 == 0xff {
                return None;
            }
            out.push((v1 << 4) | (v2 >> 2));
        }
        if bytes[i + 3] != b'=' {
            if v3 == 0xff {
                return None;
            }
            out.push((v2 << 6) | v3);
        }
        i += 4;
    }
    Some(out)
}
