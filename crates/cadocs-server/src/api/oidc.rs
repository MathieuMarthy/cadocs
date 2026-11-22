//! OpenID Connect login (ADR 0028). `/auth/oidc/{provider}` sends the browser to the provider,
//! with a cookie that binds the login to it; the callback checks the cookie, trades the code and
//! opens a session. Both are browser navigations, so both answer with a redirect: to `/` once
//! logged in, or to `/login?oidc=<problem>`.

use axum::Json;
use axum::Router;
use axum::extract::{ConnectInfo, Extension, State};
use axum::http::header::{COOKIE, LOCATION, SET_COOKIE};
use std::net::SocketAddr;

use axum::http::{HeaderMap, HeaderValue, StatusCode, Uri};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use cadocs_collab::Authorizer;
use cadocs_store::{AccountStore, DocStore, IdentityUser, StoreError};
use subtle::ConstantTimeEq;
use uuid::Uuid;

use super::auth::{
    MAX_NAME_CHARS, PasswordRequest, clean_email, clean_name, new_session, require_password,
};
use super::{ApiError, ErrorCode, JsonBody, PathParam, rate_limit};
use crate::config::{HttpConfig, Registration};
use crate::oidc::{self, Flow, Provider};
use crate::session::CurrentSession;
use crate::{AppState, session};
use cadocs_store::now_ms;
use serde_json::json;

/// Time to log in at the provider.
const FLOW_SECS: u32 = 10 * 60;

/// Why a login came back with no session, as the login screen is told.
#[derive(Debug, Clone, Copy)]
enum Problem {
    /// Anything the user cannot fix here: a provider that fails or refuses, a forged callback, a
    /// disabled account. The log says which.
    Failed,
    /// An account holds the address: its owner logs in as before, and links the provider.
    Taken,
    /// No account may be made: setup is pending or registration is closed.
    Closed,
}

/// How a link ends, as the account screen is told.
#[derive(Debug, Clone, Copy)]
enum Link {
    Linked,
    /// The subject, or a subject of this provider, is tied to an account already.
    Taken,
    Failed,
}

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore,
    A: Authorizer,
{
    // Each one makes requests to the provider: a client must not make it do many.
    let limit = from_fn_with_state(
        (state.password_guesses.clone(), state.http.clone()),
        rate_limit,
    );
    let public = Router::new()
        .route("/auth/oidc/{provider}", get(start))
        .route("/auth/oidc/{provider}/callback", get(callback))
        .route_layer(limit.clone());
    // Starting a link is a write, so it is a POST with the CSRF token: a page of another site
    // cannot send a browser to tie its own identity to the account.
    let me = Router::new()
        .route("/me/identities", get(identities))
        .route(
            "/me/identities/{provider}",
            post(start_link).route_layer(limit).delete(unlink),
        )
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>));
    public.merge(me)
}

fn provider<'a, S, A>(state: &'a AppState<S, A>, id: &str) -> Result<&'a Provider, ApiError> {
    let found = state.oidc.iter().find(|provider| provider.id == id);
    found.ok_or_else(|| ApiError::new(ErrorCode::NotFound))
}

fn redirect_uri(http: &HttpConfig, provider: &Provider) -> String {
    let origin = http.public_url.origin();
    format!("{origin}/api/v1/auth/oidc/{}/callback", provider.id)
}

/// `__Host-` over HTTPS, as the session cookie (`session::cookie_name`).
fn flow_cookie_name(http: &HttpConfig) -> &'static str {
    if http.public_url.secure() {
        "__Host-oidc"
    } else {
        "oidc"
    }
}

/// `Lax`, so the provider's redirect back, a top-level navigation, carries it.
fn flow_cookie(http: &HttpConfig, value: &str, max_age: u32) -> HeaderValue {
    let name = flow_cookie_name(http);
    let secure = if http.public_url.secure() {
        "; Secure"
    } else {
        ""
    };
    let cookie =
        format!("{name}={value}; Path=/; HttpOnly; SameSite=Lax; Max-Age={max_age}{secure}");
    HeaderValue::try_from(cookie).expect("ids and base64url are ASCII")
}

fn redirect(to: &str) -> Response {
    let location = HeaderValue::try_from(to).expect("a URL is ASCII");
    (StatusCode::SEE_OTHER, [(LOCATION, location)]).into_response()
}

/// Where to send the browser, and the cookie that binds the login (or the link, when `link` names
/// the user it is for) to it. `None` when the provider cannot be reached or is misconfigured.
async fn begin<S, A>(
    state: &AppState<S, A>,
    provider: &Provider,
    link: Option<&str>,
) -> Option<(String, HeaderValue)> {
    let id = &provider.id;
    let flow = Flow::new().ok()?;
    let redirect_uri = redirect_uri(&state.http, provider);
    let found = oidc::discover(provider).await;
    let url = found
        .and_then(|endpoints| oidc::authorization_url(&endpoints, provider, &redirect_uri, &flow));
    let url = match url {
        Ok(url) => url,
        Err(error) => {
            tracing::warn!(provider = id, "an OIDC login could not start: {error:#}");
            return None;
        }
    };
    let Flow {
        state: secret_state,
        nonce,
        verifier,
    } = &flow;
    let mut value = format!("{id}.{secret_state}.{nonce}.{verifier}");
    if let Some(user) = link {
        value = format!("{value}.{user}");
    }
    Some((url, flow_cookie(&state.http, &value, FLOW_SECS)))
}

