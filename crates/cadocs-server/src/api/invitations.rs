//! Invitation links (ADR 0013): admins make them, and whoever holds one joins with it.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_core::permissions::WorkspaceRole;
use cadocs_store::{
    AccountStore, DocStore, Invitation, InvitationPreview, InvitedUser, StoreError, User,
    WorkspaceStore, now_ms,
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
        .route("/invitations/{token}", delete(revoke))
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
    state.store.create_invitation(&new).await?;
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
    state.access_changed();
    Ok(session_body(current.user, workspaces, &current.token))
}
