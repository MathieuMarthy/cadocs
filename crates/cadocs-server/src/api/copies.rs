//! Duplicating a page with its live descendants (docs/API.md#pages), into its space or another
//! space of the workspace. The rows of the copies are made in one transaction, so the tree shows
//! the whole copy at once, then each copy gets the content of its source through its room, one
//! page at a time, with copies of the files it shows. A failure midway moves the top copy, and
//! so the whole copy, to the trash.

use std::collections::HashMap;

use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::post;
use axum::{Extension, Json, Router};
use cadocs_collab::{Authorizer, Event, PageEvent};
use cadocs_content::yjs::{read_page, replace_page, state_len};
use cadocs_files::FileError;
use cadocs_store::{
    AccountStore, AttachmentStore, CopiedPage, DocStore, NewAttachment, PageCopy, PageStore,
    StoreError, now_ms,
};
use serde::Deserialize;
use tracing::warn;
use uuid::Uuid;

use super::attachments::file_error;
use super::auth::MAX_BODY_BYTES;
use super::pages::{PageBody, new_slug, placement};
use super::spaces::present;
use super::{ApiError, ErrorCode, JsonBody, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// The most pages one copy takes, its top page included. Copies are filled one at a time, so
/// memory holds one document, the ids of the copy and the files copied so far: this bounds them,
/// and the time a request takes.
pub const MAX_COPY_PAGES: usize = 500;
/// The most distinct files one copy takes, which bounds what it remembers of them.
pub const MAX_COPY_FILES: usize = 5000;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + PageStore + AttachmentStore,
    A: Authorizer,
{
    Router::new()
        .route("/pages/{page}/duplicate", post(duplicate))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

/// Where the copy goes, as a move says it.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DuplicateRequest {
    space_id: Option<String>,
    parent_id: Option<String>,
    #[serde(default, deserialize_with = "present")]
    after: Option<Option<String>>,
}

fn too_large(message: String) -> ApiError {
    ApiError::new(ErrorCode::TooLarge).with_message(message)
}

async fn duplicate<S: DocStore + PageStore + AttachmentStore, A: Send + Sync + 'static>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
    JsonBody(body): JsonBody<DuplicateRequest>,
) -> Result<(StatusCode, Json<PageBody>), ApiError> {
    let user = current.user.id;
    let sources = state
        .store
        .subtree_to_copy(&page, &user, MAX_COPY_PAGES + 1)
        .await?;
    if sources.len() > MAX_COPY_PAGES {
        return Err(too_large(format!(
            "A copy takes at most {MAX_COPY_PAGES} pages"
        )));
    }

    let mut copies: HashMap<&str, String> = HashMap::with_capacity(sources.len());
    let mut pages = Vec::with_capacity(sources.len());
    for source in &sources {
        let id = Uuid::now_v7().to_string();
        let parent_id = source.parent_id.as_deref();
        pages.push(CopiedPage {
            id: id.clone(),
            slug: new_slug().map_err(ApiError::internal)?,
            parent_id: parent_id.and_then(|parent| copies.get(parent)).cloned(),
            position: source.position.clone(),
            title: source.title.clone(),
        });
        copies.insert(&source.id, id);
    }
    drop(copies);
    let copy = PageCopy {
        source_id: page,
        space_id: body.space_id,
        parent_id: body.parent_id,
        placement: placement(body.after),
        pages,
        actor: user.clone(),
        now: now_ms(),
    };
    let ids: Vec<String> = copy.pages.iter().map(|page| page.id.clone()).collect();
    // Both run to their end even if the client leaves: rows once made get their content, or go
    // to the trash on a failure, a panic of the work included.
    let task = state.clone();
    let worker = user.clone();
    let work = tokio::spawn(async move {
        let mut top = task.store.create_copies(&copy).await?;
        let event = Event::page(PageEvent::Created, &top.space_id, &top.id);
        task.registry.publish(&top.workspace_id, event);
        let pairs: Vec<(String, String)> = sources
            .into_iter()
            .zip(copy.pages)
            .map(|(source, copy)| (source.id, copy.id))
            .collect();
        top.title = fill(&task, &pairs, &worker).await?;
        Ok::<_, ApiError>(top)
    });
    let task = state.clone();
    let copied = tokio::spawn(async move {
        let done = work
            .await
            .unwrap_or_else(|panic| Err(ApiError::internal(panic)));
        if done.is_err() {
            discard(&task, &ids, &user).await;
        }
        done
    });
    let top = copied.await.map_err(ApiError::internal)??;
    Ok((StatusCode::CREATED, Json(top.into())))
}

