//! Bulk imports (docs/API.md#spaces): a zip of Markdown files and folders becomes a tree of
//! pages, the mirror of a space export. `a.md` is a page, and what the folder `a/` holds goes
//! under it. A folder with no such file is an empty page named after it. The zip is checked as it
//! arrives, then a job makes the pages and writes each through its room. A job that fails moves
//! all of them to the trash.
//!
//! shortcut: jobs live in memory (`exports.rs`), so a restart in the middle of an import leaves
//! its pages as far as it got. Record them in the `jobs` table if that happens to someone.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, PoisonError};

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::post;
use axum::{Extension, Json, Router};
use cadocs_content::ast::Document;
use cadocs_core::permissions::SpaceRole;
use cadocs_store::{
    AccountStore, AttachmentStore, DocStore, MAX_PAGE_DEPTH, MAX_TITLE_CHARS, PageStore, Placement,
    SpaceStore,
};
use tracing::warn;
use uuid::Uuid;

use super::copies::discard;
use super::exports::{JobBody, Progress, run};
use super::markdown::{
    Absolute, Archive, MAX_MARKDOWN_BYTES, Parsed, Unbundle, fill, invalid, not_found, param,
    parse, receive, too_large,
};
use super::{ApiError, ErrorCode, PathParam, pages};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// Entries of the zip, whose directory `check_directory` also bounds in bytes.
const MAX_ENTRIES: u16 = 5000;
/// Pages one import makes.
/// shortcut: under what a space export takes (10,000). Raise it once a large import is measured.
const MAX_PAGES: usize = 1000;
/// Characters of a failed job's message, which names an entry of up to 64 KiB and stays in memory.
const MESSAGE_CHARS: usize = 300;

/// The pages to make, by the folders and the name that lead to them, so parents come first.
/// Each has the entry that holds its content, if one does.
type Tree = BTreeMap<Vec<String>, Option<String>>;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + PageStore + SpaceStore + AttachmentStore,
    A: Send + Sync + 'static,
{
    // No `DefaultBodyLimit`, as for an import of one page.
    Router::new()
        .route("/spaces/{space}/bulk-import", post(start))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

async fn start<S, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    RawQuery(query): RawQuery,
    body: Body,
) -> Result<(StatusCode, Json<JobBody>), ApiError>
where
    S: DocStore + PageStore + SpaceStore + AttachmentStore,
    A: Send + Sync + 'static,
{
    // An id needs no decoding.
    let parent_id = param(query.as_deref(), "parent_id").map(str::to_owned);
    let user = current.user.id;
    let found = state.store.space(&space, &user).await?;
    if found.ok_or_else(not_found)?.role < SpaceRole::Editor {
        return Err(ApiError::new(ErrorCode::Forbidden));
    }
    if let Some(parent) = &parent_id {
        let parent = state.store.page(parent, &user).await?;
        if parent.is_none_or(|parent| parent.space_id != space || parent.deleted_at.is_some()) {
            return Err(invalid("parent_id", "unknown"));
        }
    }
    let limit = state.attachments.max_import_bytes;
    let file = receive(&*state.files, body, limit).await?;
    let archive = Archive::open(file, MAX_ENTRIES, limit).await?;
    let tree = plan(&archive)?;

    let task = state.clone();
    let (owner, target) = (user.clone(), space.clone());
    run(&state, &user, &space, "space_import", async move {
        let made = Arc::new(Mutex::new(Vec::new()));
        // Nested, so that a panic trashes the pages as a failure does.
        let work = {
            let (state, made) = (task.clone(), made.clone());
            let (user, space) = (owner.clone(), target.clone());
            tokio::spawn(async move {
                write(&state, space, parent_id, &user, archive, tree, &made).await
            })
        };
        let message = match work.await {
            Ok(Ok(pages)) => return Progress::Imported(pages),
            Ok(Err(message)) => Some(message.chars().take(MESSAGE_CHARS).collect::<String>()),
            Err(error) => {
                warn!(%error, "a bulk import panicked");
                None
            }
        };
        warn!(?message, space = target, "a bulk import failed");
        let made = std::mem::take(&mut *made.lock().unwrap_or_else(PoisonError::into_inner));
        if !made.is_empty() {
            discard(&task, &made, &owner).await;
        }
        Progress::Failed(message)
    })
}

/// A name that no tool made on its own: not hidden, nor the folder macOS adds to its zips.
fn kept(segment: &str) -> bool {
    !segment.is_empty() && !segment.starts_with('.') && segment != "__MACOSX"
}

