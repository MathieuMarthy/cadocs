//! The REST API under `/api/v1`: its error shape and what every route shares (`docs/API.md`).

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fmt::Display;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use axum::{Json, Router};
use cadocs_collab::Authorizer;
use cadocs_store::DocStore;
use serde::Serialize;

use crate::AppState;
use crate::config::HttpConfig;
use crate::rate_limit::RateLimiter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorCode {
    InvalidRequest,
    ValidationFailed,
    Unauthenticated,
    Forbidden,
    CsrfFailed,
    NotFound,
    MethodNotAllowed,
    Conflict,
    TooLarge,
    RateLimited,
    Internal,
    Unavailable,
}

impl ErrorCode {
    pub const ALL: [Self; 12] = [
        Self::InvalidRequest,
        Self::ValidationFailed,
        Self::Unauthenticated,
        Self::Forbidden,
        Self::CsrfFailed,
        Self::NotFound,
        Self::MethodNotAllowed,
        Self::Conflict,
        Self::TooLarge,
        Self::RateLimited,
        Self::Internal,
        Self::Unavailable,
    ];

    pub fn status(self) -> StatusCode {
        match self {
            Self::InvalidRequest | Self::ValidationFailed => StatusCode::BAD_REQUEST,
            Self::Unauthenticated => StatusCode::UNAUTHORIZED,
            Self::Forbidden | Self::CsrfFailed => StatusCode::FORBIDDEN,
            Self::NotFound => StatusCode::NOT_FOUND,
            Self::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Self::Conflict => StatusCode::CONFLICT,
            Self::TooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::RateLimited => StatusCode::TOO_MANY_REQUESTS,
            Self::Internal => StatusCode::INTERNAL_SERVER_ERROR,
            Self::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::ValidationFailed => "validation_failed",
            Self::Unauthenticated => "unauthenticated",
            Self::Forbidden => "forbidden",
            Self::CsrfFailed => "csrf_failed",
            Self::NotFound => "not_found",
            Self::MethodNotAllowed => "method_not_allowed",
            Self::Conflict => "conflict",
            Self::TooLarge => "too_large",
            Self::RateLimited => "rate_limited",
            Self::Internal => "internal",
            Self::Unavailable => "unavailable",
        }
    }

    fn message(self) -> &'static str {
        match self {
            Self::InvalidRequest => "Invalid request",
            Self::ValidationFailed => "Validation failed",
            Self::Unauthenticated => "Not logged in",
            Self::Forbidden => "Forbidden",
            Self::CsrfFailed => "Missing or invalid CSRF token",
            Self::NotFound => "Not found",
            Self::MethodNotAllowed => "Method not allowed",
            Self::Conflict => "Conflict",
            Self::TooLarge => "Too large",
            Self::RateLimited => "Too many requests",
            Self::Internal => "Internal error",
            Self::Unavailable => "Unavailable",
        }
    }
}

/// An error as clients see it. A handler returns this, with `?` where it can.
#[derive(Debug)]
pub struct ApiError {
    code: ErrorCode,
    message: Cow<'static, str>,
    fields: BTreeMap<&'static str, &'static str>,
}

impl ApiError {
    pub fn new(code: ErrorCode) -> Self {
        Self {
            code,
            message: code.message().into(),
            fields: BTreeMap::new(),
        }
    }

    /// Replaces the generic message with one a user can act on.
    pub fn with_message(mut self, message: impl Into<Cow<'static, str>>) -> Self {
        self.message = message.into();
        self
    }

    /// Names the problem with one field, as a stable code the front end can translate.
    pub fn with_field(mut self, field: &'static str, problem: &'static str) -> Self {
        self.fields.insert(field, problem);
        self
    }

    /// Logs the cause and hides it from the client.
    pub fn internal(cause: impl Display) -> Self {
        tracing::error!(%cause, "request failed");
        Self::new(ErrorCode::Internal)
    }
}

#[derive(Serialize)]
struct ErrorBody<'a> {
    error: ErrorDetail<'a>,
}

#[derive(Serialize)]
struct ErrorDetail<'a> {
    code: &'static str,
    message: &'a str,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    fields: &'a BTreeMap<&'static str, &'static str>,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: ErrorDetail {
                code: self.code.as_str(),
                message: &self.message,
                fields: &self.fields,
            },
        };
        (self.code.status(), Json(body)).into_response()
    }
}

pub fn router<S: DocStore, A: Authorizer>(http: Arc<HttpConfig>) -> Router<AppState<S, A>> {
    // A nested fallback only sees paths below the root of the nest. `router` in `lib.rs` also
    // covers the root with a slash, which a nest cannot reach.
    let routes = Router::new().route("/", any(not_found));
    finish(routes, http)
}

/// Must come after every route: both calls below only reach the routes that already exist.
fn finish<T>(routes: Router<T>, http: Arc<HttpConfig>) -> Router<T>
where
    T: Clone + Send + Sync + 'static,
{
    routes
        .fallback(not_found)
        .method_not_allowed_fallback(method_not_allowed)
        .layer(from_fn_with_state(http, check_origin))
}