async fn start<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    PathParam(id): PathParam,
) -> Result<Response, ApiError> {
    let provider = provider(&state, &id)?;
    let Some((url, cookie)) = begin(&state, provider, None).await else {
        return Ok(back(&state.http, Problem::Failed));
    };
    let mut response = redirect(&url);
    response.headers_mut().insert(SET_COOKIE, cookie);
    Ok(response)
}

/// The logged-in user asks to tie `provider` to their account: the front end sends the browser
/// to the `url` it gets.
async fn start_link<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    PathParam(id): PathParam,
    JsonBody(body): JsonBody<PasswordRequest>,
) -> Result<Response, ApiError> {
    let provider = provider(&state, &id)?;
    // A provider is a way in that skips this account's factors: a stolen session must not tie its
    // own. An account with no password has none to give, and only its provider to prove it by.
    let (_, has_password) = state.store.identities(&current.user.id).await?;
    if has_password {
        require_password(&state, &current, (peer, &headers), body.current_password).await?;
    }
    let Some((url, cookie)) = begin(&state, provider, Some(&current.user.id)).await else {
        return Err(
            ApiError::new(ErrorCode::Unavailable).with_message("The provider cannot be reached")
        );
    };
    let mut response = Json(json!({ "url": url })).into_response();
    response.headers_mut().insert(SET_COOKIE, cookie);
    Ok(response)
}

async fn identities<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
) -> Result<Response, ApiError> {
    let (providers, password) = state.store.identities(&current.user.id).await?;
    Ok(Json(json!({ "providers": providers, "password": password })).into_response())
}

async fn unlink<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<StatusCode, ApiError> {
    match state.store.unlink_identity(&current.user.id, &id).await? {
        true => Ok(StatusCode::NO_CONTENT),
        false => Err(ApiError::new(ErrorCode::NotFound)),
    }
}

/// To the login screen, with the login's cookie gone.
fn back(http: &HttpConfig, problem: Problem) -> Response {
    let problem = match problem {
        Problem::Failed => "failed",
        Problem::Taken => "taken",
        Problem::Closed => "closed",
    };
    let mut response = redirect(&format!("/login?oidc={problem}"));
    response
        .headers_mut()
        .insert(SET_COOKIE, flow_cookie(http, "", 0));
    response
}

async fn callback<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    PathParam(id): PathParam,
    uri: Uri,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let provider = provider(&state, &id)?;
    let (identity, link) = match exchange(&state, provider, &uri, &headers).await {
        Ok(found) => found,
        Err(Problem::Failed)
            if flow_of(&headers, &state.http, provider).is_some_and(|f| f.1.is_some()) =>
        {
            return Ok(back_to_account(&state.http, Link::Failed));
        }
        Err(problem) => return Ok(back(&state.http, problem)),
    };
    if let Some(user_id) = link {
        let outcome = link_to(&state, provider, &headers, &user_id, &identity.subject).await;
        return Ok(back_to_account(&state.http, outcome));
    }
    match log_in(&state, provider, identity).await {
        Ok(token) => {
            let mut response = redirect("/");
            let cookies = response.headers_mut();
            cookies.append(SET_COOKIE, flow_cookie(&state.http, "", 0));
            cookies.append(SET_COOKIE, session::cookie(&token, &state.http));
            Ok(response)
        }
        Err(problem) => Ok(back(&state.http, problem)),
    }
}

/// To the account screen, with the link's cookie gone.
fn back_to_account(http: &HttpConfig, outcome: Link) -> Response {
    let outcome = match outcome {
        Link::Linked => "linked",
        Link::Taken => "taken",
        Link::Failed => "failed",
    };
    let mut response = redirect(&format!("/account?oidc={outcome}"));
    response
        .headers_mut()
        .insert(SET_COOKIE, flow_cookie(http, "", 0));
    response
}

/// Ties the subject to `user_id`, if the browser that comes back is still logged in as them.
async fn link_to<S: AccountStore, A>(
    state: &AppState<S, A>,
    provider: &Provider,
    headers: &HeaderMap,
    user_id: &str,
    subject: &str,
) -> Link {
    let current = session::current(state, headers).await;
    if !matches!(current, Ok(Some(ref found)) if found.user.id == user_id) {
        tracing::warn!(
            provider = provider.id,
            "an OIDC link came back to another session"
        );
        return Link::Failed;
    }
    match state
        .store
        .link_identity(user_id, &provider.id, subject, now_ms())
        .await
    {
        Ok(()) => Link::Linked,
        Err(StoreError::IdentityTaken) => Link::Taken,
        Err(error) => {
            tracing::warn!(provider = provider.id, "an OIDC link failed: {error}");
            Link::Failed
        }
    }
}

