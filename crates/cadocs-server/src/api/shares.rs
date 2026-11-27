//! Public shares of a page (V1): editors list, create and revoke them. What a share opens is
//! served under `/s/` by `crate::public`. A share reaches no socket, so a change here rechecks
//! none: each public request checks the share again.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_store::{AccountStore, DocStore, NewShare, Share, ShareStore, now_ms};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::MAX_BODY_BYTES;
use super::{ApiError, ErrorCode, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + ShareStore,
    A: Authorizer,
{
    Router::new()
        .route("/pages/{page}/shares", get(list).post(create))
        .route("/shares/{id}", delete(revoke))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

#[derive(Serialize)]
struct ShareBody {
    id: String,
    page_id: String,
    token: String,
    include_children: bool,
    indexable: bool,
    created_by: Option<String>,
    expires_at: Option<i64>,
    created_at: i64,
}

impl From<Share> for ShareBody {
    fn from(share: Share) -> Self {
        Self {
            id: share.id,
            page_id: share.page_id,
            token: share.token,
            include_children: share.include_children,
            indexable: share.indexable,
            created_by: share.created_by,
            expires_at: share.expires_at,
            created_at: share.created_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewShareRequest {
    #[serde(default)]
    include_children: bool,
    #[serde(default)]
    indexable: bool,
    #[serde(default)]
    expires_at: Option<i64>,
}

/// 128 random bits, in hex: the whole capability, so as hard to guess as a session.
fn new_token() -> Result<String, ApiError> {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).map_err(ApiError::internal)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

async fn list<S: ShareStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page_id): PathParam,
    page: PageQuery,
) -> Result<Json<Page<ShareBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let shares = state
        .store
        .list_shares(&page_id, &current.user.id, after, page.fetch(), now_ms())
        .await?;
    Ok(Json(page.cut(shares, |share| share.id.clone())))
}

async fn create<S: ShareStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page_id): PathParam,
    JsonBody(body): JsonBody<NewShareRequest>,
) -> Result<Response, ApiError> {
    let now = now_ms();
    if body.expires_at.is_some_and(|at| at <= now) {
        return Err(ApiError::new(ErrorCode::ValidationFailed).with_field("expires_at", "past"));
    }
    let new = NewShare {
        id: Uuid::now_v7().to_string(),
        page_id,
        token: new_token()?,
        include_children: body.include_children,
        indexable: body.indexable,
        expires_at: body.expires_at,
        actor: current.user.id,
        now,
    };
    let share = state.store.create_share(&new).await?;
    Ok((StatusCode::CREATED, Json(ShareBody::from(share))).into_response())
}

async fn revoke<S: ShareStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<StatusCode, ApiError> {
    state.store.delete_share(&id, &current.user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
