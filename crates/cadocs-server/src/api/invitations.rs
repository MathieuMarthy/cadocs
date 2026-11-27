//! Invitation links (ADR 0013): admins make them, and whoever holds one joins with it.

use std::time::Duration;

use axum::extract::{DefaultBodyLimit, RawQuery, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use cadocs_collab::{Authorizer, Event};
use cadocs_core::permissions::WorkspaceRole;
use cadocs_mail::{Lang, Mail, Text};
use cadocs_store::{
    AccountStore, DocStore, Invitation, InvitationPreview, InvitedUser, Invitee, Membership,
    PendingInvitation, StoreError, User, WorkspaceStore, now_ms,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::{
    Fields, MAX_BODY_BYTES, check_password, clean_email, clean_name, new_account_body, new_session,
    session_body,
};
use super::workspaces::invalid_role;
use super::{ApiError, ErrorCode, JsonBody, Page, PageQuery, PathParam, rate_limit};
use crate::AppState;
use crate::session::{self, CurrentSession, new_invitation, token_hash};

const NO_STORE: (axum::http::HeaderName, HeaderValue) =
    (CACHE_CONTROL, HeaderValue::from_static("no-store"));
/// People one search finds, and the characters it needs: fewer would list the whole instance.
const INVITEES: u32 = 10;
const MIN_QUERY_CHARS: usize = 3;
/// Each account searches this many times at once, then once per `SEARCH_REFILL`: enough to type
/// a few names, slow to go through the accounts of the instance.
pub const SEARCH_BURST: u32 = 30;
pub const SEARCH_REFILL: Duration = Duration::from_secs(1);

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + WorkspaceStore,
    A: Authorizer,
{
    let limit = from_fn_with_state(
        (state.password_guesses.clone(), state.http.clone()),
        rate_limit,
    );
    // Two routes on one path, so one name: the parameter is an id for `DELETE`, a token for `GET`.
    let public = Router::new()
        .route("/invitations/{token}", get(read))
        .route(
            "/invitations/{token}/accept",
            post(accept).route_layer(limit),
        );
    let authenticated = Router::new()
        .route("/workspaces/{ws}/invitations", get(list).post(create))
        .route("/workspaces/{ws}/invitees", get(invitees))
        .route("/invitations/{token}", delete(revoke))
        .route("/me/invitations", get(pending))
        .route("/me/invitations/{id}", delete(decline))
        .route("/me/invitations/{id}/accept", post(accept_pending))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>));
    public
        .merge(authenticated)
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

#[derive(Serialize)]
struct InvitationBody {
    id: String,
    email: String,
    role: &'static str,
    invited_by: String,
    expires_at: i64,
    created_at: i64,
}

impl From<Invitation> for InvitationBody {
    fn from(invitation: Invitation) -> Self {
        Self {
            id: invitation.id,
            email: invitation.email,
            role: invitation.role.as_str(),
            invited_by: invitation.invited_by,
            expires_at: invitation.expires_at,
            created_at: invitation.created_at,
        }
    }
}

async fn list<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    page: PageQuery,
) -> Result<Json<Page<InvitationBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let invitations = state
        .store
        .list_invitations(&workspace, &current.user.id, now_ms(), after, page.fetch())
        .await?;
    Ok(Json(page.cut(invitations, |row| row.id.clone())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    email: String,
    role: String,
}

/// `owner` is only ever granted to a member, by an owner.
fn invited_role(role: &str) -> Option<WorkspaceRole> {
    WorkspaceRole::parse(role).filter(|role| *role != WorkspaceRole::Owner)
}

#[derive(Serialize)]
struct CreatedBody {
    #[serde(flatten)]
    invitation: InvitationBody,
    link: String,
}

async fn create<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    JsonBody(body): JsonBody<CreateRequest>,
) -> Result<Response, ApiError> {
    let mut fields = Fields::default();
    let email = fields.check("email", clean_email(&body.email));
    fields.finish()?;
    let role = invited_role(&body.role).ok_or_else(invalid_role)?;

    let public_url = &state.http.public_url;
    let (link, new) = new_invitation(public_url, &workspace, email, role, &current.user.id)
        .map_err(ApiError::internal)?;
    let invitee = state.store.create_invitation(&new).await?;
    if let Some(user) = invitee {
        state
            .registry
            .publish_to_user(Event::invitation(&user, &new.id));
    }
    // Past the limit of the address, the admin still has the link to give by hand.
    if state.mail.is_some() && state.mail_addresses.check(&new.email) {
        let mailed = mail_invitation(&state, &workspace, &current.user, &new.email, &link).await;
        if let Err(error) = mailed {
            tracing::warn!("an invitation could not be mailed, {error}");
        }
    }
    let invitation = InvitationBody {
        id: new.id,
        email: new.email,
        role: new.role.as_str(),
        invited_by: new.invited_by,
        expires_at: new.expires_at,
        created_at: new.now,
    };
    let body = CreatedBody { invitation, link };
    Ok((StatusCode::CREATED, [NO_STORE], Json(body)).into_response())
}

