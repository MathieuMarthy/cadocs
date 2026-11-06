//! Accounts and sessions: first run, login, logout, `/session` and `/me`.

use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::header::{CACHE_CONTROL, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_store::{
    AccountStore, DocStore, FirstUser, Membership, NewSession, UpdateBasis, User, UserChanges,
    now_ms,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, ErrorCode, JsonBody, count_request, rate_limit};
use crate::AppState;
use crate::session::{self, CurrentSession, csrf_token, new_token, token_hash};

/// Each client can guess this many passwords at once, then one per `GUESS_REFILL`. Setup, login
/// and a `PATCH /me` that checks the current password share this budget.
pub const GUESS_BURST: u32 = 10;
pub const GUESS_REFILL: Duration = Duration::from_secs(15);

/// Far above any of these bodies. Routes before a session must not buffer the default 2 MB.
const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_NAME_CHARS: usize = 100;
const MAX_EMAIL_BYTES: usize = 254;
const MIN_PASSWORD_CHARS: usize = 8;
const MAX_PASSWORD_CHARS: usize = 1024;
const MAX_SLUG_CHARS: usize = 64;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore,
    A: Authorizer,
{
    let limit = from_fn_with_state(
        (state.password_guesses.clone(), state.http.clone()),
        rate_limit,
    );
    let public = Router::new()
        .route("/auth/setup", post(setup).route_layer(limit.clone()))
        .route("/auth/login", post(login).route_layer(limit));
    let authenticated = Router::new()
        .route("/auth/logout", post(logout))
        .route("/session", get(current_session))
        .route("/me", patch(update_me).delete(delete_me))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>));
    public
        .merge(authenticated)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

#[derive(Serialize)]
struct UserBody {
    id: String,
    email: String,
    name: String,
    is_instance_admin: bool,
}

impl From<User> for UserBody {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            name: user.name,
            is_instance_admin: user.is_instance_admin,
        }
    }
}

#[derive(Serialize)]
struct WorkspaceBody {
    id: String,
    slug: String,
    name: String,
    role: String,
}

#[derive(Serialize)]
struct SessionBody {
    user: UserBody,
    workspaces: Vec<WorkspaceBody>,
    csrf_token: String,
}

fn session_body(user: User, workspaces: Vec<Membership>, token: &str) -> Response {
    let body = SessionBody {
        user: user.into(),
        workspaces: workspaces
            .into_iter()
            .map(|membership| WorkspaceBody {
                id: membership.workspace_id,
                slug: membership.slug,
                name: membership.name,
                role: membership.role,
            })
            .collect(),
        csrf_token: csrf_token(token),
    };
    let no_store = [(CACHE_CONTROL, HeaderValue::from_static("no-store"))];
    (no_store, Json(body)).into_response()
}

fn new_session<S, A>(
    state: &AppState<S, A>,
    user_id: &str,
) -> Result<(String, NewSession), ApiError> {
    let token = new_token().map_err(ApiError::internal)?;
    let now = now_ms();
    let session = NewSession {
        token_hash: token_hash(&token),
        user_id: user_id.to_owned(),
        now,
        expires_at: now + state.http.session_lifetime.as_millis() as i64,
    };
    Ok((token, session))
}

#[derive(Deserialize)]
struct SetupRequest {
    email: String,
    name: String,
    password: String,
    workspace_name: String,
}

async fn setup<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    JsonBody(body): JsonBody<SetupRequest>,
) -> Result<Response, ApiError> {
    // Before validating, and before the cost of a hash. `create_first_user` is what decides.
    if state.store.has_users().await? {
        return Err(cadocs_store::StoreError::AlreadySetUp.into());
    }

    let mut fields = Fields::default();
    let email = fields.check("email", clean_email(&body.email));
    let name = fields.check("name", clean_name(&body.name));
    fields.check("password", check_password(&body.password));
    let workspace_name = fields.check("workspace_name", clean_name(&body.workspace_name));
    fields.finish()?;

    let user = User {
        id: Uuid::now_v7().to_string(),
        email,
        name,
        is_instance_admin: true,
    };
    let first = FirstUser {
        id: user.id.clone(),
        email: user.email.clone(),
        name: user.name.clone(),
        password_hash: state.passwords.hash(body.password).await?,
        workspace_id: Uuid::now_v7().to_string(),
        workspace_slug: slug(&workspace_name),
        workspace_name,
    };
    let (token, session) = new_session(&state, &user.id)?;
    state.store.create_first_user(&first, &session).await?;

    // Built from what is at hand: past the commit, a failed read would hide that the account exists.
    let workspace = Membership {
        workspace_id: first.workspace_id,
        slug: first.workspace_slug,
        name: first.workspace_name,
        role: "owner".to_owned(),
    };
    let mut response = session_body(user, vec![workspace], &token);
    *response.status_mut() = StatusCode::CREATED;
    response
        .headers_mut()
        .insert(SET_COOKIE, session::cookie(&token, &state.http));
    Ok(response)
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
}

