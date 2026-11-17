//! Comments of a page (V1, ADR 0012). A `commenter` writes them here, never through the page
//! socket. Anchors are stored as the client sends them, and every change publishes a `comment`
//! event to the readers of the space.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, patch, post};
use axum::{Extension, Json, Router};
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use cadocs_collab::{Authorizer, Event};
use cadocs_store::{
    AccountStore, Anchor, Comment, CommentChange, CommentStore, CommentUser, DocStore, NewComment,
    Thread, now_ms,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::Fields;
use super::{ApiError, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// The longest body, in characters once trimmed.
pub const MAX_COMMENT_CHARS: usize = 10_000;
/// The longest quote of an anchor, in characters.
pub const MAX_QUOTE_CHARS: usize = 1_000;
/// The longest relative position, decoded. A real one takes a few dozen bytes.
pub const MAX_POSITION_BYTES: usize = 1_024;
/// A body of `MAX_COMMENT_CHARS` in 4-byte characters, with room for the rest of the request.
const MAX_REQUEST_BYTES: usize = 64 * 1024;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + CommentStore,
    A: Authorizer,
{
    Router::new()
        .route("/pages/{page}/comments", get(list).post(create))
        .route("/comments/{comment}", patch(edit).delete(delete_comment))
        .route("/comments/{comment}/resolve", post(resolve))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
}

#[derive(Serialize)]
struct UserBody {
    id: String,
    name: String,
}

impl From<CommentUser> for UserBody {
    fn from(user: CommentUser) -> Self {
        Self {
            id: user.id,
            name: user.name,
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct AnchorBody {
    /// Base64 of an encoded Yjs `RelativePosition`.
    start: String,
    end: String,
    text: String,
}

#[derive(Serialize)]
struct CommentBody {
    id: String,
    page_id: String,
    parent_id: Option<String>,
    author: Option<UserBody>,
    body: String,
    anchor: Option<AnchorBody>,
    resolved_at: Option<i64>,
    resolved_by: Option<UserBody>,
    created_at: i64,
    updated_at: i64,
}

impl From<Comment> for CommentBody {
    fn from(comment: Comment) -> Self {
        Self {
            id: comment.id,
            page_id: comment.page_id,
            parent_id: comment.parent_id,
            author: comment.author.map(Into::into),
            body: comment.body,
            anchor: comment.anchor.map(|anchor| AnchorBody {
                start: STANDARD.encode(anchor.start),
                end: STANDARD.encode(anchor.end),
                text: anchor.text,
            }),
            resolved_at: comment.resolved_at,
            resolved_by: comment.resolved_by.map(Into::into),
            created_at: comment.created_at,
            updated_at: comment.updated_at,
        }
    }
}

#[derive(Serialize)]
struct ThreadBody {
    #[serde(flatten)]
    comment: CommentBody,
    replies: Vec<CommentBody>,
}

impl From<Thread> for ThreadBody {
    fn from(thread: Thread) -> Self {
        Self {
            comment: thread.comment.into(),
            replies: thread.replies.into_iter().map(Into::into).collect(),
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    body: String,
    #[serde(default)]
    parent_id: Option<String>,
    #[serde(default)]
    anchor: Option<AnchorBody>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditRequest {
    body: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ResolveRequest {
    resolved: bool,
}

fn clean_body(body: &str) -> Result<String, &'static str> {
    let body = body.trim();
    if body.is_empty() {
        Err("required")
    } else if body.chars().count() > MAX_COMMENT_CHARS {
        Err("too_long")
    } else {
        Ok(body.to_owned())
    }
}

fn position(encoded: &str) -> Option<Vec<u8>> {
    // Base64 of `MAX_POSITION_BYTES` takes a third more: refuse longer before decoding.
    if encoded.len() > MAX_POSITION_BYTES.div_ceil(3) * 4 {
        return None;
    }
    let bytes = STANDARD.decode(encoded).ok()?;
    (!bytes.is_empty() && bytes.len() <= MAX_POSITION_BYTES).then_some(bytes)
}

fn clean_anchor(anchor: &AnchorBody) -> Result<Option<Anchor>, &'static str> {
    if anchor.text.chars().count() > MAX_QUOTE_CHARS {
        return Err("too_long");
    }
    match (position(&anchor.start), position(&anchor.end)) {
        (Some(start), Some(end)) => Ok(Some(Anchor {
            start,
            end,
            text: anchor.text.clone(),
        })),
        _ => Err("invalid"),
    }
}

/// Tells the readers of the space, once the change is stored.
fn announce<S: DocStore, A>(state: &AppState<S, A>, change: &CommentChange) {
    let event = Event::comment(&change.site.space_id, &change.comment.page_id);
    state.registry.publish(&change.site.workspace_id, event);
}

async fn list<S: DocStore + CommentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
    query: PageQuery,
) -> Result<Json<Page<ThreadBody>>, ApiError> {
    let after = query.cursor.as_deref();
    let threads = state
        .store
        .threads(&page, &current.user.id, after, query.fetch())
        .await?;
    Ok(Json(query.cut(threads, |thread| thread.comment.id.clone())))
}

async fn create<S: DocStore + CommentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
    JsonBody(request): JsonBody<CreateRequest>,
) -> Result<Response, ApiError> {
    let mut fields = Fields::default();
    let body = fields.check("body", clean_body(&request.body));
    let anchor = match (&request.anchor, &request.parent_id) {
        (None, _) => None,
        // ADR 0012: a reply belongs to its thread's anchor.
        (Some(_), Some(_)) => fields.check("anchor", Err::<Option<Anchor>, _>("reply")),
        (Some(anchor), None) => fields.check("anchor", clean_anchor(anchor)),
    };
    fields.finish()?;
    let new = NewComment {
        id: Uuid::now_v7().to_string(),
        page_id: page,
        parent_id: request.parent_id,
        actor: current.user.id,
        body,
        anchor,
        now: now_ms(),
    };
    let change = state.store.create_comment(&new).await?;
    announce(&state, &change);
    Ok((StatusCode::CREATED, Json(CommentBody::from(change.comment))).into_response())
}

async fn edit<S: DocStore + CommentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(comment): PathParam,
    JsonBody(request): JsonBody<EditRequest>,
) -> Result<Json<CommentBody>, ApiError> {
    let mut fields = Fields::default();
    let body = fields.check("body", clean_body(&request.body));
    fields.finish()?;
    let change = state
        .store
        .edit_comment(&comment, &current.user.id, &body, now_ms())
        .await?;
    announce(&state, &change);
    Ok(Json(change.comment.into()))
}

async fn delete_comment<S: DocStore + CommentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(comment): PathParam,
) -> Result<StatusCode, ApiError> {
    let change = state
        .store
        .delete_comment(&comment, &current.user.id)
        .await?;
    announce(&state, &change);
    Ok(StatusCode::NO_CONTENT)
}

async fn resolve<S: DocStore + CommentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(comment): PathParam,
    JsonBody(request): JsonBody<ResolveRequest>,
) -> Result<Json<CommentBody>, ApiError> {
    let change = state
        .store
        .resolve_comment(&comment, &current.user.id, request.resolved, now_ms())
        .await?;
    announce(&state, &change);
    Ok(Json(change.comment.into()))
}