/// In the language of the workspace, which the invited person is about to join (step 62).
async fn mail_invitation<S: WorkspaceStore, A>(
    state: &AppState<S, A>,
    workspace_id: &str,
    inviter: &User,
    email: &str,
    link: &str,
) -> Result<(), StoreError> {
    let Some(outbox) = &state.mail else {
        return Ok(());
    };
    // Gone since, or the inviter with it: the link died too.
    let Some(workspace) = state.store.workspace(workspace_id, &inviter.id).await? else {
        return Ok(());
    };
    let lang = Lang::of(Some(&workspace.language));
    let values = [
        ("who", inviter.name.as_str()),
        ("workspace", workspace.membership.name.as_str()),
        ("origin", state.http.public_url.origin()),
        ("link", link),
    ];
    outbox.queue(Mail {
        to: email.to_owned(),
        subject: Text::InvitationSubject.get(lang).to_owned(),
        text: Text::Invitation.fill(lang, &values),
    });
    Ok(())
}

async fn revoke<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(invitation): PathParam,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .revoke_invitation(&invitation, &current.user.id)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct PreviewBody {
    workspace_id: String,
    workspace_name: String,
    email: String,
    role: &'static str,
    expires_at: i64,
    has_account: bool,
}

impl From<InvitationPreview> for PreviewBody {
    fn from(preview: InvitationPreview) -> Self {
        Self {
            workspace_id: preview.workspace_id,
            workspace_name: preview.workspace_name,
            email: preview.email,
            role: preview.role.as_str(),
            expires_at: preview.expires_at,
            has_account: preview.has_account,
        }
    }
}

async fn usable<S: WorkspaceStore, A>(
    state: &AppState<S, A>,
    hash: &str,
) -> Result<InvitationPreview, ApiError> {
    let found = state.store.invitation(hash, now_ms()).await?;
    found.ok_or_else(|| ApiError::new(ErrorCode::NotFound))
}

