//! Shared bearer-token axum middleware for the bridge's operator/admin HTTP
//! surfaces (the validator's operator API, sig-store). Both services gate their
//! state-changing routes the same way: a shared secret presented as
//! `Authorization: Bearer <token>`, compared in constant time so the token can't
//! be recovered via early-exit timing on the comparison itself.

use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::Response;
use std::sync::Arc;

/// Bearer-token gate: reject any request that doesn't present `expected`.
pub async fn require_auth(
    State(expected): State<Arc<String>>,
    req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let presented = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if ct_eq(presented.as_bytes(), expected.as_bytes()) {
        Ok(next.run(req).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

/// Length-checked constant-time byte comparison (avoids leaking the token via
/// early-exit timing on the compare itself).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ct_eq_is_correct() {
        assert!(ct_eq(b"abc", b"abc"));
        assert!(ct_eq(b"", b""));
        assert!(!ct_eq(b"abc", b"abd"));
        assert!(!ct_eq(b"abc", b"ab")); // length mismatch
        assert!(!ct_eq(b"", b"x"));
    }
}
