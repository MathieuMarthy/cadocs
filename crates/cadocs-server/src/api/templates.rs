//! Templates of a space (docs/API.md#templates): editors save a page as one and delete it,
//! readers of the space list them, and an editor of a space starts a page from one. A new page
//! is made empty, as a copy is, then gets the content of the template through its room, with
//! copies of the files it shows. A failure midway moves it to the trash.

use std::collections::HashMap;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::{delete, get, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_content::yjs::replace_page;
use cadocs_jobs::{Decoded, decode_bounded, pack};
use cadocs_store::{
    AccountStore, AttachmentStore, DocStore, MAX_TITLE_CHARS, NewTemplate, PageStore, Template,
    TemplateStore, now_ms,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::auth::MAX_BODY_BYTES;
use super::copies::{MAX_COPY_FILES, copy_file, discard};
use super::pages::{PageBody, add, placement};
use super::spaces::present;
use super::{ApiError, ErrorCode, JsonBody, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + PageStore + AttachmentStore + TemplateStore,
    A: Authorizer,
{
    Router::new()
        .route("/spaces/{space}/templates", get(list))
        .route("/pages/{page}/template", post(save))
        .route("/templates/{id}", delete(remove))
        .route("/templates/{id}/pages", post(start))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

#[derive(Serialize)]
struct TemplateBody {
    id: String,
    space_id: String,
    title: String,
    created_by: Option<String>,
    created_at: i64,
}

impl From<Template> for TemplateBody {
    fn from(template: Template) -> Self {
        Self {
            id: template.id,
            space_id: template.space_id,
            title: template.title,
            created_by: template.created_by,
            created_at: template.created_at,
        }
    }
}

async fn list<S: TemplateStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    page: PageQuery,
) -> Result<Json<Page<TemplateBody>>, ApiError> {
    let after = page.cursor.as_deref();
    let rows = state
        .store
        .list_templates(&space, &current.user.id, after, page.fetch())
        .await?;
    let rows: Vec<TemplateBody> = rows.into_iter().map(Into::into).collect();
    Ok(Json(page.cut(rows, |row| row.id.clone())))
}

/// Keeps the page as it is now, flushed or not, with its title.
async fn save<S: DocStore + PageStore + TemplateStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
) -> Result<(StatusCode, Json<TemplateBody>), ApiError> {
    let user = current.user.id;
    let read = state.store.page_to_read(&page, &user).await?;
    let (title, packed) = pack(&state.registry, &page).await?;
    let new = NewTemplate {
        id: Uuid::now_v7().to_string(),
        space_id: read.space_id,
        title: title.chars().take(MAX_TITLE_CHARS).collect(),
        state: packed,
        actor: user,
        now: now_ms(),
    };
    let template = state.store.create_template(&new).await?;
    Ok((StatusCode::CREATED, Json(template.into())))
}

async fn remove<S: TemplateStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<StatusCode, ApiError> {
    state.store.delete_template(&id, &current.user.id).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Where the page goes, as a move says it. The space is the template's own when missing.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartRequest {
    space_id: Option<String>,
    parent_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    after: Option<Option<String>>,
}

async fn start<
    S: DocStore + PageStore + AttachmentStore + TemplateStore,
    A: Send + Sync + 'static,
>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
    JsonBody(body): JsonBody<StartRequest>,
) -> Result<(StatusCode, Json<PageBody>), ApiError> {
    let user = current.user.id;
    let found = state
        .store
        .template_to_use(&id, body.space_id.as_deref(), &user)
        .await?;
    let decoded = decode_bounded(&state.registry, &found.state).await?;
    drop(found.state);
    let space = body.space_id.unwrap_or(found.template.space_id);
    let mut page = add(
        &state,
        space,
        body.parent_id,
        placement(body.after),
        user.clone(),
    )
    .await?;
    let id = page.id.clone();
    // Runs to its end even if the client leaves: a page made must get its content or go to the
    // trash, a panic of the work included.
    let task = state.clone();
    let worker = user.clone();
    let work = tokio::spawn(async move { fill(&task, &id, decoded, &worker).await });
    let task = state.clone();
    let id = page.id.clone();
    let title = tokio::spawn(async move {
        let done = work
            .await
            .unwrap_or_else(|panic| Err(ApiError::internal(panic)));
        if done.is_err() {
            discard(&task, &[id], &user).await;
        }
        done
    })
    .await
    .map_err(ApiError::internal)??;
    page.title = title;
    Ok((StatusCode::CREATED, Json(page.into())))
}

/// Writes the template into the page through its room, and answers its title.
async fn fill<S: DocStore + PageStore + AttachmentStore, A>(
    state: &AppState<S, A>,
    page: &str,
    decoded: Decoded,
    user: &str,
) -> Result<String, ApiError> {
    let Decoded {
        title,
        mut document,
        state_bytes,
    } = decoded;
    // Each file once, so a file the content shows twice has one copy. `None` leaves the id as is.
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    for id in document.attachments() {
        if files.contains_key(&id) {
            continue;
        }
        if files.len() >= MAX_COPY_FILES {
            return Err(ApiError::new(ErrorCode::TooLarge)
                .with_message(format!("A template takes at most {MAX_COPY_FILES} files")));
        }
        let copied = copy_file(state, &id, page, user).await?;
        files.insert(id, copied);
    }
    document.map_attachments(&mut |id| files.get(id).cloned().flatten());
    // Checked again once the room is open, as a load can take long.
    state.registry.open(page).await?;
    state.store.page_to_edit(page, user).await?;
    state
        .registry
        .rewrite(page, user, state_bytes, |txn| {
            replace_page(txn, &title, &document)
        })
        .await?;
    Ok(title)
}
