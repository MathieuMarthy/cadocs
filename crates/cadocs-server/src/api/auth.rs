//! Accounts and sessions: first run, login, logout, `/session` and `/me`, and what an instance
//! with a relay adds (ADR 0026): the reset link by mail, registration, a second factor and the
//! proof of a new address. And a second factor from an authenticator app, which needs no relay.

use std::net::SocketAddr;
use std::time::Duration;

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::header::{CACHE_CONTROL, SET_COOKIE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, patch, post, put};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_core::permissions::{InstanceAction, instance_allows, instance_role};
use cadocs_mail::{Lang, Mail, Outbox, Text};
use cadocs_store::{
    AccountStore, CodeAttempt, CodePurpose, DocStore, FirstUser, MAX_SLUG_CHARS, Membership,
    NewEmailCode, NewSession, NewUser, StoreError, TotpProof, UpdateBasis, User, UserChanges,
    now_ms,
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
use crate::totp;

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

/// Each account takes this many guesses at the code of its app at once, then one per
/// `TOTP_REFILL`, from any client: about 1,440 a day, a chance in 230 to hit one of the 3 codes a
/// window accepts. Only someone who knows the password gets to guess.
pub const TOTP_BURST: u32 = 5;
pub const TOTP_REFILL: Duration = Duration::from_secs(60);

/// Far above any of these bodies. Routes before a session must not buffer the default 2 MB.
pub(super) const MAX_BODY_BYTES: usize = 16 * 1024;
pub(super) const MAX_NAME_CHARS: usize = 100;
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
        .route("/me", patch(update_me).delete(delete_me))
        // The app needs no relay.
        .route("/me/totp", post(start_totp).delete(disable_totp))
        .route("/me/totp/confirm", post(enable_totp))
        .route("/me/totp/recovery-codes", post(new_recovery_codes));
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
            .route("/me/email-notifications", put(set_email_notifications))
            .route("/me/email-2fa/confirm", post(enable_email_2fa))
            .route("/me/email/confirm", post(confirm_email));
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
    email_notifications: bool,
    totp: bool,
    language: Option<String>,
}

