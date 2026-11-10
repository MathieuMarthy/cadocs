//! Workspaces and their members. Whoever is not a member finds nothing here.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_core::permissions::WorkspaceRole;
use cadocs_search::Language;
use cadocs_store::{
    AccountStore, DocStore, Member, Membership, NewWorkspace, Workspace, WorkspaceChanges,
    WorkspaceStore, now_ms,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::{Fields, MAX_BODY_BYTES, WorkspaceBody, clean_name, slug};
use super::{ApiError, ErrorCode, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + WorkspaceStore,
    A: Authorizer,
{
    Router::new()
        .route("/workspaces", get(list).post(create))
        .route(
            "/workspaces/{ws}",
            get(read).patch(update).delete(delete_workspace),
        )
        .route("/workspaces/{ws}/members", get(list_members))
        .route(
            "/workspaces/{ws}/members/{user}",
            patch(update_member).delete(remove_member),
        )
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

pub(super) fn invalid_role() -> ApiError {
    ApiError::new(ErrorCode::ValidationFailed).with_field("role", "invalid")
}

#[derive(Serialize)]
struct WorkspaceDetail {
    #[serde(flatten)]
    workspace: WorkspaceBody,
    language: String,
    created_at: i64,
    updated_at: i64,
}

impl From<Workspace> for WorkspaceDetail {
    fn from(workspace: Workspace) -> Self {
        Self {
            workspace: workspace.membership.into(),
            language: workspace.language,
            created_at: workspace.created_at,
            updated_at: workspace.updated_at,
        }
    }
}

async fn list<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    page: PageQuery,
) -> Result<Json<Page<WorkspaceBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let workspaces = state
        .store
        .list_workspaces(&current.user.id, after, page.fetch())
        .await?;
    let cursor = |membership: &Membership| membership.workspace_id.clone();
    Ok(Json(page.cut(workspaces, cursor)))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    name: String,
}

async fn create<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    JsonBody(body): JsonBody<CreateRequest>,
) -> Result<Response, ApiError> {
    let mut fields = Fields::default();
    let name = fields.check("name", clean_name(&body.name));
    fields.finish()?;

    let new = NewWorkspace {
        id: Uuid::now_v7().to_string(),
        slug: slug(&name, "workspace"),
        name,
        owner_id: current.user.id,
        now: now_ms(),
    };
    let workspace = state.store.create_workspace(&new).await?;
    let body = WorkspaceDetail::from(workspace);
    Ok((StatusCode::CREATED, Json(body)).into_response())
}

async fn read<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    let found = state.store.workspace(&workspace, &current.user.id).await?;
    let workspace = found.ok_or_else(|| ApiError::new(ErrorCode::NotFound))?;
    Ok(Json(workspace.into()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequest {
    name: Option<String>,
    language: Option<String>,
}

fn clean_language(language: String) -> Result<String, &'static str> {
    if Language::parse(&language).is_some() {
        Ok(language)
    } else {
        Err("invalid")
    }
}

async fn update<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    JsonBody(body): JsonBody<UpdateRequest>,
) -> Result<Json<WorkspaceDetail>, ApiError> {
    let mut fields = Fields::default();
    let name = body
        .name
        .map(|name| fields.check("name", clean_name(&name)));
    let language = body
        .language
        .map(|language| fields.check("language", clean_language(language)));
    fields.finish()?;

    let changes = WorkspaceChanges { name, language };
    let workspace = state
        .store
        .update_workspace(&workspace, &current.user.id, &changes, now_ms())
        .await?;
    Ok(Json(workspace.into()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteRequest {
    name: String,
}

async fn delete_workspace<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    JsonBody(body): JsonBody<DeleteRequest>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .delete_workspace(&workspace, &current.user.id, &body.name)
        .await?;
    state.access_changed();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct MemberBody {
    id: String,
    email: String,
    name: String,
    role: &'static str,
    disabled: bool,
    created_at: i64,
}

impl From<Member> for MemberBody {
    fn from(member: Member) -> Self {
        Self {
            id: member.id,
            email: member.email,
            name: member.name,
            role: member.role.as_str(),
            disabled: member.disabled,
            created_at: member.created_at,
        }
    }
}

async fn list_members<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    page: PageQuery,
) -> Result<Json<Page<MemberBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let members = state
        .store
        .list_members(&workspace, &current.user.id, after, page.fetch())
        .await?;
    Ok(Json(page.cut(members, |member| member.id.clone())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateMemberRequest {
    role: String,
}

async fn update_member<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam((workspace, user)): PathParam<(String, String)>,
    JsonBody(body): JsonBody<UpdateMemberRequest>,
) -> Result<Json<MemberBody>, ApiError> {
    let role = WorkspaceRole::parse(&body.role).ok_or_else(invalid_role)?;

    let member = state
        .store
        .set_member_role(&workspace, &current.user.id, &user, role)
        .await?;
    state.access_changed();
    Ok(Json(member.into()))
}

async fn remove_member<S: WorkspaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam((workspace, user)): PathParam<(String, String)>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .remove_member(&workspace, &current.user.id, &user)
        .await?;
    state.access_changed();
    Ok(StatusCode::NO_CONTENT)
}