/// The tree of pages the zip describes, refused if it is over a limit.
fn plan(archive: &Archive) -> Result<Tree, ApiError> {
    let mut names: Vec<&String> = archive.entries.keys().collect();
    names.sort();
    let mut tree = Tree::new();
    for name in names {
        let Some((stem, extension)) = name.rsplit_once('.') else {
            continue;
        };
        if !matches!(extension.to_ascii_lowercase().as_str(), "md" | "markdown") {
            continue;
        }
        let mut path: Vec<String> = stem.split('/').map(str::to_owned).collect();
        // An export puts its files in `attachments/<id>/`, Markdown ones included.
        let exported =
            path.len() > 2 && path[0] == "attachments" && Uuid::try_parse(&path[1]).is_ok();
        if exported || !path.iter().all(|segment| kept(segment)) {
            continue;
        }
        if archive.entries[name].size > MAX_MARKDOWN_BYTES {
            return Err(too_large());
        }
        if path.len() > MAX_PAGE_DEPTH {
            return Err(invalid("file", "too_deep"));
        }
        for above in 1..path.len() {
            tree.entry(path[..above].to_vec()).or_insert(None);
        }
        // `a.md` beside `a.markdown`: the second one keeps its extension.
        if tree.get(&path).is_some_and(Option::is_some)
            && let Some(last) = path.last_mut()
        {
            *last = format!("{last}.{extension}");
        }
        tree.insert(path, Some(name.clone()));
        if tree.len() > MAX_PAGES {
            return Err(invalid("file", "too_many_pages"));
        }
    }
    if tree.is_empty() {
        return Err(invalid("file", "no_markdown"));
    }
    Ok(tree)
}

/// Makes every page of `tree`, listing them in `made`, then writes their contents. Answers how
/// many there are, or what to tell the starter.
async fn write<S, A>(
    state: &AppState<S, A>,
    space: String,
    parent_id: Option<String>,
    user: &str,
    mut archive: Archive,
    tree: Tree,
    made: &Mutex<Vec<String>>,
) -> Result<usize, String>
where
    S: DocStore + PageStore + AttachmentStore,
{
    // All the rows first, so a link between two files knows the page it leads to.
    let mut ids: BTreeMap<&[String], String> = BTreeMap::new();
    let mut linked = HashMap::new();
    for (path, entry) in &tree {
        let above = &path[..path.len() - 1];
        let parent = ids.get(above).cloned().or_else(|| parent_id.clone());
        let page = pages::add(
            state,
            space.clone(),
            parent,
            Placement::Last,
            user.to_owned(),
        )
        .await
        .map_err(|error| explain("A page cannot be made", &error))?;
        made.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(page.id.clone());
        if let Some(entry) = entry {
            linked.insert(entry.clone(), page.id.clone());
        }
        ids.insert(path, page.id);
    }

    // Files stored so far, by entry name.
    let mut stored = HashMap::new();
    for (path, entry) in &tree {
        let name = path.last().map(String::as_str).unwrap_or_default();
        let at = entry.as_deref().unwrap_or(name);
        let failed = |error: ApiError| explain(at, &error);
        let parsed = match entry {
            Some(entry) => {
                let markdown = archive.markdown(entry).await.map_err(failed)?;
                let links = Unbundle {
                    absolute: Absolute {
                        origin: state.http.public_url.origin(),
                    },
                    entries: &archive.entries,
                    dir: entry.rsplit_once('/').map_or("", |(dir, _)| dir),
                    pages: &linked,
                };
                parse(name, markdown, &links, Some(&archive)).map_err(failed)?
            }
            None => Parsed {
                title: name.chars().take(MAX_TITLE_CHARS).collect(),
                document: Document::default(),
                bytes: 0,
                attachments: Vec::new(),
            },
        };
        let max = state.attachments.max_upload_bytes;
        if parsed
            .attachments
            .iter()
            .any(|file| archive.entries[file].size > max)
        {
            return Err(format!("{at}: an attachment is over the upload limit"));
        }
        fill(
            state,
            &ids[path.as_slice()],
            user,
            &parsed,
            Some(&mut archive),
            &mut stored,
        )
        .await
        .map_err(failed)?;
    }
    Ok(tree.len())
}

/// What a failed job says: where it stopped, and why.
fn explain(at: &str, error: &ApiError) -> String {
    match error.fields.get("file") {
        Some(problem) => format!("{at}: {problem}"),
        None => format!("{at}: {}", error.message),
    }
}