pub(crate) async fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound)
}

async fn method_not_allowed() -> ApiError {
    ApiError::new(ErrorCode::MethodNotAllowed)
}

async fn check_origin(
    State(http): State<Arc<HttpConfig>>,
    request: Request,
    next: Next,
) -> Response {
    if http.public_url.allows(request.headers()) {
        next.run(request).await
    } else {
        ApiError::new(ErrorCode::Forbidden).into_response()
    }
}

/// A layer for one route: `route_layer(from_fn_with_state((limiter, http), api::rate_limit))`.
pub async fn rate_limit(
    State((limiter, http)): State<(Arc<RateLimiter>, Arc<HttpConfig>)>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    request: Request,
    next: Next,
) -> Response {
    let client = http.trusted_proxies.client_ip(peer.ip(), request.headers());
    match limiter.check(client) {
        Ok(()) => next.run(request).await,
        Err(wait) => {
            let mut response = ApiError::new(ErrorCode::RateLimited).into_response();
            response
                .headers_mut()
                .insert(header::RETRY_AFTER, HeaderValue::from(whole_secs(wait)));
            response
        }
    }
}

/// Rounded up: a client that waits this long is sure to pass.
fn whole_secs(wait: Duration) -> u64 {
    wait.as_secs() + u64::from(wait.subsec_nanos() > 0)
}

#[cfg(test)]
mod tests {
    use axum::body::Body;
    use axum::routing::post;
    use tower::ServiceExt;

    use super::*;
    use crate::client_ip::TrustedProxies;
    use crate::config::PublicUrl;

    /// A route as later steps will add them, finished the way `router` finishes its routes.
    fn with_a_post_route() -> Router {
        let http = HttpConfig {
            public_url: PublicUrl::parse("https://wiki.example.org").unwrap(),
            trusted_proxies: TrustedProxies::default(),
        };
        finish(
            Router::new().route("/thing", post(|| async { "done" })),
            Arc::new(http),
        )
    }

    #[tokio::test]
    async fn a_wrong_method_answers_in_the_error_shape() {
        let request = Request::get("/thing").body(Body::empty()).unwrap();
        let response = with_a_post_route().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        let body: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["error"]["code"], "method_not_allowed");
    }

    #[tokio::test]
    async fn the_origin_check_covers_real_routes() {
        let request = |origin| {
            Request::post("/thing")
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap()
        };
        let app = with_a_post_route();
        let refused = app.clone().oneshot(request("https://evil.example"));
        assert_eq!(refused.await.unwrap().status(), StatusCode::FORBIDDEN);
        let allowed = app.oneshot(request("https://wiki.example.org"));
        assert_eq!(allowed.await.unwrap().status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn an_internal_error_hides_its_cause() {
        let response = ApiError::internal("disk /srv/secret is full").into_response();
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({ "error": { "code": "internal", "message": "Internal error" } })
        );
    }

    #[tokio::test]
    async fn a_validation_error_names_its_fields() {
        let response = ApiError::new(ErrorCode::ValidationFailed)
            .with_message("Title is too long")
            .with_field("title", "too_long")
            .into_response();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = axum::body::to_bytes(response.into_body(), 1024)
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({ "error": {
                "code": "validation_failed",
                "message": "Title is too long",
                "fields": { "title": "too_long" },
            } })
        );
    }

    #[test]
    fn a_wait_is_rounded_up_to_whole_seconds() {
        assert_eq!(whole_secs(Duration::from_secs(90)), 90);
        assert_eq!(whole_secs(Duration::from_millis(1)), 1);
        assert_eq!(whole_secs(Duration::from_millis(1500)), 2);
    }

    #[test]
    fn each_code_has_the_status_of_the_api_guide() {
        let expected = [
            (ErrorCode::InvalidRequest, 400, "invalid_request"),
            (ErrorCode::ValidationFailed, 400, "validation_failed"),
            (ErrorCode::Unauthenticated, 401, "unauthenticated"),
            (ErrorCode::Forbidden, 403, "forbidden"),
            (ErrorCode::CsrfFailed, 403, "csrf_failed"),
            (ErrorCode::NotFound, 404, "not_found"),
            (ErrorCode::MethodNotAllowed, 405, "method_not_allowed"),
            (ErrorCode::Conflict, 409, "conflict"),
            (ErrorCode::TooLarge, 413, "too_large"),
            (ErrorCode::RateLimited, 429, "rate_limited"),
            (ErrorCode::Internal, 500, "internal"),
            (ErrorCode::Unavailable, 503, "unavailable"),
        ];
        assert_eq!(expected.len(), ErrorCode::ALL.len());
        for (code, status, name) in expected {
            assert_eq!(code.status().as_u16(), status, "{name}");
            assert_eq!(code.as_str(), name);
            assert!(!code.message().is_empty(), "{name}");
        }
    }
}