impl From<User> for UserBody {
    fn from(user: User) -> Self {
        Self {
            id: user.id,
            email: user.email,
            name: user.name,
            is_instance_admin: user.is_instance_admin,
            email_2fa: user.email_2fa,
            email_notifications: user.email_notifications,
            totp: user.totp,
            language: user.language,
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
    /// The ids of the OIDC providers, for the login buttons. Left out when there are none.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    oidc: Vec<String>,
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
        oidc: state.oidc.iter().map(|p| p.id.clone()).collect(),
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
        email_notifications: false,
        totp: false,
        language: None,
        browser_language: None,
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
    /// A code from the authenticator app, or a recovery code.
    #[serde(default)]
    totp: String,
}

/// An unknown email, a wrong password and a disabled account answer alike, and in the same time.
/// Past the password, an account with the second factor on needs the code mailed to it: without
/// one, the answer mails it and names `code` as `required`. The app, when on, takes the place of
/// the code by mail, and is asked for as `totp`.
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
    let created = if let Some(secret) = &found.totp_secret {
        let given = totp::clean_code(&body.totp);
        if given.is_empty() {
            return Err(totp_problem("required"));
        }
        if let Err(wait) = state.totp_guesses.check(&found.user.id) {
            return Err(ApiError::rate_limited(wait));
        }
        let recovery;
        let proof = if totp::is_app_code(&given) {
            let now = totp::step_at(session.now);
            let Some(step) = totp::matching_step(secret, &given, now) else {
                return Err(totp_problem("wrong"));
            };
            TotpProof::App { secret, step }
        } else {
            recovery = totp::recovery_hash(&given);
            TotpProof::Recovery {
                code_hash: &recovery,
            }
        };
        let store = &state.store;
        match store
            .create_session_with_totp(&session, checked, &proof)
            .await
        {
            Err(StoreError::WrongCode) => return Err(totp_problem("wrong")),
            created => created?,
        }
    } else if found.needs_code(state.mail.is_some()) {
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
        let mail = state.mail.is_some();
        state.store.create_session(&session, checked, mail).await?
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

fn totp_problem(problem: &'static str) -> ApiError {
    let error = ApiError::new(ErrorCode::ValidationFailed).with_field("totp", problem);
    match problem {
        "required" => error.with_message("Type a code from your authenticator app"),
        _ => error.with_message("Wrong code"),
    }
}

fn code_problem(problem: &'static str) -> ApiError {
    let error = ApiError::new(ErrorCode::ValidationFailed).with_field("code", problem);
    match problem {
        "required" => error.with_message("Type the code mailed to you"),
        _ => error,
    }
}

/// The language to mail user `user_id` in (step 62). English when it cannot be read: it is read
/// after the change a message tells of, and a message in English beats none.
async fn mail_lang<S: AccountStore>(store: &S, user_id: &str) -> Lang {
    match store.mail_language(user_id).await {
        Ok(code) => Lang::of(code.as_deref()),
        Err(error) => {
            tracing::warn!(%error, "cannot read the language of mail, English it is");
            Lang::En
        }
    }
}

/// The language of a request's `Accept-Language` that mail is written in, if any.
pub(crate) fn browser_lang(headers: &HeaderMap) -> Option<Lang> {
    let header = headers
        .get(axum::http::header::ACCEPT_LANGUAGE)?
        .to_str()
        .ok()?;
    Lang::from_accept_language(header)
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
    let lang = mail_lang(state.store.as_ref(), &user.id).await;
    let (subject, text) = match purpose {
        CodePurpose::Login => (Text::LoginCodeSubject, Text::LoginCode),
        _ => (Text::CodeSubject, Text::SecondFactorCode),
    };
    place.send(Mail {
        to: user.email.clone(),
        subject: subject.get(lang).to_owned(),
        text: text.fill(lang, &[("origin", origin), ("code", &code)]),
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
    #[serde(default, deserialize_with = "super::spaces::present")]
    language: Option<Option<String>>,
}

/// The languages the interface comes in (step 34). `None` follows the browser's.
const LANGUAGES: [&str; 2] = ["en", "fr"];

fn interface_language(language: &str) -> Result<String, &'static str> {
    LANGUAGES
        .contains(&language)
        .then(|| language.to_owned())
        .ok_or("invalid")
}

/// With a relay, a new email is not changed here: a code goes to it, and `confirm_email` changes
/// it (ADR 0026). The answer is then the same, in the same time, whether the address is free.
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
    let language = body.language.map(|language| {
        language.map(|language| fields.check("language", interface_language(&language)))
    });
    let needs_password = email.is_some() || body.password.is_some();
    if needs_password && body.current_password.is_none() {
        fields.check::<()>("current_password", Err("required"));
    }
    fields.finish()?;

    let client = (peer, &headers);
    let user = current.user.clone();
    let email = email.filter(|email| *email != user.email);
    // With mail on, the new address waits for its code.
    let (email, to_prove) = match (&state.mail, email) {
        (Some(outbox), Some(email)) => {
            count_request(&state.mail_requests, &state.http, peer, &headers)?;
            (None, Some((outbox.clone(), email)))
        }
        (_, email) => (email, None),
    };

    let mut checked_hash = None;
    if let Some(password) = body.current_password.filter(|_| needs_password) {
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
        totp_off: false,
        language,
    };
    let keep = current.token_hash();
    let basis = UpdateBasis {
        session: &keep,
        checked_hash: checked_hash.as_deref(),
    };
    state
        .store
        .update_user(&user.id, &changes, &basis, now_ms())
        .await?;
    if changes.password_hash.is_some() {
        state.access_changed();
    }
    // Past the limit of the address the answer stays the same.
    if let Some((outbox, email)) = to_prove.filter(|(_, email)| state.mail_addresses.check(email)) {
        let (store, http, key) = (
            state.store.clone(),
            state.http.clone(),
            state.code_key.clone(),
        );
        let user_id = user.id.clone();
        tokio::spawn(async move {
            let mailed = mail_new_address(store.as_ref(), &http, &key, &outbox, user_id, email);
            if let Err(error) = mailed.await {
                tracing::warn!("a code for a new address could not be mailed, {error}");
            }
        });
    }
    // An address changed without a code is one nobody proved (`update_user`).
    let unproven = changes.email.is_some();
    Ok(Json(UserBody {
        name: changes.name.unwrap_or(user.name),
        email: changes.email.unwrap_or(user.email),
        id: user.id,
        is_instance_admin: user.is_instance_admin,
        email_2fa: user.email_2fa && !unproven,
        email_notifications: user.email_notifications,
        totp: user.totp,
        language: changes.language.unwrap_or(user.language),
    }))
}

/// Mails a code that moves the account `user_id` to `email`, or tells the owner of the address
/// that it has an account, as registration does.
async fn mail_new_address<S: AccountStore>(
    store: &S,
    http: &HttpConfig,
    key: &[u8; 32],
    outbox: &Outbox,
    user_id: String,
    email: String,
) -> anyhow::Result<()> {
    let origin = http.public_url.origin();
    match store.credentials(&email).await? {
        None => {}
        Some(found) if found.disabled => return Ok(()),
        Some(found) => {
            let lang = mail_lang(store, &found.user.id).await;
            outbox.queue(Mail {
                to: found.user.email,
                subject: Text::AccountSubject.get(lang).to_owned(),
                text: Text::MoveTaken.fill(lang, &[("origin", origin)]),
            });
            return Ok(());
        }
    }
    // Before the new code replaces the one before, as for a reset link.
    let Some(place) = outbox.reserve() else {
        return Ok(());
    };
    let lang = mail_lang(store, &user_id).await;
    let code = new_code()?;
    let now = now_ms();
    let purpose = CodePurpose::Email;
    store
        .create_email_code(&NewEmailCode {
            purpose,
            code_hash: code_hash(key, purpose, &email, &code),
            email: email.clone(),
            user_id: Some(user_id),
            now,
            expires_at: now + CODE_LIFETIME.as_millis() as i64,
        })
        .await?;
    place.send(Mail {
        to: email,
        subject: Text::CodeSubject.get(lang).to_owned(),
        text: Text::MoveCode.fill(lang, &[("origin", origin), ("code", &code)]),
    });
    Ok(())
}

#[derive(Deserialize)]
struct ConfirmEmailRequest {
    #[serde(default)]
    email: String,
    #[serde(default)]
    code: String,
}

/// Moves the account to the address a code went to, which the code proves.
async fn confirm_email<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<ConfirmEmailRequest>,
) -> Result<Json<UserBody>, ApiError> {
    let mut fields = Fields::default();
    let email = fields.check("email", clean_email(&body.email));
    let code = body.code.trim();
    if code.is_empty() {
        fields.check::<()>("code", Err("required"));
    }
    fields.finish()?;
    // A guess at a code, as at login.
    count_request(&state.password_guesses, &state.http, peer, &headers)?;
    let user = current.user;
    // The code that turns the factor on went to this address: it moves nothing.
    if email == user.email {
        return Err(code_problem("wrong"));
    }
    let purpose = CodePurpose::Email;
    let hash = code_hash(&state.code_key, purpose, &email, code);
    let attempt = CodeAttempt {
        purpose,
        email: &email,
        code_hash: &hash,
        now: now_ms(),
    };
    state.store.change_email(&user.id, &attempt).await?;
    // The owner may not be the one who moved it: the old address hears of it.
    if let Some(outbox) = &state.mail {
        let origin = state.http.public_url.origin();
        let lang = mail_lang(state.store.as_ref(), &user.id).await;
        let values = [("origin", origin), ("email", email.as_str())];
        let queued = outbox.queue(Mail {
            to: user.email.clone(),
            subject: Text::AddressChangedSubject.get(lang).to_owned(),
            text: Text::AddressChanged.fill(lang, &values),
        });
        if !queued {
            tracing::warn!("the notice of an address change could not be queued");
        }
    }
    Ok(Json(UserBody {
        email,
        ..UserBody::from(user)
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
        .enable_email_2fa(&current.user.id, &attempt, &current.token_hash())
        .await?;
    // The other sessions are gone: their sockets close.
    state.access_changed();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct DisableRequest {
    current_password: Option<String>,
}

#[derive(Deserialize)]
struct NotificationsRequest {
    enabled: bool,
}

/// Mail of notifications (step 42): only a relay registers this route.
async fn set_email_notifications<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    JsonBody(body): JsonBody<NotificationsRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .set_email_notifications(&current.user.id, body.enabled)
        .await?;
    Ok(StatusCode::NO_CONTENT)
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
        language: None,
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
pub(super) struct PasswordRequest {
    pub(super) current_password: Option<String>,
}

/// Checks the password a change needs, as a guess, and returns the basis of that change.
pub(super) async fn require_password<S: AccountStore, A>(
    state: &AppState<S, A>,
    current: &CurrentSession,
    client: (SocketAddr, &HeaderMap),
    password: Option<String>,
) -> Result<Option<String>, ApiError> {
    match password {
        Some(password) => check_current_password(state, current, client, password).await,
        None => {
            Err(ApiError::new(ErrorCode::ValidationFailed)
                .with_field("current_password", "required"))
        }
    }
}

/// How long after its login a session stands for the password of an account that has none.
const FRESH_SESSION_MS: i64 = 10 * 60 * 1000;

/// As `require_password`, but an account with no password (made by a provider) proves itself by
/// a session opened in the last 10 minutes instead: older is a `400` with `session: too_old`,
/// and the client offers to log in again.
pub(super) async fn require_password_or_fresh_session<S: AccountStore, A>(
    state: &AppState<S, A>,
    current: &CurrentSession,
    client: (SocketAddr, &HeaderMap),
    password: Option<String>,
) -> Result<Option<String>, ApiError> {
    if state.store.password_hash(&current.user.id).await?.is_some() {
        return require_password(state, current, client, password).await;
    }
    if now_ms() - current.created_at > FRESH_SESSION_MS {
        return Err(ApiError::new(ErrorCode::ValidationFailed)
            .with_message("Log in again first")
            .with_field("session", "too_old"));
    }
    Ok(None)
}

#[derive(Serialize)]
struct TotpSetup {
    /// Base32, for an app that takes it typed.
    secret: String,
    /// `otpauth://`, for a QR code.
    uri: String,
}

/// Starts the setup of an authenticator app: a new secret, which counts once a code from it
/// comes back. It needs the password, or a stolen session could tie the account to its own app.
async fn start_totp<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<PasswordRequest>,
) -> Result<Response, ApiError> {
    let client = (peer, &headers);
    let checked_hash = require_password(&state, &current, client, body.current_password).await?;
    let secret = totp::new_secret().map_err(ApiError::internal)?;
    let keep = current.token_hash();
    let basis = UpdateBasis {
        session: &keep,
        checked_hash: checked_hash.as_deref(),
    };
    let user = &current.user;
    state
        .store
        .start_totp(&user.id, &secret, &basis, now_ms())
        .await?;
    let body = TotpSetup {
        secret: totp::encode_secret(&secret),
        uri: totp::uri(&secret, state.http.public_url.host(), &user.email),
    };
    let no_store = [(CACHE_CONTROL, HeaderValue::from_static("no-store"))];
    Ok((no_store, Json(body)).into_response())
}

#[derive(Serialize)]
struct RecoveryCodes {
    recovery_codes: Vec<String>,
}

/// New recovery codes as an answer, never cached, and the hashes to store.
fn recovery_codes() -> Result<(Response, Vec<String>), ApiError> {
    let codes = totp::new_recovery_codes().map_err(ApiError::internal)?;
    let hashes = codes.iter().map(|code| totp::recovery_hash(code)).collect();
    let no_store = [(CACHE_CONTROL, HeaderValue::from_static("no-store"))];
    let body = RecoveryCodes {
        recovery_codes: codes,
    };
    Ok(((no_store, Json(body)).into_response(), hashes))
}

/// Turns the app on with a first code from it, which proves it holds the secret, and answers
/// the recovery codes, shown this once.
async fn enable_totp<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<EnableRequest>,
) -> Result<Response, ApiError> {
    let code = totp::clean_code(&body.code);
    if code.is_empty() {
        return Err(code_problem("required"));
    }
    // As for the other codes, though this one guards nothing the session does not have already.
    count_request(&state.password_guesses, &state.http, peer, &headers)?;
    let user_id = &current.user.id;
    let Some(secret) = state.store.pending_totp(user_id).await? else {
        return Err(StoreError::WrongCode.into());
    };
    let now = now_ms();
    let step = totp::is_app_code(&code)
        .then(|| totp::matching_step(&secret, &code, totp::step_at(now)))
        .flatten()
        .ok_or(StoreError::WrongCode)?;
    let (response, hashes) = recovery_codes()?;
    state
        .store
        .enable_totp(user_id, &secret, step, &hashes, &current.token_hash(), now)
        .await?;
    // The other sessions are gone: their sockets close.
    state.access_changed();
    Ok(response)
}

/// Replaces the recovery codes, as when they run out or might have leaked.
async fn new_recovery_codes<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<PasswordRequest>,
) -> Result<Response, ApiError> {
    let client = (peer, &headers);
    let checked_hash = require_password(&state, &current, client, body.current_password).await?;
    let (response, hashes) = recovery_codes()?;
    let keep = current.token_hash();
    let basis = UpdateBasis {
        session: &keep,
        checked_hash: checked_hash.as_deref(),
    };
    state
        .store
        .replace_recovery_codes(&current.user.id, &hashes, &basis)
        .await?;
    Ok(response)
}

/// Turns the app off, or ends a setup, with the password, as for the code by mail.
async fn disable_totp<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<PasswordRequest>,
) -> Result<StatusCode, ApiError> {
    let client = (peer, &headers);
    let checked_hash = require_password(&state, &current, client, body.current_password).await?;
    let changes = UserChanges {
        totp_off: true,
        language: None,
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
    let lang = mail_lang(store, &found.user.id).await;
    place.send(Mail {
        to: found.user.email,
        subject: Text::ResetSubject.get(lang).to_owned(),
        text: Text::Reset.fill(lang, &[("origin", origin), ("link", link.as_str())]),
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
    headers: HeaderMap,
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
            // An address with no account has no language yet but the browser's that asked.
            let lang = browser_lang(&headers).unwrap_or(Lang::En);
            let mailed = mail_registration(store.as_ref(), &http, &key, &outbox, email, lang);
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
    lang: Lang,
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
            let lang = mail_lang(store, &found.user.id).await;
            outbox.queue(Mail {
                to: found.user.email,
                subject: Text::AccountSubject.get(lang).to_owned(),
                text: Text::RegisterTaken.fill(lang, &[("origin", origin)]),
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
        subject: Text::CodeSubject.get(lang).to_owned(),
        text: Text::RegisterCode.fill(lang, &[("origin", origin), ("code", &code)]),
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
        email_notifications: false,
        totp: false,
        language: None,
        browser_language: None,
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

/// A stolen session must not delete the account: it needs the password, or a fresh session.
async fn delete_me<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    JsonBody(body): JsonBody<PasswordRequest>,
) -> Result<Response, ApiError> {
    let client = (peer, &headers);
    let checked_hash =
        require_password_or_fresh_session(&state, &current, client, body.current_password).await?;
    let keep = current.token_hash();
    let basis = UpdateBasis {
        session: &keep,
        checked_hash: checked_hash.as_deref(),
    };
    state
        .store
        .delete_user(&current.user.id, Some(&basis))
        .await?;
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