/// An unknown email, a wrong password and a disabled account answer alike, and in the same time.
async fn login<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    JsonBody(body): JsonBody<LoginRequest>,
) -> Result<Response, ApiError> {
    let email = body.email.trim().to_lowercase();
    let found = state.store.credentials(&email).await?;
    let hash = found.as_ref().and_then(|found| found.password_hash.clone());
    let matches = state.passwords.verify(body.password, hash).await?;
    let Some(found) = found.filter(|found| matches && !found.disabled) else {
        return Err(wrong_credentials());
    };

    let workspaces = state.store.memberships(&found.user.id).await?;
    let (token, session) = new_session(&state, &found.user.id)?;
    let checked = found.password_hash.as_deref().unwrap_or_default();
    if !state.store.create_session(&session, checked).await? {
        return Err(wrong_credentials());
    }
    let mut response = session_body(found.user, workspaces, &token);
    response
        .headers_mut()
        .insert(SET_COOKIE, session::cookie(&token, &state.http));
    Ok(response)
}

fn wrong_credentials() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated).with_message("Wrong email or password")
}

async fn logout<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
) -> Result<Response, ApiError> {
    state.store.delete_session(&current.token_hash()).await?;
    Ok(session_ended(&state))
}

fn session_ended<S, A>(state: &AppState<S, A>) -> Response {
    let removal = [(SET_COOKIE, session::removal_cookie(&state.http))];
    (StatusCode::NO_CONTENT, removal).into_response()
}

async fn current_session<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
) -> Result<Response, ApiError> {
    let workspaces = state.store.memberships(&current.user.id).await?;
    Ok(session_body(current.user, workspaces, &current.token))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateMeRequest {
    name: Option<String>,
    email: Option<String>,
    password: Option<String>,
    current_password: Option<String>,
}

async fn update_me<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<UpdateMeRequest>,
) -> Result<Json<UserBody>, ApiError> {
    let mut fields = Fields::default();
    let name = body
        .name
        .map(|name| fields.check("name", clean_name(&name)));
    let email = body
        .email
        .map(|email| fields.check("email", clean_email(&email)));
    if let Some(password) = &body.password {
        fields.check("password", check_password(password));
    }
    let needs_password = email.is_some() || body.password.is_some();
    if needs_password && body.current_password.is_none() {
        fields.check::<()>("current_password", Err("required"));
    }
    fields.finish()?;

    let mut checked_hash = None;
    if let Some(password) = body.current_password.filter(|_| needs_password) {
        count_request(&state.password_guesses, &state.http, peer, &headers)?;
        checked_hash = state.store.password_hash(&current.user.id).await?;
        if !state
            .passwords
            .verify(password, checked_hash.clone())
            .await?
        {
            return Err(ApiError::new(ErrorCode::ValidationFailed)
                .with_message("Wrong current password")
                .with_field("current_password", "wrong"));
        }
    }
    let password_hash = match body.password {
        Some(password) => Some(state.passwords.hash(password).await?),
        None => None,
    };

    let changes = UserChanges {
        name,
        email,
        password_hash,
    };
    let keep = current.token_hash();
    let basis = UpdateBasis {
        session: &keep,
        checked_hash: checked_hash.as_deref(),
    };
    let user = current.user;
    state
        .store
        .update_user(&user.id, &changes, &basis, now_ms())
        .await?;
    Ok(Json(UserBody {
        name: changes.name.unwrap_or(user.name),
        email: changes.email.unwrap_or(user.email),
        id: user.id,
        is_instance_admin: user.is_instance_admin,
    }))
}

