//! Instance administration: accounts, and the reset links an admin hands over (ADR 0013).

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::header::CACHE_CONTROL;
use axum::http::{HeaderValue, StatusCode};
use axum::middleware::{Next, from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_store::{AccountStore, AdminChanges, DocStore, UserRecord, now_ms};
use serde::{Deserialize, Serialize};

use super::auth::{MAX_BODY_BYTES, UserBody};
use super::{ApiError, ErrorCode, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession, new_password_reset};

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore,
    A: Authorizer,
{
    Router::new()
        .route("/admin/users", get(list_users))
        .route(
            "/admin/users/{user}",
            patch(update_user).delete(delete_user),
        )
        .route("/admin/users/{user}/password-reset", post(create_reset))
        // The last layer runs first: the session has to be there for the role check.
        .route_layer(from_fn(require_instance_admin))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
}

async fn require_instance_admin(
    Extension(current): Extension<CurrentSession>,
    request: Request,
    next: Next,
) -> Response {
    if !current.user.is_instance_admin {
        return ApiError::new(ErrorCode::Forbidden).into_response();
    }
    let mut response = next.run(request).await;
    // Every answer here holds accounts, or a link that takes one over.
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

#[derive(Serialize)]
struct AdminUserBody {
    #[serde(flatten)]
    user: UserBody,
    disabled_at: Option<i64>,
    created_at: i64,
}

impl From<UserRecord> for AdminUserBody {
    fn from(record: UserRecord) -> Self {
        Self {
            user: record.user.into(),
            disabled_at: record.disabled_at,
            created_at: record.created_at,
        }
    }
}

async fn list_users<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    page: PageQuery,
) -> Result<Json<Page<AdminUserBody>>, ApiError> {
    // One more than asked says whether another page exists.
    let after = page.cursor.as_deref();
    let mut users = state.store.list_users(after, page.limit + 1).await?;
    let more = users.len() > page.limit as usize;
    users.truncate(page.limit as usize);
    let next_cursor = more
        .then(|| users.last().map(|last| last.user.id.clone()))
        .flatten();
    Ok(Json(Page {
        items: users.into_iter().map(Into::into).collect(),
        next_cursor,
    }))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct UpdateUserRequest {
    disabled: Option<bool>,
    is_instance_admin: Option<bool>,
}

async fn update_user<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    PathParam(user): PathParam,
    JsonBody(body): JsonBody<UpdateUserRequest>,
) -> Result<Json<AdminUserBody>, ApiError> {
    let changes = AdminChanges {
        disabled: body.disabled,
        is_instance_admin: body.is_instance_admin,
    };
    let record = state
        .store
        .administer_user(&user, changes, now_ms())
        .await?;
    Ok(Json(record.into()))
}

async fn delete_user<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    PathParam(user): PathParam,
) -> Result<StatusCode, ApiError> {
    state.store.delete_user(&user).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Serialize)]
struct ResetBody {
    link: String,
    expires_at: i64,
}

async fn create_reset<S: AccountStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(user): PathParam,
) -> Result<Response, ApiError> {
    let (link, reset) = new_password_reset(&state.http.public_url, &user, Some(&current.user.id))
        .map_err(ApiError::internal)?;
    state.store.create_password_reset(&reset).await?;
    let body = ResetBody {
        link,
        expires_at: reset.expires_at,
    };
    Ok((StatusCode::CREATED, Json(body)).into_response())
}
