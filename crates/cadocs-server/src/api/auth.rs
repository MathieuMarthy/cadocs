//! Accounts and sessions: first run, login, logout, `/session` and `/me`, and what an instance
//! with a relay adds (ADR 0026): the reset link by mail, registration and a second factor.

use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::header::{CACHE_CONTROL, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_core::permissions::{InstanceAction, instance_allows, instance_role};
use cadocs_mail::{Mail, Outbox};
use cadocs_store::{
    AccountStore, CodeAttempt, CodePurpose, DocStore, FirstUser, MAX_SLUG_CHARS, Membership,
    NewEmailCode, NewSession, NewUser, StoreError, UpdateBasis, User, UserChanges, now_ms,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{ApiError, ErrorCode, JsonBody, PathParam, count_request, rate_limit};
use crate::AppState;
use crate::config::{HttpConfig, Registration};
use crate::session::{
    self, CODE_LIFETIME, CurrentSession, check_setup_code, code_hash, csrf_token, new_code,
    new_password_reset, new_token, token_hash,
};

/// Each client can guess this many passwords at once, then one per `GUESS_REFILL`. Setup, login,
/// using a reset link and a `PATCH /me` that checks the current password share this budget.
pub const GUESS_BURST: u32 = 10;
pub const GUESS_REFILL: Duration = Duration::from_secs(15);

/// Each client can ask for this many messages at once, then one per `MAIL_REFILL`. Every route
/// that sends mail shares this budget (ADR 0026).
pub const MAIL_BURST: u32 = 5;
pub const MAIL_REFILL: Duration = Duration::from_secs(60);
/// Each address gets this many messages at once, then one per `ADDRESS_REFILL`, whoever asks.
pub const ADDRESS_BURST: u32 = 3;
pub const ADDRESS_REFILL: Duration = Duration::from_secs(10 * 60);

/// Far above any of these bodies. Routes before a session must not buffer the default 2 MB.
pub(super) const MAX_BODY_BYTES: usize = 16 * 1024;
const MAX_NAME_CHARS: usize = 100;
const MAX_EMAIL_BYTES: usize = 254;
const MIN_PASSWORD_CHARS: usize = 8;
const MAX_PASSWORD_CHARS: usize = 1024;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore,
    A: Authorizer,
{
    let limit = from_fn_with_state(
        (state.password_guesses.clone(), state.http.clone()),
        rate_limit,
    );
    let mut public = Router::new()
        .route(
            "/auth/setup",
            post(setup).route_layer(limit.clone()).get(setup_state),
        )
        .route("/auth/login", post(login).route_layer(limit.clone()))
        .route(
            "/auth/password-reset/{token}",
            post(use_password_reset).route_layer(limit.clone()),
        );
    // Not registered with mail off: the route then answers as any path that never was.
    if state.mail.is_some() {
        let sends = from_fn_with_state(
            (state.mail_requests.clone(), state.http.clone()),
            rate_limit,
        );
        public = public.route(
            "/auth/forgot-password",
            post(forgot_password).route_layer(sends.clone()),
        );
        if state.registration == Registration::Open {
            public = public
                .route("/auth/register", post(register).route_layer(sends))
                // It hashes a password, and each try is a guess at a code.
                .route(
                    "/auth/register/confirm",
                    post(confirm_registration).route_layer(limit),
                );
        }
    }
    let mut authenticated = Router::new()
        .route("/auth/logout", post(logout))
        .route("/session", get(current_session))
        .route("/me", patch(update_me).delete(delete_me));
    if state.mail.is_some() {
        let sends = from_fn_with_state(
            (state.mail_requests.clone(), state.http.clone()),
            rate_limit,
        );
        authenticated = authenticated
            .route(
                "/me/email-2fa",
                post(ask_email_2fa)
                    .route_layer(sends)
                    .merge(delete(disable_email_2fa)),
            )
            .route("/me/email-2fa/confirm", post(enable_email_2fa));
    }
    let authenticated =
        authenticated.route_layer(from_fn_with_state(state.clone(), session::require::<S, A>));
    public
        .merge(authenticated)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

#[derive(Serialize)]
pub(super) struct UserBody {
    id: String,
    email: String,
    name: String,
    is_instance_admin: bool,
    email_2fa: bool,
}

impl From<User> for UserBody {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            name: user.name,
            is_instance_admin: user.is_instance_admin,
            email_2fa: user.email_2fa,
        }
    }
}

#[derive(Serialize)]
pub(super) struct WorkspaceBody {
    id: String,
    slug: String,
    name: String,
    role: String,
}

impl From<Membership> for WorkspaceBody {
    fn from(membership: Membership) -> Self {
        Self {
            id: membership.workspace_id,
            slug: membership.slug,
            name: membership.name,
            role: membership.role,
        }
    }
}

#[derive(Serialize)]
struct SessionBody {
    user: UserBody,
    workspaces: Vec<WorkspaceBody>,
    /// Whether `POST /workspaces` would let this user through (ADR 0024).
    can_create_workspace: bool,
    csrf_token: String,
}

pub(super) fn session_body<S, A>(
    state: &AppState<S, A>,
    user: User,
    workspaces: Vec<Membership>,
    token: &str,
) -> Response {
    let role = instance_role(true, user.is_instance_admin);
    let creation = InstanceAction::CreateWorkspace(state.workspace_creation);
    let body = SessionBody {
        can_create_workspace: instance_allows(role, creation),
        user: user.into(),
        workspaces: workspaces.into_iter().map(Into::into).collect(),
        csrf_token: csrf_token(token),
    };
    let no_store = [(CACHE_CONTROL, HeaderValue::from_static("no-store"))];
    (no_store, Json(body)).into_response()
}

/// The answer of a route that makes an account: it is logged in from here on.
pub(super) fn new_account_body<S, A>(
    state: &AppState<S, A>,
    user: User,
    workspaces: Vec<Membership>,
    token: &str,
) -> Response {
    let mut response = session_body(state, user, workspaces, token);
    *response.status_mut() = StatusCode::CREATED;
    response
        .headers_mut()
        .insert(SET_COOKIE, session::cookie(token, &state.http));
    response
}

pub(super) fn new_session<S, A>(
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

#[derive(Serialize)]
struct SetupState {
    required: bool,
    /// Whether the instance sends mail, so the login screen can offer what needs it. Left out
    /// when it does not: an instance with no relay answers as it did before ADR 0026.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    mail: bool,
    /// Whether anyone can make an account, which needs mail. Left out when not.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    registration: bool,
}

/// Tells the front end to show first-run setup, since `GET /session` is a `401` either way.
async fn setup_state<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
) -> Result<Response, ApiError> {
    let required = !state.store.has_users().await?;
    let no_store = [(CACHE_CONTROL, HeaderValue::from_static("no-store"))];
    let mail = state.mail.is_some();
    // Before first-run setup, an account would take the instance from its operator.
    let registration = mail && state.registration == Registration::Open && !required;
    let body = SetupState {
        required,
        mail,
        registration,
    };
    Ok((no_store, Json(body)).into_response())
}

#[derive(Deserialize)]
struct SetupRequest {
    email: String,
    name: String,
    password: String,
    workspace_name: String,
    /// From the link in the server's log (ADR 0021). Missing reads as empty, so the form can
    /// name it like any other field.
    #[serde(default)]
    code: String,
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
    fields.check("code", check_setup_code(&body.code, &state.setup_code));
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
        email_2fa: false,
    };
    let first = FirstUser {
        id: user.id.clone(),
        email: user.email.clone(),
        name: user.name.clone(),
        password_hash: state.passwords.hash(body.password).await?,
        workspace_id: Uuid::now_v7().to_string(),
        workspace_slug: slug(&workspace_name, "workspace"),
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
    Ok(new_account_body(&state, user, vec![workspace], &token))
}

#[derive(Deserialize)]
struct LoginRequest {
    email: String,
    password: String,
    /// The code mailed for the second factor (ADR 0026).
    #[serde(default)]
    code: String,
}

/// An unknown email, a wrong password and a disabled account answer alike, and in the same time.
/// Past the password, an account with the second factor on needs the code mailed to it: without
/// one, the answer mails it and names `code` as `required`.
async fn login<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<LoginRequest>,
) -> Result<Response, ApiError> {
    let email = normalize_email(&body.email);
    let found = state.store.credentials(&email).await?;
    let hash = found.as_ref().and_then(|found| found.password_hash.clone());
    let matches = state.passwords.verify(body.password, hash).await?;
    let Some(found) = found.filter(|found| matches && !found.disabled) else {
        return Err(wrong_credentials());
    };

    let workspaces = state.store.memberships(&found.user.id).await?;
    let (token, session) = new_session(&state, &found.user.id)?;
    let checked = found.password_hash.as_deref().unwrap_or_default();
    let created = if found.needs_code(state.mail.is_some()) {
        let code = body.code.trim();
        if code.is_empty() {
            count_request(&state.mail_requests, &state.http, peer, &headers)?;
            mail_code(&state, CodePurpose::Login, &found.user).await?;
            return Err(code_problem("required"));
        }
        let purpose = CodePurpose::Login;
        let hash = code_hash(&state.code_key, purpose, &found.user.email, code);
        let attempt = CodeAttempt {
            purpose,
            email: &found.user.email,
            code_hash: &hash,
            now: session.now,
        };
        let store = &state.store;
        store
            .create_session_with_code(&session, checked, &attempt)
            .await?
    } else {
        state.store.create_session(&session, checked).await?
    };
    if !created {
        return Err(wrong_credentials());
    }
    let mut response = session_body(&state, found.user, workspaces, &token);
    response
        .headers_mut()
        .insert(SET_COOKIE, session::cookie(&token, &state.http));
    Ok(response)
}

