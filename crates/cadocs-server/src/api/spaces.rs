//! Spaces and their grants. A space the caller cannot read answers `404`.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_core::permissions::{DefaultRole, SpaceRole};
use cadocs_store::{
    AccountStore, DocStore, Grant, GrantTarget, NewGrant, NewSpace, Space, SpaceChanges,
    SpaceStore, now_ms,
};
use serde::{Deserialize, Deserializer, Serialize};
use uuid::Uuid;

use super::auth::{Fields, MAX_BODY_BYTES, clean_name, slug};
use super::workspaces::invalid_role;
use super::{ApiError, ErrorCode, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

const MAX_DESCRIPTION_CHARS: usize = 1000;
/// A `PUT` sends them all, and they are written in one transaction.
const MAX_GRANTS: usize = 500;
/// Room for `MAX_GRANTS` grants.
const MAX_GRANTS_BODY_BYTES: usize = 64 * 1024;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + SpaceStore,
    A: Authorizer,
{
    let grants = Router::new()
        .route("/spaces/{space}/grants", get(list_grants).put(set_grants))
        .layer(DefaultBodyLimit::max(MAX_GRANTS_BODY_BYTES));
    Router::new()
        .route("/workspaces/{ws}/spaces", get(list).post(create))
        .route("/workspaces/{ws}/spaces/trash", get(list_trash))
        .route("/spaces/{space}", get(read).patch(update).delete(trash))
        .route("/spaces/{space}/restore", post(restore))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .merge(grants)
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

#[derive(Serialize)]
struct SpaceBody {
    id: String,
    workspace_id: String,
    slug: String,
    name: String,
    description: String,
    default_role: Option<&'static str>,
    role: &'static str,
    deleted_at: Option<i64>,
    created_at: i64,
    updated_at: i64,
}

impl From<Space> for SpaceBody {
    fn from(space: Space) -> Self {
        Self {
            id: space.id,
            workspace_id: space.workspace_id,
            slug: space.slug,
            name: space.name,
            description: space.description,
            default_role: space.default_role.map(DefaultRole::as_str),
            role: space.role.as_str(),
            deleted_at: space.deleted_at,
            created_at: space.created_at,
            updated_at: space.updated_at,
        }
    }
}

fn clean_description(description: &str) -> Result<String, &'static str> {
    let description = description.trim();
    if description.chars().count() > MAX_DESCRIPTION_CHARS {
        Err("too_long")
    } else {
        Ok(description.to_owned())
    }
}

/// `None` for no default role.
fn clean_default_role(role: Option<&str>) -> Result<Option<DefaultRole>, &'static str> {
    role.map(|role| DefaultRole::parse(role).ok_or("invalid"))
        .transpose()
}

/// For a field where `null` is a value: it comes out as `Some(None)`, and a missing field as
/// `None`, through `#[serde(default)]`.
pub(super) fn present<'de, T, D>(deserializer: D) -> Result<Option<T>, D::Error>
where
    T: Deserialize<'de>,
    D: Deserializer<'de>,
{
    T::deserialize(deserializer).map(Some)
}

async fn list<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    page: PageQuery,
) -> Result<Json<Page<SpaceBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let spaces = state
        .store
        .list_spaces(&workspace, &current.user.id, after, page.fetch())
        .await?;
    Ok(Json(page.cut(spaces, |space| space.id.clone())))
}

async fn list_trash<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    page: PageQuery,
) -> Result<Json<Page<SpaceBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let spaces = state
        .store
        .list_trashed_spaces(&workspace, &current.user.id, after, page.fetch())
        .await?;
    Ok(Json(page.cut(spaces, |space| space.id.clone())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    name: String,
    description: Option<String>,
    default_role: Option<String>,
}

async fn create<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    JsonBody(body): JsonBody<CreateRequest>,
) -> Result<Response, ApiError> {
    let mut fields = Fields::default();
    let name = fields.check("name", clean_name(&body.name));
    let description = body.description.unwrap_or_default();
    let description = fields.check("description", clean_description(&description));
    let default_role = clean_default_role(body.default_role.as_deref());
    let default_role = fields.check("default_role", default_role);
    fields.finish()?;

    let new = NewSpace {
        id: Uuid::now_v7().to_string(),
        workspace_id: workspace,
        slug: slug(&name, "space"),
        name,
        description,
        default_role,
        actor: current.user.id,
        now: now_ms(),
    };
    let space = state.store.create_space(&new).await?;
    Ok((StatusCode::CREATED, Json(SpaceBody::from(space))).into_response())
}

