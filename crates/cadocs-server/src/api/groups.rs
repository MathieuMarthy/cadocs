//! Groups of a workspace and their members (V1). A change to who is in a group, or a deleted
//! group, can change what its members reach, so it rechecks open sockets.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, put};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_store::{AccountStore, DocStore, Group, GroupStore, Member, NewGroup, now_ms};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::{Fields, MAX_BODY_BYTES, clean_name};
use super::workspaces::MemberBody;
use super::{ApiError, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + GroupStore,
    A: Authorizer,
{
    Router::new()
        .route("/workspaces/{ws}/groups", get(list).post(create))
        .route("/groups/{group}", patch(rename).delete(delete_group))
        .route("/groups/{group}/members", get(list_members))
        .route(
            "/groups/{group}/members/{user}",
            put(add_member).delete(remove_member),
        )
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

#[derive(Serialize)]
struct GroupBody {
    id: String,
    workspace_id: String,
    name: String,
    member_count: i64,
    created_at: i64,
}

impl From<Group> for GroupBody {
    fn from(group: Group) -> Self {
        Self {
            id: group.id,
            workspace_id: group.workspace_id,
            name: group.name,
            member_count: group.member_count,
            created_at: group.created_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NameRequest {
    name: String,
}

fn checked_name(body: &NameRequest) -> Result<String, ApiError> {
    let mut fields = Fields::default();
    let name = fields.check("name", clean_name(&body.name));
    fields.finish()?;
    Ok(name)
}

async fn list<S: GroupStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    page: PageQuery,
) -> Result<Json<Page<GroupBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let groups = state
        .store
        .list_groups(&workspace, &current.user.id, after, page.fetch())
        .await?;
    Ok(Json(page.cut(groups, |group| group.id.clone())))
}

async fn create<S: GroupStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    JsonBody(body): JsonBody<NameRequest>,
) -> Result<Response, ApiError> {
    let new = NewGroup {
        id: Uuid::now_v7().to_string(),
        workspace_id: workspace,
        name: checked_name(&body)?,
        actor: current.user.id,
        now: now_ms(),
    };
    let group = state.store.create_group(&new).await?;
    Ok((StatusCode::CREATED, Json(GroupBody::from(group))).into_response())
}

async fn rename<S: GroupStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(group): PathParam,
    JsonBody(body): JsonBody<NameRequest>,
) -> Result<Json<GroupBody>, ApiError> {
    let name = checked_name(&body)?;
    let group = state
        .store
        .rename_group(&group, &current.user.id, &name)
        .await?;
    Ok(Json(group.into()))
}

async fn delete_group<S: GroupStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(group): PathParam,
) -> Result<StatusCode, ApiError> {
    state.store.delete_group(&group, &current.user.id).await?;
    state.access_changed();
    Ok(StatusCode::NO_CONTENT)
}

async fn list_members<S: GroupStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(group): PathParam,
    page: PageQuery,
) -> Result<Json<Page<MemberBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let members = state
        .store
        .list_group_members(&group, &current.user.id, after, page.fetch())
        .await?;
    Ok(Json(page.cut(members, |member: &Member| member.id.clone())))
}

async fn add_member<S: GroupStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam((group, user)): PathParam<(String, String)>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .add_group_member(&group, &current.user.id, &user)
        .await?;
    state.access_changed();
    Ok(StatusCode::NO_CONTENT)
}

async fn remove_member<S: GroupStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam((group, user)): PathParam<(String, String)>,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .remove_group_member(&group, &current.user.id, &user)
        .await?;
    state.access_changed();
    Ok(StatusCode::NO_CONTENT)
}
