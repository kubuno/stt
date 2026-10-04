//! Caller authentication for the STT module.
//!
//! The module listens on loopback and is reached through the core's proxy, which
//! mints a short-lived identity token (`X-Kubuno-Auth`, see `kubuno-modauth`)
//! signed with this module's internal secret — on plain HTTP requests and on the
//! WebSocket handshake of `/stream` alike. Every route but `/health` requires
//! that token: the plain `X-Kubuno-User-*` headers are never trusted, since any
//! local process reaching the port could forge them, and an anonymous caller
//! could otherwise burn the server's CPU on Whisper or administer the module.

use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderMap, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;

/// This module's id: the audience its identity tokens are minted for.
pub const MODULE_ID: &str = "stt";

/// The verified caller, stored in the request extensions by [`require_auth`].
#[derive(Debug, Clone)]
pub struct Caller {
    pub role: String,
}

impl Caller {
    pub fn is_admin(&self) -> bool {
        self.role == "admin"
    }
}

/// Verifies the signed identity token of a request.
///
/// An empty configured secret refuses everything: an HMAC keyed with an empty
/// secret could be minted by anyone.
pub fn authenticate(secret: &str, headers: &HeaderMap) -> Result<Caller, StatusCode> {
    if secret.is_empty() {
        return Err(StatusCode::UNAUTHORIZED);
    }
    let token = headers
        .get(kubuno_modauth::TOKEN_HEADER)
        .and_then(|v| v.to_str().ok())
        .ok_or(StatusCode::UNAUTHORIZED)?;
    let user = kubuno_modauth::verify(secret.as_bytes(), token, MODULE_ID)
        .map_err(|_| StatusCode::UNAUTHORIZED)?;
    Ok(Caller { role: user.role })
}

/// Router layer: every route it wraps requires a valid identity token.
pub async fn require_auth(State(secret): State<Arc<String>>, mut req: Request, next: Next) -> Response {
    if secret.is_empty() {
        tracing::error!("stt: KUBUNO_INTERNAL_SECRET is empty, every request is refused");
    }
    match authenticate(&secret, req.headers()) {
        Ok(caller) => {
            req.extensions_mut().insert(caller);
            next.run(req).await
        }
        Err(code) => (code, Json(json!({ "error": "unauthorized" }))).into_response(),
    }
}

/// Admin routes: the role comes from the verified token only. `Some` is the
/// refusal to return.
pub fn admin_denied(caller: &Caller) -> Option<Response> {
    if caller.is_admin() {
        None
    } else {
        Some((StatusCode::FORBIDDEN, Json(json!({ "error": "admin only" }))).into_response())
    }
}

/// A model or engine name used to build a file path: letters, digits, `.`, `_`
/// and `-` only, never `.`/`..` or a leading dot, at most 128 characters.
pub fn is_safe_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && !name.starts_with('.')
        && !name.contains("..")
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    const SECRET: &str = "module-secret";

    fn token(secret: &str, role: &str, aud: &str) -> String {
        let user = kubuno_modauth::ModuleUser {
            id: uuid::Uuid::new_v4(),
            role: role.into(),
            email: "a@example.org".into(),
        };
        kubuno_modauth::sign(secret.as_bytes(), &user, aud)
    }

    fn headers(pairs: &[(&str, &str)]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for (k, v) in pairs {
            h.insert(
                axum::http::HeaderName::from_bytes(k.as_bytes()).expect("name"),
                HeaderValue::from_str(v).expect("value"),
            );
        }
        h
    }

    #[test]
    fn forged_plain_headers_are_refused() {
        let h = headers(&[("x-kubuno-user-id", "6f2c1a3e-0000-4000-8000-000000000000"), ("x-kubuno-user-role", "admin")]);
        assert_eq!(authenticate(SECRET, &h).err(), Some(StatusCode::UNAUTHORIZED));
        assert_eq!(authenticate(SECRET, &HeaderMap::new()).err(), Some(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn a_valid_token_is_accepted() {
        let h = headers(&[(kubuno_modauth::TOKEN_HEADER, &token(SECRET, "user", MODULE_ID))]);
        let caller = authenticate(SECRET, &h).expect("valid");
        assert!(!caller.is_admin());
    }

    #[test]
    fn a_token_for_another_module_or_key_is_refused() {
        let other_aud = headers(&[(kubuno_modauth::TOKEN_HEADER, &token(SECRET, "admin", "drive"))]);
        assert_eq!(authenticate(SECRET, &other_aud).err(), Some(StatusCode::UNAUTHORIZED));
        let other_key = headers(&[(kubuno_modauth::TOKEN_HEADER, &token("another", "admin", MODULE_ID))]);
        assert_eq!(authenticate(SECRET, &other_key).err(), Some(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn an_empty_secret_refuses_everything() {
        // A token signed with the empty key must not open an unconfigured module.
        let h = headers(&[(kubuno_modauth::TOKEN_HEADER, &token("", "admin", MODULE_ID))]);
        assert_eq!(authenticate("", &h).err(), Some(StatusCode::UNAUTHORIZED));
    }

    #[test]
    fn admin_routes_need_the_admin_role_from_the_token() {
        let user = authenticate(SECRET, &headers(&[(kubuno_modauth::TOKEN_HEADER, &token(SECRET, "user", MODULE_ID))]))
            .expect("valid");
        let denied = admin_denied(&user).expect("refused");
        assert_eq!(denied.status(), StatusCode::FORBIDDEN);
        let admin = authenticate(SECRET, &headers(&[(kubuno_modauth::TOKEN_HEADER, &token(SECRET, "admin", MODULE_ID))]))
            .expect("valid");
        assert!(admin_denied(&admin).is_none());
    }

    #[test]
    fn path_like_names_are_refused() {
        for ok in ["whisper", "ggml-base.en", "large-v3_q5"] {
            assert!(is_safe_name(ok), "{ok}");
        }
        for bad in ["", ".", "..", "../x", "..\\x", "a/b", "a\\b", ".hidden", "x..y", "c:", "a b", &"x".repeat(129)] {
            assert!(!is_safe_name(bad), "{bad}");
        }
    }

    #[tokio::test]
    async fn the_layer_refuses_forged_headers_and_lets_a_signed_call_through() {
        use axum::{body::Body, routing::get, Router};
        use tower::ServiceExt;

        let app = Router::new()
            .route("/transcribe", get(|| async { "ok" }))
            .layer(axum::middleware::from_fn_with_state(Arc::new(SECRET.to_string()), require_auth));

        let forged = Request::builder()
            .uri("/transcribe")
            .header("x-kubuno-user-role", "admin")
            .header("x-kubuno-user-id", "6f2c1a3e-0000-4000-8000-000000000000")
            .body(Body::empty())
            .expect("request");
        assert_eq!(app.clone().oneshot(forged).await.expect("response").status(), StatusCode::UNAUTHORIZED);

        let signed = Request::builder()
            .uri("/transcribe")
            .header(kubuno_modauth::TOKEN_HEADER, token(SECRET, "user", MODULE_ID))
            .body(Body::empty())
            .expect("request");
        assert_eq!(app.oneshot(signed).await.expect("response").status(), StatusCode::OK);
    }
}
