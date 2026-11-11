//! Optional bearer-token middleware.
//!
//! When the server is started without `--api-key`, every request is allowed
//! through (server runs in "open" mode — appropriate for `localhost`-only
//! deployments). When a key is configured, every `/v1/*` request must
//! present `Authorization: Bearer <key>` and the comparison is constant-
//! time to avoid timing-leaking the key.

use std::net::IpAddr;

use axum::{
    extract::{Request, State},
    http::{
        StatusCode,
        header::{HOST, ORIGIN},
    },
    middleware::Next,
    response::Response,
};
use subtle::ConstantTimeEq;

use crate::state::AppState;

/// Axum middleware. Mount with `axum::middleware::from_fn_with_state`.
pub async fn require_bearer(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let Some(expected) = state.api_key() else {
        // Open mode — no key configured. Allow everything.
        return Ok(next.run(request).await);
    };

    let Some(header) = request
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
    else {
        return Err(StatusCode::UNAUTHORIZED);
    };

    let presented = header
        .strip_prefix("Bearer ")
        .or_else(|| header.strip_prefix("bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    if presented.as_bytes().ct_eq(expected.as_bytes()).into() {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

pub async fn require_local_origin(
    State(check_host): State<bool>,
    request: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let headers = request.headers();
    let host_ok = !check_host
        || headers
            .get(HOST)
            .and_then(|v| v.to_str().ok())
            .is_some_and(is_loopback_authority);
    let origin_ok = headers
        .get(ORIGIN)
        .is_none_or(|v| v.to_str().is_ok_and(is_loopback_origin));
    if host_ok && origin_ok {
        Ok(next.run(request).await)
    } else {
        Err(StatusCode::FORBIDDEN)
    }
}

pub fn is_loopback_origin(origin: &str) -> bool {
    origin
        .strip_prefix("http://")
        .or_else(|| origin.strip_prefix("https://"))
        .is_some_and(is_loopback_authority)
}

fn is_loopback_authority(authority: &str) -> bool {
    let (host, port) = match authority.strip_prefix('[') {
        Some(rest) => match rest.split_once(']') {
            Some((host, port)) => (host, port),
            None => return false,
        },
        None => authority.split_at(authority.find(':').unwrap_or(authority.len())),
    };
    let port_ok = port.is_empty()
        || port
            .strip_prefix(':')
            .is_some_and(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()));
    port_ok
        && (host.eq_ignore_ascii_case("localhost")
            || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loopback_authorities_pass_and_rebound_names_fail() {
        for ok in [
            "localhost",
            "localhost:3000",
            "LOCALHOST:3000",
            "127.0.0.1",
            "127.0.0.1:3000",
            "[::1]",
            "[::1]:3000",
        ] {
            assert!(is_loopback_authority(ok), "{ok}");
        }
        for bad in [
            "attacker.test",
            "attacker.test:3000",
            "localhost.attacker.test:3000",
            "127.0.0.1.nip.io",
            "localhost:3000@evil",
            "localhost:",
            "[::1",
            "[::1]x",
            "10.0.0.1:3000",
            "",
        ] {
            assert!(!is_loopback_authority(bad), "{bad}");
        }
    }

    #[test]
    fn only_loopback_http_origins_pass() {
        assert!(is_loopback_origin("http://localhost:5173"));
        assert!(is_loopback_origin("http://127.0.0.1:3000"));
        assert!(!is_loopback_origin("http://attacker.test:3000"));
        assert!(!is_loopback_origin("null"));
        assert!(!is_loopback_origin("file://"));
    }
}
