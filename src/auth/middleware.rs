// Auth middleware for CCM server.
//
// Two gateways:
//   1. `require_api_key`    — guards LLM proxy endpoints (/v1/*).
//      Clients authenticate with `x-api-key: <key>` or
//      `Authorization: Bearer <key>` (standard Anthropic/OpenAI conventions,
//      so existing clients work without changes).
//   2. `require_admin_key`  — guards admin routes (/api/config/json,
//      /api/reload, /api/logs, /, etc.). Clients send `x-ccm-admin-key: <key>`.
//
// Both middlewares are opt-in via config: when the corresponding config field
// (`server.api_key` / `server.admin_key`) is None, the middleware passes through
// (backward compatible — local single-user setups keep working unauthenticated).
//
// Security: the auth headers (`x-api-key`, `authorization`, `x-ccm-admin-key`)
// are added to `headers::BLOCK_LIST` so they are NEVER forwarded to upstream
// providers — this prevents client CCM-auth credentials from leaking to or
// conflicting with provider credentials (see headers/mod.rs).

use axum::extract::State;
use axum::http::{Request, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::server::AppState;

/// Extract a bearer token from an `Authorization` header value.
/// Accepts "Bearer <token>" (case-insensitive scheme). Returns None if the
/// header is absent or not a Bearer token.
fn bearer_token(auth_header: Option<&str>) -> Option<&str> {
    let v = auth_header?.trim();
    let (scheme, rest) = v.split_once(' ')?;
    if scheme.eq_ignore_ascii_case("Bearer") {
        Some(rest.trim())
    } else {
        None
    }
}

/// Middleware: require a valid client API key for LLM proxy endpoints.
///
/// Reads `server.api_key` from the current config snapshot. When None → pass
/// (no auth enforced). When Some(expected) → the request must carry
/// `x-api-key: expected` OR `Authorization: Bearer expected`.
pub async fn require_api_key(
    State(state): State<std::sync::Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let expected = state.snapshot().config.server.api_key.clone();
    let Some(expected) = expected else {
        return next.run(req).await;
    };
    // Empty string configured = treat as no auth (avoids footgun lockout).
    if expected.is_empty() {
        return next.run(req).await;
    }

    let provided = req
        .headers()
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            bearer_token(req.headers().get("authorization").and_then(|v| v.to_str().ok()))
        });

    match provided {
        Some(p) if constant_time_eq(p.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            "Missing or invalid API key. Send x-api-key or Authorization: Bearer header.",
        )
            .into_response(),
    }
}

/// Middleware: require a valid admin key for admin routes.
///
/// Reads `server.admin_key` from the current config snapshot. When None → pass.
/// When Some(expected) → the request must carry `x-ccm-admin-key: expected`.
pub async fn require_admin_key(
    State(state): State<std::sync::Arc<AppState>>,
    req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let expected = state.snapshot().config.server.admin_key.clone();
    let Some(expected) = expected else {
        return next.run(req).await;
    };
    if expected.is_empty() {
        return next.run(req).await;
    }

    let header_provided = req
        .headers()
        .get("x-ccm-admin-key")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.trim())
        .filter(|s| !s.is_empty());

    // EventSource (browser SSE API) cannot set custom request headers, so for
    // the admin SSE stream endpoint we also accept the key via `?key=` query
    // param. Header takes precedence; query is a fallback for SSE only.
    let provided = if header_provided.is_some() {
        header_provided
    } else if req.uri().path().ends_with("/stream") {
        req.uri()
            .query()
            .and_then(|q| {
                q.split('&')
                    .find_map(|kv| {
                        let (k, v) = kv.split_once('=')?;
                        (k == "key").then_some(v)
                    })
            })
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
    } else {
        None
    };

    match provided {
        Some(p) if constant_time_eq(p.as_bytes(), expected.as_bytes()) => next.run(req).await,
        _ => (
            StatusCode::UNAUTHORIZED,
            "Missing or invalid admin key. Send x-ccm-admin-key header.",
        )
            .into_response(),
    }
}

/// Constant-time-ish byte comparison to mitigate timing side-channels on key
/// checks. NOTE: a fully constant-time comparison requires equal lengths (the
/// `a.len() != b.len()` early-return below leaks the key *length*). For admin
/// and API keys this is acceptable: keys are high-entropy random strings, and
/// network jitter dominates any length-distinguishable timing. Upgrading to a
/// true constant-time impl (padding + iterate over max(len)) is possible but
/// offers negligible security gain for this threat model.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearer_token_parses_standard() {
        assert_eq!(bearer_token(Some("Bearer abc123")), Some("abc123"));
        assert_eq!(bearer_token(Some("bearer abc123")), Some("abc123"));
        assert_eq!(bearer_token(Some("Bearer  abc123 ")), Some("abc123"));
    }

    #[test]
    fn bearer_token_rejects_non_bearer() {
        assert_eq!(bearer_token(Some("Basic abc123")), None);
        assert_eq!(bearer_token(Some("abc123")), None);
        assert_eq!(bearer_token(None), None);
    }

    #[test]
    fn constant_time_eq_works() {
        assert!(constant_time_eq(b"abc", b"abc"));
        assert!(!constant_time_eq(b"abc", b"abd"));
        assert!(!constant_time_eq(b"abc", b"ab"));
        assert!(constant_time_eq(b"", b""));
    }

    #[test]
    fn bearer_token_handles_unicode_and_edge_cases() {
        // Unicode key preserved verbatim (trim only strips ASCII whitespace).
        assert_eq!(bearer_token(Some("Bearer 密钥")), Some("密钥"));
        // Multiple spaces between scheme and token: split_once takes first
        // space, rest retains remaining spaces; trim() collapses them.
        assert_eq!(bearer_token(Some("Bearer   multi")), Some("multi"));
        // Empty token: "Bearer " trims to "Bearer" (no space) → split_once None.
        assert_eq!(bearer_token(Some("Bearer ")), None);
        // Trailing spaces only: trim strips them, no space left → None.
        assert_eq!(bearer_token(Some("Bearer  ")), None);
        // Unknown scheme.
        assert_eq!(bearer_token(Some("Negotiate abc")), None);
        // Leading/trailing whitespace around whole header value.
        assert_eq!(bearer_token(Some("  Bearer xyz  ")), Some("xyz"));
    }
}