async fn delete_me<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
) -> Result<Response, ApiError> {
    state.store.delete_user(&current.user.id).await?;
    Ok(session_ended(&state))
}

/// Collects every refused field, so that a form can show them all at once.
#[derive(Default)]
struct Fields(Option<ApiError>);

impl Fields {
    fn check<T: Default>(&mut self, field: &'static str, value: Result<T, &'static str>) -> T {
        value.unwrap_or_else(|problem| {
            let error = self.0.take();
            let error = error.unwrap_or_else(|| ApiError::new(ErrorCode::ValidationFailed));
            self.0 = Some(error.with_field(field, problem));
            T::default()
        })
    }

    fn finish(self) -> Result<(), ApiError> {
        self.0.map_or(Ok(()), Err)
    }
}

fn clean_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        Err("required")
    } else if name.chars().count() > MAX_NAME_CHARS {
        Err("too_long")
    } else {
        Ok(name.to_owned())
    }
}

/// An email is an identifier only, nothing is sent to it (ADR 0013): this catches typing mistakes.
fn clean_email(email: &str) -> Result<String, &'static str> {
    let email = email.trim().to_lowercase();
    if email.is_empty() {
        return Err("required");
    }
    if email.len() > MAX_EMAIL_BYTES {
        return Err("too_long");
    }
    let parts = email.split_once('@');
    let valid = parts.is_some_and(|(local, domain)| {
        !local.is_empty() && !domain.is_empty() && !domain.contains('@')
    });
    if !valid || email.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err("invalid");
    }
    Ok(email)
}

fn check_password(password: &str) -> Result<(), &'static str> {
    let length = password.chars().count();
    if length < MIN_PASSWORD_CHARS {
        Err("too_short")
    } else if length > MAX_PASSWORD_CHARS {
        Err("too_long")
    } else {
        Ok(())
    }
}

/// Runs of anything but ASCII letters and digits become one dash.
fn slug(name: &str) -> String {
    let lowered = name.to_ascii_lowercase();
    let words = lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty());
    let slug = words.collect::<Vec<_>>().join("-");
    let slug: String = slug.chars().take(MAX_SLUG_CHARS).collect();
    match slug.trim_end_matches('-') {
        "" => "workspace".to_owned(),
        slug => slug.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_keeps_ascii_letters_and_digits() {
        assert_eq!(slug("Acme Docs"), "acme-docs");
        assert_eq!(slug("  R&D -- 2026! "), "r-d-2026");
        assert_eq!(slug("Équipe"), "quipe");
        assert_eq!(slug("日本語"), "workspace");
        assert_eq!(slug(&"a".repeat(200)).len(), MAX_SLUG_CHARS);
        assert_eq!(slug(&format!("{}-b", "a".repeat(63))), "a".repeat(63));
    }

    #[test]
    fn an_email_is_trimmed_lowered_and_checked() {
        assert_eq!(clean_email(" Ada@Example.ORG ").unwrap(), "ada@example.org");
        assert_eq!(clean_email("").unwrap_err(), "required");
        for email in ["ada", "@example.org", "ada@", "a@b@c", "a b@c.d"] {
            assert_eq!(clean_email(email).unwrap_err(), "invalid", "{email}");
        }
        let long = format!("{}@example.org", "a".repeat(250));
        assert_eq!(clean_email(&long).unwrap_err(), "too_long");
    }

    #[test]
    fn a_name_has_a_length() {
        assert_eq!(clean_name("  Ada ").unwrap(), "Ada");
        assert_eq!(clean_name("   ").unwrap_err(), "required");
        assert_eq!(clean_name(&"é".repeat(100)).unwrap().chars().count(), 100);
        assert_eq!(clean_name(&"é".repeat(101)).unwrap_err(), "too_long");
    }

    #[test]
    fn a_password_has_a_length_in_characters() {
        assert!(check_password("12345678").is_ok());
        assert_eq!(check_password("1234567").unwrap_err(), "too_short");
        assert_eq!(check_password("ééé").unwrap_err(), "too_short");
        assert_eq!(check_password(&"x".repeat(1025)).unwrap_err(), "too_long");
    }
}
