//! Pages and their tree. A page the caller cannot read answers `404`, and so does a trashed one,
//! outside the trash. The title lives in the Yjs document, so no route here writes it.

use axum::extract::{DefaultBodyLimit, RawQuery, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_store::{
    AccountStore, DateCursor, DocStore, NewPage, PageMove, PageRecord, PageStore, Placement, now_ms,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::MAX_BODY_BYTES;
use super::spaces::present;
use super::{ApiError, ErrorCode, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// 32 letters and digits, none easy to mistake for another: a byte gives one with no bias.
const SLUG_ALPHABET: &[u8; 32] = b"0123456789abcdefghjkmnpqrstvwxyz";
const SLUG_CHARS: usize = 12;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + PageStore,
    A: Authorizer,
{
    Router::new()
        .route("/spaces/{space}/pages", post(create))
        .route("/spaces/{space}/tree", get(tree))
        .route("/spaces/{space}/trash", get(list_trash))
        .route("/pages/{page}", get(read).delete(trash))
        .route("/pages/{page}/move", post(move_page))
        .route("/pages/{page}/restore", post(restore))
        .route("/pages/{page}/purge", delete(purge))
        .route("/workspaces/{ws}/pages/by-slug/{slug}", get(read_by_slug))
        .route("/workspaces/{ws}/recent", get(recent))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

#[derive(Serialize)]
struct PageBody {
    id: String,
    workspace_id: String,
    space_id: String,
    parent_id: Option<String>,
    position: String,
    slug: String,
    title: String,
    role: &'static str,
    has_children: bool,
    created_by: Option<String>,
    updated_by: Option<String>,
    deleted_at: Option<i64>,
    deleted_by: Option<String>,
    created_at: i64,
    updated_at: i64,
}

impl From<PageRecord> for PageBody {
    fn from(page: PageRecord) -> Self {
        Self {
            id: page.id,
            workspace_id: page.workspace_id,
            space_id: page.space_id,
            parent_id: page.parent_id,
            position: page.position,
            slug: page.slug,
            title: page.title,
            role: page.role.as_str(),
            has_children: page.has_children,
            created_by: page.created_by,
            updated_by: page.updated_by,
            deleted_at: page.deleted_at,
            deleted_by: page.deleted_by,
            created_at: page.created_at,
            updated_at: page.updated_at,
        }
    }
}

fn new_slug() -> Result<String, getrandom::Error> {
    let mut bytes = [0; SLUG_CHARS];
    getrandom::fill(&mut bytes)?;
    let letter = |byte: &u8| char::from(SLUG_ALPHABET[usize::from(byte % 32)]);
    Ok(bytes.iter().map(letter).collect())
}

/// `after` as a request has it: missing for the end of the list, `null` for its start.
fn placement(after: Option<Option<String>>) -> Placement {
    match after {
        None => Placement::Last,
        Some(None) => Placement::First,
        Some(Some(sibling)) => Placement::After(sibling),
    }
}

/// The cursor of a list ordered by date.
fn date_cursor(date: i64, id: &str) -> String {
    format!("{date}_{id}")
}

fn parse_date_cursor(page: &PageQuery) -> Result<Option<DateCursor>, ApiError> {
    let Some(cursor) = &page.cursor else {
        return Ok(None);
    };
    let parsed = cursor.split_once('_').and_then(|(date, id)| {
        let date = date.parse().ok()?;
        let id = Uuid::parse_str(id).ok()?.to_string();
        Some(DateCursor { date, id })
    });
    let ours = parsed.filter(|parsed| date_cursor(parsed.date, &parsed.id) == *cursor);
    let invalid = || ApiError::new(ErrorCode::ValidationFailed).with_field("cursor", "invalid");
    ours.map(Some).ok_or_else(invalid)
}

/// `?parent=` holds an id, which needs no decoding.
fn parent_param(query: Option<&str>) -> Option<&str> {
    let mut pairs = query.unwrap_or_default().split('&');
    pairs.find_map(|pair| pair.strip_prefix("parent="))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateRequest {
    parent_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    after: Option<Option<String>>,
}

async fn create<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    JsonBody(body): JsonBody<CreateRequest>,
) -> Result<Response, ApiError> {
    let new = NewPage {
        id: Uuid::now_v7().to_string(),
        space_id: space,
        parent_id: body.parent_id,
        placement: placement(body.after),
        slug: new_slug().map_err(ApiError::internal)?,
        actor: current.user.id,
        now: now_ms(),
    };
    let page = state.store.create_page(&new).await?;
    Ok((StatusCode::CREATED, Json(PageBody::from(page))).into_response())
}

async fn read<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
) -> Result<Json<PageBody>, ApiError> {
    let found = state.store.page(&page, &current.user.id).await?;
    let page = found.ok_or_else(|| ApiError::new(ErrorCode::NotFound))?;
    Ok(Json(page.into()))
}

async fn read_by_slug<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam((workspace, slug)): PathParam<(String, String)>,
) -> Result<Json<PageBody>, ApiError> {
    let actor = &current.user.id;
    let found = state.store.page_by_slug(&workspace, &slug, actor).await?;
    let page = found.ok_or_else(|| ApiError::new(ErrorCode::NotFound))?;
    Ok(Json(page.into()))
}

async fn tree<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    RawQuery(query): RawQuery,
    page: PageQuery,
) -> Result<Json<Page<PageBody>>, ApiError> {
    let parent = parent_param(query.as_deref());
    let after = page.cursor.as_deref();
    let pages = state
        .store
        .tree(&space, parent, &current.user.id, after, page.fetch())
        .await?;
    Ok(Json(page.cut(pages, |page| page.position.clone())))
}

async fn list_trash<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    page: PageQuery,
) -> Result<Json<Page<PageBody>>, ApiError> {
    let after = parse_date_cursor(&page)?;
    let pages = state
        .store
        .list_trashed_pages(&space, &current.user.id, after.as_ref(), page.fetch())
        .await?;
    let cursor = |page: &PageRecord| date_cursor(page.deleted_at.unwrap_or_default(), &page.id);
    Ok(Json(page.cut(pages, cursor)))
}