fn wrong_credentials() -> ApiError {
    ApiError::new(ErrorCode::Unauthenticated).with_message("Wrong email or password")
}

fn code_problem(problem: &'static str) -> ApiError {
    let error = ApiError::new(ErrorCode::ValidationFailed).with_field("code", problem);
    match problem {
        "required" => error.with_message("Type the code mailed to you"),
        _ => error,
    }
}

/// Mails a code to the address of `user`, in place of the one it had for `purpose`. Silent past
/// the limit of the address, or with the outbox full: the user asks again.
async fn mail_code<S: AccountStore, A>(
    state: &AppState<S, A>,
    purpose: CodePurpose,
    user: &User,
) -> Result<(), ApiError> {
    let Some(outbox) = &state.mail else {
        return Err(ApiError::new(ErrorCode::NotFound));
    };
    if !state.mail_addresses.check(&user.email) {
        return Ok(());
    }
    // Before the new code replaces the one before, as for a reset link.
    let Some(place) = outbox.reserve() else {
        return Ok(());
    };
    let code = new_code().map_err(ApiError::internal)?;
    let now = now_ms();
    state
        .store
        .create_email_code(&NewEmailCode {
            purpose,
            code_hash: code_hash(&state.code_key, purpose, &user.email, &code),
            email: user.email.clone(),
            user_id: Some(user.id.clone()),
            now,
            expires_at: now + CODE_LIFETIME.as_millis() as i64,
        })
        .await?;
    let origin = state.http.public_url.origin();
    let (subject, what, warning) = match purpose {
        CodePurpose::Login => (
            "Your cadocs login code",
            "log in to",
            "If you did not just log in, someone knows your password: change\n\
             it.",
        ),
        _ => (
            "Your cadocs code",
            "turn on the second factor of your account on",
            "If you did not ask for it, ignore this message: nothing changes\n\
             without the code.",
        ),
    };
    place.send(Mail {
        to: user.email.clone(),
        subject: subject.to_owned(),
        text: format!(
            "Your code to {what}\n\
             {origin}\n\n\
             \x20   {code}\n\n\
             It works for 10 minutes.\n\
             {warning}\n"
        ),
    });
    Ok(())
}