/// Unknown, used, replaced, revoked and expired links all answer alike.
async fn read<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    PathParam(token): PathParam,
) -> Result<Response, ApiError> {
    let preview = usable(&state, &token_hash(&token)).await?;
    let body = PreviewBody::from(preview);
    Ok(([NO_STORE], Json(body)).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AcceptRequest {
    name: Option<String>,
    password: Option<String>,
}

/// With a name or a password this makes the account, otherwise the session of the invited account
/// joins. Either way a JSON body is needed: a cross-site form cannot send one.
async fn accept<S: AccountStore + WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    PathParam(token): PathParam,
    headers: HeaderMap,
    JsonBody(body): JsonBody<AcceptRequest>,
) -> Result<Response, ApiError> {
    let hash = token_hash(&token);
    let invitation = usable(&state, &hash).await?;
    if body.name.is_none() && body.password.is_none() {
        let current = session::require_in_handler(&state, &headers).await?;
        return join(&state, &hash, current).await;
    }
    if invitation.has_account {
        return Err(StoreError::EmailTaken.into());
    }

    let mut fields = Fields::default();
    let name = body.name.as_deref().ok_or("required");
    let name = fields.check("name", name.and_then(clean_name));
    let password = body.password.ok_or("required");
    let checked = password.as_deref().map_err(|problem| *problem);
    fields.check("password", checked.and_then(check_password));
    fields.finish()?;
    let password = password.unwrap_or_default();

    let invited = InvitedUser {
        id: Uuid::now_v7().to_string(),
        name,
        password_hash: state.passwords.hash(password).await?,
    };
    let (session_token, session) = new_session(&state, &invited.id)?;
    let workspaces = state
        .store
        .accept_as_new_user(&hash, &invited, &session)
        .await?;
    let user = User {
        id: invited.id,
        email: invitation.email,
        name: invited.name,
        is_instance_admin: false,
        email_2fa: false,
        email_notifications: false,
        totp: false,
        language: None,
        browser_language: None,
    };
    Ok(new_account_body(&state, user, workspaces, &session_token))
}

async fn join<S: WorkspaceStore, A>(
    state: &AppState<S, A>,
    hash: &str,
    current: CurrentSession,
) -> Result<Response, ApiError> {
    let id = &current.user.id;
    let workspaces = state.store.accept_as_user(hash, id, now_ms()).await?;
    Ok(joined(state, current, workspaces))
}

fn joined<S, A>(
    state: &AppState<S, A>,
    current: CurrentSession,
    workspaces: Vec<Membership>,
) -> Response {
    state.access_changed();
    session_body(state, current.user, workspaces, &current.token)
}

#[derive(Serialize)]
struct InviteeBody {
    id: String,
    name: String,
    email: String,
}

#[derive(Serialize)]
struct Invitees {
    items: Vec<InviteeBody>,
}

/// Accounts of the whole instance an admin may invite, an accepted risk (`OPERATIONS.md`).
async fn invitees<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    RawQuery(query): RawQuery,
) -> Result<Json<Invitees>, ApiError> {
    let q = super::search::parse(query.as_deref())?.q;
    let q = q.trim();
    if q.chars().count() < MIN_QUERY_CHARS {
        return Err(ApiError::new(ErrorCode::ValidationFailed)
            .with_message("A search needs 3 characters at least")
            .with_field("q", "too_short"));
    }
    if let Err(wait) = state.people_searches.check(&current.user.id) {
        return Err(ApiError::rate_limited(wait));
    }
    let found = state
        .store
        .find_invitees(&workspace, &current.user.id, q, INVITEES)
        .await?;
    let items = found
        .into_iter()
        .map(|Invitee { id, name, email }| InviteeBody { id, name, email })
        .collect();
    Ok(Json(Invitees { items }))
}

#[derive(Serialize)]
struct PendingBody {
    id: String,
    workspace_id: String,
    workspace_name: String,
    role: &'static str,
    invited_by_name: String,
    expires_at: i64,
    created_at: i64,
}

impl From<PendingInvitation> for PendingBody {
    fn from(pending: PendingInvitation) -> Self {
        Self {
            id: pending.id,
            workspace_id: pending.workspace_id,
            workspace_name: pending.workspace_name,
            role: pending.role.as_str(),
            invited_by_name: pending.invited_by_name,
            expires_at: pending.expires_at,
            created_at: pending.created_at,
        }
    }
}

async fn pending<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    page: PageQuery,
) -> Result<Json<Page<PendingBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let found = state
        .store
        .pending_invitations(&current.user.id, now_ms(), after, page.fetch())
        .await?;
    Ok(Json(page.cut(found, |row| row.id.clone())))
}

async fn accept_pending<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(invitation): PathParam,
) -> Result<Response, ApiError> {
    let id = &current.user.id;
    let workspaces = state
        .store
        .accept_pending(&invitation, id, now_ms())
        .await?;
    Ok(joined(&state, current, workspaces))
}

async fn decline<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(invitation): PathParam,
) -> Result<StatusCode, ApiError> {
    let id = &current.user.id;
    state
        .store
        .decline_pending(&invitation, id, now_ms())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