async fn recent<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    page: PageQuery,
) -> Result<Json<Page<PageBody>>, ApiError> {
    let after = parse_date_cursor(&page)?;
    let pages = state
        .store
        .recent_pages(&workspace, &current.user.id, after.as_ref(), page.fetch())
        .await?;
    Ok(Json(
        page.cut(pages, |page| date_cursor(page.updated_at, &page.id)),
    ))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveRequest {
    space_id: Option<String>,
    parent_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    after: Option<Option<String>>,
}

async fn move_page<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
    JsonBody(body): JsonBody<MoveRequest>,
) -> Result<Json<PageBody>, ApiError> {
    let destination = PageMove {
        space_id: body.space_id,
        parent_id: body.parent_id,
        placement: placement(body.after),
    };
    let page = state
        .store
        .move_page(&page, &current.user.id, &destination, now_ms())
        .await?;
    Ok(Json(page.into()))
}

async fn trash<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
) -> Result<StatusCode, ApiError> {
    state
        .store
        .trash_page(&page, &current.user.id, now_ms())
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn restore<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
) -> Result<Json<PageBody>, ApiError> {
    let page = state.store.restore_page(&page, &current.user.id).await?;
    Ok(Json(page.into()))
}

async fn purge<S: PageStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
) -> Result<StatusCode, ApiError> {
    state.store.purge_page(&page, &current.user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_slug_is_short_and_random() {
        let slug = new_slug().unwrap();
        assert_eq!(slug.len(), SLUG_CHARS);
        assert!(slug.bytes().all(|byte| SLUG_ALPHABET.contains(&byte)));
        assert_ne!(slug, new_slug().unwrap());
    }

    #[test]
    fn the_parent_is_read_from_the_query() {
        assert_eq!(parent_param(None), None);
        assert_eq!(parent_param(Some("limit=5")), None);
        assert_eq!(parent_param(Some("limit=5&parent=abc")), Some("abc"));
    }
}