/// Moves the pages of a failed copy to the trash, whatever its maker may still do in the target:
/// a half copy must not stay. A copy whose rows were never made has nothing to trash.
async fn discard<S: PageStore, A>(state: &AppState<S, A>, ids: &[String], user: &str) {
    let page = &ids[0];
    match state.store.trash_failed_copy(ids, user, now_ms()).await {
        Ok(site) => {
            state.access_changed();
            let event = Event::page(PageEvent::Trashed, &site.space_id, page);
            state.registry.publish(&site.workspace_id, event);
        }
        Err(StoreError::PageNotFound) => {}
        Err(error) => warn!(%error, page, "cannot trash a failed copy"),
    }
}

/// Writes the content of each source into its copy, through the copy's room, and answers the
/// title of the first one.
async fn fill<S: DocStore + PageStore + AttachmentStore, A>(
    state: &AppState<S, A>,
    pairs: &[(String, String)],
    user: &str,
) -> Result<String, ApiError> {
    // Each file once, so pages that show the same one share its copy. `None` leaves the id as is.
    let mut files: HashMap<String, Option<String>> = HashMap::new();
    let mut top_title = None;
    let mut space = None;
    for (source, copy) in pairs {
        // A copy can last minutes: each source is checked again before it is read. One trashed,
        // unreadable or moved out of the space of the first one is gone, and so is the copy.
        let read = state.store.page_to_read(source, user).await?;
        if *space.get_or_insert_with(|| read.space_id.clone()) != read.space_id {
            return Err(StoreError::PageNotFound.into());
        }
        let (title, mut document, bytes) = state
            .registry
            .read(source, |doc| {
                let (title, document) = read_page(doc);
                (title, document, state_len(doc))
            })
            .await?;
        for id in document.attachments() {
            if files.contains_key(&id) {
                continue;
            }
            if files.len() >= MAX_COPY_FILES {
                return Err(too_large(format!(
                    "A copy takes at most {MAX_COPY_FILES} files"
                )));
            }
            let copied = copy_file(state, &id, copy, user).await?;
            files.insert(id, copied);
        }
        document.map_attachments(&mut |id| files.get(id).cloned().flatten());
        // Checked again once the room is open, as a load can take long.
        state.registry.open(copy).await?;
        state.store.page_to_edit(copy, user).await?;
        state
            .registry
            .rewrite(copy, user, bytes, |txn| {
                replace_page(txn, &title, &document)
            })
            .await?;
        top_title.get_or_insert(title);
    }
    Ok(top_title.unwrap_or_default())
}

/// A copy of the attachment `id` for `page`, or `None` to leave it out: one the user cannot
/// download, one of another workspace, which a crafted client can write, or one whose file is
/// gone.
async fn copy_file<S: PageStore + AttachmentStore, A>(
    state: &AppState<S, A>,
    id: &str,
    page: &str,
    user: &str,
) -> Result<Option<String>, ApiError> {
    let Some(found) = state.store.attachment(id, user).await? else {
        return Ok(None);
    };
    let target = state.store.upload_target(page, user).await?;
    if found.workspace_id != target.workspace_id {
        return Ok(None);
    }
    let quota = state
        .attachments
        .workspace_quota_bytes
        .map(|quota| quota as i64);
    // Checked before the bytes move, and again as the row is written.
    if quota.is_some_and(|quota| target.used_bytes.saturating_add(found.size) > quota) {
        return Err(StoreError::QuotaExceeded.into());
    }
    let body = match state.files.get(&found.id).await {
        Ok(body) => body,
        Err(FileError::NotFound) => return Ok(None),
        Err(error) => return Err(file_error(error)),
    };
    let copy = Uuid::now_v7().to_string();
    let max_bytes = u64::try_from(found.size).unwrap_or_default();
    let stored = state
        .files
        .put(&copy, body.bytes, max_bytes)
        .await
        .map_err(file_error)?;
    let new = NewAttachment {
        id: copy,
        page_id: page.to_owned(),
        file_name: found.file_name,
        content_type: found.content_type,
        size: stored.size as i64,
        sha256: stored.sha256,
        actor: user.to_owned(),
        now: now_ms(),
    };
    if let Err(error) = state.store.create_attachment(&new, quota).await {
        if let Err(error) = state.files.delete(&new.id).await {
            warn!(%error, id = new.id, "cannot remove an unrecorded copy");
        }
        return Err(error.into());
    }
    Ok(Some(new.id))
}