/// The flow this browser started for `provider`, from its cookie.
fn flow_of(
    headers: &HeaderMap,
    http: &HttpConfig,
    provider: &Provider,
) -> Option<(Flow, Option<String>)> {
    let prefix = format!("{}=", flow_cookie_name(http));
    let value = headers
        .get_all(COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .find_map(|pair| pair.trim().strip_prefix(&prefix))?;
    let mut parts = value.split('.');
    let (Some(id), Some(state), Some(nonce), Some(verifier)) =
        (parts.next(), parts.next(), parts.next(), parts.next())
    else {
        return None;
    };
    // A fifth part is the user a link is for.
    let link = parts.next();
    if parts.next().is_some() || link == Some("") {
        return None;
    }
    let filled = [state, nonce, verifier].iter().all(|part| !part.is_empty());
    (id == provider.id && filled).then(|| {
        let flow = Flow {
            state: state.to_owned(),
            nonce: nonce.to_owned(),
            verifier: verifier.to_owned(),
        };
        (flow, link.map(str::to_owned))
    })
}

/// Checks the callback and trades its code: who the provider says logged in, and the user the
/// flow was a link for.
async fn exchange<S: AccountStore, A>(
    state: &AppState<S, A>,
    provider: &Provider,
    uri: &Uri,
    headers: &HeaderMap,
) -> Result<(oidc::Identity, Option<String>), Problem> {
    let failed = |why: &dyn std::fmt::Display| {
        tracing::warn!(provider = provider.id, "an OIDC login failed: {why}");
        Problem::Failed
    };
    let query = uri.query().unwrap_or_default().as_bytes();
    let field = |name: &str| {
        url::form_urlencoded::parse(query).find_map(|(key, value)| (key == name).then_some(value))
    };
    if let Some(error) = field("error") {
        return Err(failed(&format!("the provider answered {error:?}")));
    }
    let Some((flow, link)) = flow_of(headers, &state.http, provider) else {
        return Err(failed(&"no login of this provider started in this browser"));
    };
    // Login CSRF: the code must come back to the browser that asked for it.
    let sent = field("state").unwrap_or_default();
    if !bool::from(sent.as_bytes().ct_eq(flow.state.as_bytes())) {
        return Err(failed(&"the state is not the one this browser sent"));
    }
    let Some(code) = field("code").filter(|code| !code.is_empty()) else {
        return Err(failed(&"the provider sent no code"));
    };
    let redirect_uri = redirect_uri(&state.http, provider);
    let endpoints = oidc::discover(provider)
        .await
        .map_err(|e| failed(&format!("{e:#}")))?;
    let identity = oidc::identify(&endpoints, provider, &redirect_uri, &flow, &code)
        .await
        .map_err(|e| failed(&format!("{e:#}")))?;
    Ok((identity, link))
}

/// Returns the token of the new session.
async fn log_in<S: AccountStore, A>(
    state: &AppState<S, A>,
    provider: &Provider,
    identity: oidc::Identity,
) -> Result<String, Problem> {
    let failed = |why: &dyn std::fmt::Display| {
        tracing::warn!(provider = provider.id, "an OIDC login failed: {why}");
        Problem::Failed
    };
    let store = &state.store;
    let stored = |error: StoreError| failed(&error);
    let found = store.identity_user(&provider.id, &identity.subject).await;
    if let Some(found) = found.map_err(stored)? {
        let (token, session) = new_session(state, &found.user.id).map_err(|_| Problem::Failed)?;
        // The provider vouches for the person: its own factors apply, not this account's.
        return match store.create_identity_session(&session).await {
            Ok(true) => Ok(token),
            Ok(false) => Err(failed(&"the account is disabled")),
            Err(error) => Err(stored(error)),
        };
    }

    if state.registration == Registration::Closed {
        return Err(Problem::Closed);
    }
    let email = identity.email.as_deref().unwrap_or_default();
    let email = clean_email(email).map_err(|why| failed(&format!("email {why}")))?;
    let name = identity.name.as_deref().unwrap_or_default().trim();
    let name: String = name.chars().take(MAX_NAME_CHARS).collect();
    let name = clean_name(&name)
        .or_else(|_| clean_name(email.split('@').next().unwrap_or_default()))
        .map_err(|why| failed(&format!("name {why}")))?;
    let user = IdentityUser {
        id: Uuid::now_v7().to_string(),
        email,
        name,
        email_verified: identity.email_verified,
        provider: provider.id.clone(),
        subject: identity.subject,
    };
    let (token, session) = new_session(state, &user.id).map_err(|_| Problem::Failed)?;
    match store.register_identity(&user, &session).await {
        Ok(()) => Ok(token),
        // Never a way into an account that exists: its owner links the provider.
        Err(StoreError::EmailTaken) => Err(Problem::Taken),
        Err(StoreError::NotAllowed) => Err(Problem::Closed),
        Err(error) => Err(stored(error)),
    }
}