async fn read<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
) -> Result<Json<SpaceBody>, ApiError> {
    let found = state.store.space(&space, &current.user.id).await?;
    let space = found.ok_or_else(|| ApiError::new(ErrorCode::NotFound))?;
    Ok(Json(space.into()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateRequest {
    name: Option<String>,
    description: Option<String>,
    #[serde(default, deserialize_with = "present")]
    default_role: Option<Option<String>>,
}

async fn update<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    JsonBody(body): JsonBody<UpdateRequest>,
) -> Result<Json<SpaceBody>, ApiError> {
    let mut fields = Fields::default();
    let name = body
        .name
        .map(|name| fields.check("name", clean_name(&name)));
    let description = body
        .description
        .map(|text| fields.check("description", clean_description(&text)));
    let default_role = body
        .default_role
        .map(|role| fields.check("default_role", clean_default_role(role.as_deref())));
    fields.finish()?;

    let changes = SpaceChanges {
        name,
        description,
        default_role,
    };
    let space = state
        .store
        .update_space(&space, &current.user.id, &changes, now_ms())
        .await?;
    Ok(Json(space.into()))
}

async fn trash<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .trash_space(&space, &current.user.id, now_ms())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
) -> Result<Json<SpaceBody>, ApiError> {
    let space = state
        .store
        .restore_space(&space, &current.user.id, now_ms())
        .await?;
    Ok(Json(space.into()))
}

#[derive(Serialize)]
struct GrantBody {
    id: String,
    user_id: Option<String>,
    group_id: Option<String>,
    role: &'static str,
    created_at: i64,
}

impl From<Grant> for GrantBody {
    fn from(grant: Grant) -> Self {
        let (user_id, group_id) = match grant.target {
            GrantTarget::User(id) => (Some(id), None),
            GrantTarget::Group(id) => (None, Some(id)),
        };
        Self {
            id: grant.id,
            user_id,
            group_id,
            role: grant.role.as_str(),
            created_at: grant.created_at,
        }
    }
}

async fn list_grants<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    page: PageQuery,
) -> Result<Json<Page<GrantBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let grants = state
        .store
        .list_grants(&space, &current.user.id, after, page.fetch())
        .await?;
    Ok(Json(page.cut(grants, |grant| grant.id.clone())))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantRequest {
    user_id: Option<String>,
    group_id: Option<String>,
    role: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SetGrantsRequest {
    grants: Vec<GrantRequest>,
}

fn grants_refused(problem: &'static str) -> ApiError {
    ApiError::new(ErrorCode::ValidationFailed).with_field("grants", problem)
}

fn new_grants(requests: Vec<GrantRequest>) -> Result<Vec<NewGrant>, ApiError> {
    if requests.len() > MAX_GRANTS {
        return Err(grants_refused("too_many"));
    }
    let mut grants = Vec::with_capacity(requests.len());
    for request in requests {
        let target = match (request.user_id, request.group_id) {
            (Some(id), None) => GrantTarget::User(id),
            (None, Some(id)) => GrantTarget::Group(id),
            _ => return Err(grants_refused("invalid")),
        };
        let role = SpaceRole::parse(&request.role).ok_or_else(invalid_role)?;
        grants.push(NewGrant {
            id: Uuid::now_v7().to_string(),
            target,
            role,
        });
    }
    Ok(grants)
}

async fn set_grants<S: SpaceStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    JsonBody(body): JsonBody<SetGrantsRequest>,
) -> Result<StatusCode, ApiError> {
    let grants = new_grants(body.grants)?;
    state
        .store
        .set_grants(&space, &current.user.id, &grants, now_ms())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