async fn logout<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
) -> Result<Response, ApiError> {
    state.store.delete_session(&current.token_hash()).await?;
    state.access_changed();
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
    Ok(session_body(
        &state,
        current.user,
        workspaces,
        &current.token,
    ))
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
        let client = (peer, &headers);
        checked_hash = check_current_password(&state, &current, client, password).await?;
    }
    let password_hash = match body.password {
        Some(password) => Some(state.passwords.hash(password).await?),
        None => None,
    };

    let changes = UserChanges {
        name,
        email,
        password_hash,
        email_2fa_off: false,
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
    if changes.password_hash.is_some() {
        state.access_changed();
    }
    Ok(Json(UserBody {
        name: changes.name.unwrap_or(user.name),
        email: changes.email.unwrap_or(user.email),
        id: user.id,
        is_instance_admin: user.is_instance_admin,
        email_2fa: user.email_2fa,
    }))
}

/// Verifies the password of the session's user, as a guess of the client, and returns the hash it
/// was checked against, for `UpdateBasis`.
async fn check_current_password<S: AccountStore, A>(
    state: &AppState<S, A>,
    current: &CurrentSession,
    (peer, headers): (SocketAddr, &HeaderMap),
    password: String,
) -> Result<Option<String>, ApiError> {
    count_request(&state.password_guesses, &state.http, peer, headers)?;
    let checked_hash = state.store.password_hash(&current.user.id).await?;
    if !state
        .passwords
        .verify(password, checked_hash.clone())
        .await?
    {
        return Err(ApiError::new(ErrorCode::ValidationFailed)
            .with_message("Wrong current password")
            .with_field("current_password", "wrong"));
    }
    Ok(checked_hash)
}

