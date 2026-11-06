//! Session tokens, the session cookie, and the middleware of authenticated routes.

use std::time::Duration;

use axum::extract::{Request, State};
use axum::http::header::{COOKIE, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use cadocs_store::{AccountStore, NewPasswordReset, User, now_ms};
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use tokio::time::timeout;

use crate::AppState;
use crate::api::{ApiError, ErrorCode};
use crate::config::{HttpConfig, PublicUrl};

const COOKIE_NAME: &str = "session";
const CSRF_HEADER: &str = "x-csrf-token";
/// `docs/DATA_MODEL.md`: so that reads do not turn into writes.
const TOUCH_INTERVAL_MS: i64 = 5 * 60 * 1000;
const TOUCH_WAIT: Duration = Duration::from_secs(1);
/// Each value costs a query, and a client chooses how many it sends.
const MAX_COOKIES: usize = 4;
/// ADR 0013.
const RESET_LIFETIME: Duration = Duration::from_secs(60 * 60);

/// Handlers behind `require` read this from the request extensions.
#[derive(Debug, Clone)]
pub struct CurrentSession {
    pub user: User,
    pub token: String,
}

impl CurrentSession {
    pub fn token_hash(&self) -> String {
        token_hash(&self.token)
    }
}

pub fn new_token() -> Result<String, getrandom::Error> {
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)?;
    Ok(hex(&bytes))
}

pub fn token_hash(token: &str) -> String {
    hex(&Sha256::digest(token))
}

/// A link to hand over, and the row that makes it work, of which only the hash of the token is
/// kept. The page behind the link sends the token to `POST /auth/password-reset/{token}`.
pub fn new_password_reset(
    public_url: &PublicUrl,
    user_id: &str,
    created_by: Option<&str>,
) -> Result<(String, NewPasswordReset), getrandom::Error> {
    let token = new_token()?;
    let now = now_ms();
    let reset = NewPasswordReset {
        token_hash: token_hash(&token),
        user_id: user_id.to_owned(),
        created_by: created_by.map(str::to_owned),
        now,
        expires_at: now + RESET_LIFETIME.as_millis() as i64,
    };
    let link = format!("{}/reset-password/{token}", public_url.origin());
    Ok((link, reset))
}

/// Derived from the token, so it needs no column, and it lasts as long as the session. Only a
/// page of our origin can read it, through `GET /session`.
pub fn csrf_token(token: &str) -> String {
    hex(&Sha256::new()
        .chain_update("cadocs-csrf:")
        .chain_update(token)
        .finalize())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(DIGITS[usize::from(byte >> 4)] as char);
        text.push(DIGITS[usize::from(byte & 15)] as char);
    }
    text
}

pub fn cookie(token: &str, http: &HttpConfig) -> HeaderValue {
    cookie_with(token, http.session_lifetime.as_secs(), http)
}

/// Tells the browser to forget the session.
pub fn removal_cookie(http: &HttpConfig) -> HeaderValue {
    cookie_with("", 0, http)
}

fn cookie_with(token: &str, max_age: u64, http: &HttpConfig) -> HeaderValue {
    let secure = if http.public_url.secure() {
        "; Secure"
    } else {
        ""
    };
    let cookie =
        format!("{COOKIE_NAME}={token}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}");
    HeaderValue::try_from(cookie).expect("a token is hexadecimal")
}

/// Every value of our cookie. Another app of the parent domain may use the same name, and a browser
/// sends both. Bytes, not text: one foreign cookie that is not ASCII must not hide ours.
fn tokens_of(headers: &HeaderMap) -> Vec<String> {
    let prefix = format!("{COOKIE_NAME}=");
    headers
        .get_all(COOKIE)
        .iter()
        .flat_map(|value| value.as_bytes().split(|byte| *byte == b';'))
        .filter_map(|pair| pair.trim_ascii().strip_prefix(prefix.as_bytes()))
        .filter_map(|token| std::str::from_utf8(token).ok())
        .map(str::to_owned)
        .take(MAX_COOKIES)
        .collect()
}

fn csrf_matches(headers: &HeaderMap, token: &str) -> bool {
    let expected = csrf_token(token);
    headers
        .get(CSRF_HEADER)
        .is_some_and(|sent| bool::from(sent.as_bytes().ct_eq(expected.as_bytes())))
}

/// Layer of every route that needs a user. It also checks CSRF, so that no such route can forget.
pub async fn require<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    mut request: Request,
    next: Next,
) -> Response {
    let now = now_ms();
    let mut found = None;
    for token in tokens_of(request.headers()) {
        match state.store.session(&token_hash(&token), now).await {
            Ok(Some(session)) => {
                found = Some((token, session));
                break;
            }
            Ok(None) => {}
            Err(error) => return ApiError::from(error).into_response(),
        }
    }
    let Some((token, session)) = found else {
        return ApiError::new(ErrorCode::Unauthenticated).into_response();
    };

    if !request.method().is_safe() && !csrf_matches(request.headers(), &token) {
        return ApiError::new(ErrorCode::CsrfFailed).into_response();
    }

    let due = now - session.last_seen_at >= TOUCH_INTERVAL_MS;
    let renewed = (due && touch(&state, &token, now).await).then(|| cookie(&token, &state.http));
    request.extensions_mut().insert(CurrentSession {
        user: session.user,
        token,
    });
    let mut response = next.run(request).await;
    // The browser's copy expires too. A handler that set a cookie, as logout does, has the last word.
    if let Some(cookie) = renewed
        && !response.headers().contains_key(SET_COOKIE)
    {
        response.headers_mut().insert(SET_COOKIE, cookie);
    }
    response
}

/// Extends a session, and says whether it worked. The session is valid either way: a full disk or
/// a busy writer must neither log people out nor hold up what is only a read.
async fn touch<S: AccountStore, A>(state: &AppState<S, A>, token: &str, now: i64) -> bool {
    let expires_at = now + state.http.session_lifetime.as_millis() as i64;
    let hash = token_hash(token);
    match timeout(
        TOUCH_WAIT,
        state.store.touch_session(&hash, now, expires_at),
    )
    .await
    {
        Ok(Ok(())) => true,
        Ok(Err(error)) => {
            tracing::warn!(%error, "cannot extend a session");
            false
        }
        Err(_) => {
            tracing::warn!("extending a session took too long");
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_are_long_and_never_repeat() {
        let token = new_token().unwrap();
        assert_eq!(token.len(), 64);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
        assert_ne!(token, new_token().unwrap());
    }

    #[test]
    fn the_hash_is_sha_256_in_hexadecimal() {
        assert_eq!(
            token_hash("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_csrf_token_reveals_neither_the_session_nor_its_hash() {
        let token = "a".repeat(64);
        let csrf = csrf_token(&token);
        assert_ne!(csrf, token);
        assert_ne!(csrf, token_hash(&token));
        assert_eq!(csrf, csrf_token(&token));
        assert_ne!(csrf, csrf_token(&"b".repeat(64)));
    }
}