/// Mails a code to the account's address, which turns the second factor on (ADR 0026): it
/// proves the address works before a login depends on it.
async fn ask_email_2fa<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
) -> Result<StatusCode, ApiError> {
    mail_code(&state, CodePurpose::Email, &current.user).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct EnableRequest {
    #[serde(default)]
    code: String,
}

async fn enable_email_2fa<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<EnableRequest>,
) -> Result<StatusCode, ApiError> {
    let code = body.code.trim();
    if code.is_empty() {
        return Err(code_problem("required"));
    }
    // A guess at a code, as at login.
    count_request(&state.password_guesses, &state.http, peer, &headers)?;
    let purpose = CodePurpose::Email;
    let email = &current.user.email;
    let hash = code_hash(&state.code_key, purpose, email, code);
    let attempt = CodeAttempt {
        purpose,
        email,
        code_hash: &hash,
        now: now_ms(),
    };
    state
        .store
        .enable_email_2fa(&current.user.id, &attempt)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DisableRequest {
    current_password: Option<String>,
}

/// A stolen session must not take the factor off: it needs the password.
async fn disable_email_2fa<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<DisableRequest>,
) -> Result<StatusCode, ApiError> {
    let Some(password) = body.current_password else {
        let mut fields = Fields::default();
        fields.check::<()>("current_password", Err("required"));
        return fields.finish().map(|()| StatusCode::NO_CONTENT);
    };
    let checked_hash = check_current_password(&state, &current, (peer, &headers), password).await?;
    let changes = UserChanges {
        email_2fa_off: true,
        ..UserChanges::default()
    };
    let keep = current.token_hash();
    let basis = UpdateBasis {
        session: &keep,
        checked_hash: checked_hash.as_deref(),
    };
    state
        .store
        .update_user(&current.user.id, &changes, &basis, now_ms())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ResetRequest {
    password: String,
}

/// Unknown, used, replaced and expired links all answer alike.
async fn use_password_reset<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    PathParam(token): PathParam,
    JsonBody(body): JsonBody<ResetRequest>,
) -> Result<StatusCode, ApiError> {
    let hash = token_hash(&token);
    if !state.store.password_reset_usable(&hash, now_ms()).await? {
        return Err(ApiError::new(ErrorCode::NotFound));
    }
    let mut fields = Fields::default();
    fields.check("password", check_password(&body.password));
    fields.finish()?;

    let password_hash = state.passwords.hash(body.password).await?;
    if !state
        .store
        .use_password_reset(&hash, &password_hash, now_ms())
        .await?
    {
        return Err(ApiError::new(ErrorCode::NotFound));
    }
    state.access_changed();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct ForgotRequest {
    email: String,
}

/// Mails a reset link to the account of an address, if there is one to mail. The answer is the
/// same for every address, and so is its time: the lookup, the link and the message all happen
/// after it, in a task of their own.
async fn forgot_password<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    JsonBody(body): JsonBody<ForgotRequest>,
) -> Result<StatusCode, ApiError> {
    let Some(outbox) = state.mail.clone() else {
        return Err(ApiError::new(ErrorCode::NotFound));
    };
    let mut fields = Fields::default();
    let email = fields.check("email", clean_email(&body.email));
    fields.finish()?;

    // Past the limit of an address the answer stays the same, or it would tell that someone
    // asked for it. Counted for every address, with or without an account.
    if state.mail_addresses.check(&email) {
        let (store, http) = (state.store.clone(), state.http.clone());
        tokio::spawn(async move {
            if let Err(error) = mail_reset(store.as_ref(), &http, &outbox, &email).await {
                tracing::warn!("a reset link could not be mailed, {error}");
            }
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn mail_reset<S: AccountStore>(
    store: &S,
    http: &HttpConfig,
    outbox: &Outbox,
    email: &str,
) -> anyhow::Result<()> {
    let Some(found) = store
        .credentials(email)
        .await?
        .filter(|found| !found.disabled)
    else {
        return Ok(());
    };
    // Before the new link replaces the one before: with no room for its message, the account
    // would be left with no link that works.
    let Some(place) = outbox.reserve() else {
        return Ok(());
    };
    let (link, reset) = new_password_reset(&http.public_url, &found.user.id, None)?;
    match store.create_password_reset(&reset).await {
        Ok(()) => {}
        // Deleted or disabled since the lookup.
        Err(StoreError::UserNotFound | StoreError::UserDisabled) => return Ok(()),
        Err(error) => return Err(error.into()),
    }
    let origin = http.public_url.origin();
    place.send(Mail {
        to: found.user.email,
        subject: "Reset your cadocs password".to_owned(),
        text: format!(
            "Someone asked to reset the password of your account on\n\
             {origin}\n\n\
             To choose a new password, open this link within one hour:\n\n\
             {link}\n\n\
             If you did not ask for this, ignore this message: your password\n\
             has not changed.\n"
        ),
    });
    Ok(())
}

#[derive(Deserialize)]
struct RegisterRequest {
    email: String,
}

/// Mails a code that makes an account, or tells the owner of the address that it has one. As
/// for a forgotten password, the answer and its time are the same for every address.
async fn register<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    JsonBody(body): JsonBody<RegisterRequest>,
) -> Result<StatusCode, ApiError> {
    let Some(outbox) = state.mail.clone() else {
        return Err(ApiError::new(ErrorCode::NotFound));
    };
    let mut fields = Fields::default();
    let email = fields.check("email", clean_email(&body.email));
    fields.finish()?;

    if state.mail_addresses.check(&email) {
        let (store, http, key) = (
            state.store.clone(),
            state.http.clone(),
            state.code_key.clone(),
        );
        tokio::spawn(async move {
            let mailed = mail_registration(store.as_ref(), &http, &key, &outbox, email);
            if let Err(error) = mailed.await {
                tracing::warn!("a registration code could not be mailed, {error}");
            }
        });
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn mail_registration<S: AccountStore>(
    store: &S,
    http: &HttpConfig,
    key: &[u8; 32],
    outbox: &Outbox,
    email: String,
) -> anyhow::Result<()> {
    // Before first-run setup, an account would take the instance from its operator (ADR 0021).
    if !store.has_users().await? {
        return Ok(());
    }
    let origin = http.public_url.origin();
    match store.credentials(&email).await? {
        None => {}
        Some(found) if found.disabled => return Ok(()),
        Some(found) => {
            outbox.queue(Mail {
                to: found.user.email,
                subject: "Your cadocs account".to_owned(),
                text: format!(
                    "Someone asked to create an account on\n\
                     {origin}\n\
                     with this address, which already has an account. Nothing has\n\
                     changed.\n\n\
                     To log in: {origin}/login\n\
                     If you forgot your password: {origin}/forgot-password\n"
                ),
            });
            return Ok(());
        }
    }
    // Before the new code replaces the one before, as for a reset link.
    let Some(place) = outbox.reserve() else {
        return Ok(());
    };
    let code = new_code()?;
    let now = now_ms();
    let purpose = CodePurpose::Register;
    store
        .create_email_code(&NewEmailCode {
            purpose,
            code_hash: code_hash(key, purpose, &email, &code),
            email: email.clone(),
            user_id: None,
            now,
            expires_at: now + CODE_LIFETIME.as_millis() as i64,
        })
        .await?;
    place.send(Mail {
        to: email,
        subject: "Your cadocs code".to_owned(),
        text: format!(
            "Your code to create an account on\n\
             {origin}\n\n\
             \x20   {code}\n\n\
             It works for 10 minutes. If you did not ask for it, ignore this\n\
             message: no account is made without the code.\n"
        ),
    });
    Ok(())
}

#[derive(Deserialize)]
struct ConfirmRequest {
    email: String,
    code: String,
    name: String,
    password: String,
}

/// Makes the account a mailed code was for. It belongs to no workspace (ADR 0026).
async fn confirm_registration<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    JsonBody(body): JsonBody<ConfirmRequest>,
) -> Result<Response, ApiError> {
    let mut fields = Fields::default();
    let email = fields.check("email", clean_email(&body.email));
    let code = body.code.trim();
    if code.is_empty() {
        fields.check::<()>("code", Err("required"));
    }
    let name = fields.check("name", clean_name(&body.name));
    fields.check("password", check_password(&body.password));
    fields.finish()?;

    let user = User {
        id: Uuid::now_v7().to_string(),
        email,
        name,
        is_instance_admin: false,
        email_2fa: false,
    };
    let new = NewUser {
        id: user.id.clone(),
        email: user.email.clone(),
        name: user.name.clone(),
        password_hash: state.passwords.hash(body.password).await?,
    };
    let purpose = CodePurpose::Register;
    let hash = code_hash(&state.code_key, purpose, &user.email, code);
    let (token, session) = new_session(&state, &user.id)?;
    let attempt = CodeAttempt {
        purpose,
        email: &user.email,
        code_hash: &hash,
        now: session.now,
    };
    state.store.register_user(&new, &attempt, &session).await?;
    Ok(new_account_body(&state, user, Vec::new(), &token))
}

async fn delete_me<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
) -> Result<Response, ApiError> {
    state.store.delete_user(&current.user.id).await?;
    state.access_changed();
    Ok(session_ended(&state))
}

/// Collects every refused field, so that a form can show them all at once.
#[derive(Default)]
pub(super) struct Fields(Option<ApiError>);

impl Fields {
    pub(super) fn check<T: Default>(
        &mut self,
        field: &'static str,
        value: Result<T, &'static str>,
    ) -> T {
        value.unwrap_or_else(|problem| {
            let error = self.0.take();
            let error = error.unwrap_or_else(|| ApiError::new(ErrorCode::ValidationFailed));
            self.0 = Some(error.with_field(field, problem));
            T::default()
        })
    }

    pub(super) fn finish(self) -> Result<(), ApiError> {
        self.0.map_or(Ok(()), Err)
    }
}

pub(super) fn clean_name(name: &str) -> Result<String, &'static str> {
    let name = name.trim();
    if name.is_empty() {
        Err("required")
    } else if name.chars().count() > MAX_NAME_CHARS {
        Err("too_long")
    } else {
        Ok(name.to_owned())
    }
}

/// The form an email is stored in, and looked up by.
pub fn normalize_email(email: &str) -> String {
    email.trim().to_lowercase()
}

/// Catches typing mistakes. Only a code sent to an address proves it (ADR 0026).
pub(super) fn clean_email(email: &str) -> Result<String, &'static str> {
    let email = normalize_email(email);
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

pub(super) fn check_password(password: &str) -> Result<(), &'static str> {
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
pub(super) fn slug(name: &str, fallback: &str) -> String {
    let lowered = name.to_ascii_lowercase();
    let words = lowered
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty());
    let slug = words.collect::<Vec<_>>().join("-");
    let slug: String = slug.chars().take(MAX_SLUG_CHARS).collect();
    match slug.trim_end_matches('-') {
        "" => fallback.to_owned(),
        slug => slug.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_keeps_ascii_letters_and_digits() {
        assert_eq!(slug("Acme Docs", "workspace"), "acme-docs");
        assert_eq!(slug("  R&D -- 2026! ", "workspace"), "r-d-2026");
        assert_eq!(slug("Équipe", "workspace"), "quipe");
        assert_eq!(slug("日本語", "workspace"), "workspace");
        assert_eq!(slug(&"a".repeat(200), "workspace").len(), MAX_SLUG_CHARS);
        assert_eq!(
            slug(&format!("{}-b", "a".repeat(63)), "workspace"),
            "a".repeat(63)
        );
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
