//! Space exports (docs/API.md#spaces): a job that writes a zip of the space's pages, folders for
//! pages with children, into an anonymous scratch file, which its starter downloads for an hour.
//! The jobs live in memory, not in the `jobs` table: the file does not survive a restart anyway
//! (`data/tmp` is emptied), so there is nothing a table could keep. One export runs at a time.

// async_zip 0.0.19 deprecates its seekable reader for a `read1` module it does not ship yet.
#![allow(deprecated)]

use std::collections::{HashMap, HashSet};
use std::fs::File as StdFile;
use std::io;
use std::os::unix::fs::FileExt;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
use axum::body::{Body, Bytes};
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use cadocs_content::yjs::read_page;
use cadocs_files::FileError;
use cadocs_store::{AccountStore, AttachmentStore, DocStore, PageStore, RevisionStore, SpaceStore};
use futures_util::{AsyncWriteExt as _, StreamExt};
use serde::Serialize;
use tokio::fs::File;
use tokio::sync::Semaphore;
use tokio::time::sleep;
use tracing::warn;
use uuid::Uuid;

use super::markdown::{
    Absolute, Bundle, HTML_TYPE, MARKDOWN_TYPE, ZIP_TYPE, download_headers, invalid, not_found,
    page_html, page_markdown, param, safe_name,
};
use super::revisions::Readable;
use super::{ApiError, ErrorCode, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// Pages one export takes, so its list of folders to visit stays small.
const MAX_PAGES: usize = 10_000;
/// Jobs held at once, running or waiting to be downloaded: each holds a file on the disk.
const MAX_JOBS: usize = 8;
/// Of those, what one user may hold, so one cannot take every slot.
const MAX_USER_JOBS: usize = 2;
/// How long an ended job stays, for its status and its file.
const KEEP: Duration = Duration::from_secs(3600);
/// Siblings read from the store at a time.
const BATCH: u32 = 100;
const CHUNK: usize = 64 * 1024;

enum Progress {
    Running,
    Done(Arc<StdFile>, u64, String),
    Failed,
}

struct Job {
    user: String,
    space: String,
    progress: Progress,
}

/// What the export jobs hold, shared by the routes and the tasks.
pub struct Exports {
    jobs: Mutex<HashMap<String, Job>>,
    /// One export at a time, whoever asks.
    turn: Semaphore,
}

impl Default for Exports {
    fn default() -> Self {
        Self {
            jobs: Mutex::default(),
            turn: Semaphore::new(1),
        }
    }
}

impl Exports {
    fn jobs(&self) -> std::sync::MutexGuard<'_, HashMap<String, Job>> {
        self.jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn settle(&self, id: &str, progress: Progress) {
        if let Some(job) = self.jobs().get_mut(id) {
            job.progress = progress;
        }
    }
}

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + PageStore + SpaceStore + AttachmentStore + RevisionStore,
    A: Send + Sync + 'static,
{
    Router::new()
        .route("/spaces/{space}/export", post(start))
        .route("/jobs/{id}", get(status))
        .route("/jobs/{id}/file", get(file))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

#[derive(Serialize)]
struct JobBody {
    id: String,
    kind: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
}

async fn start<S, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    RawQuery(query): RawQuery,
) -> Result<(StatusCode, Json<JobBody>), ApiError>
where
    S: DocStore + PageStore + SpaceStore + AttachmentStore + RevisionStore,
    A: Send + Sync + 'static,
{
    let html = match param(query.as_deref(), "format") {
        None | Some("md") => false,
        Some("html") => true,
        Some(_) => return Err(invalid("format", "invalid")),
    };
    let user = current.user.id;
    let found = state
        .store
        .space(&space, &user)
        .await?
        .ok_or_else(not_found)?;
    let id = Uuid::now_v7().to_string();
    {
        let mut jobs = state.exports.jobs();
        if jobs.len() >= MAX_JOBS {
            return Err(ApiError::new(ErrorCode::RateLimited)
                .with_message("Too many exports are running or waiting to be downloaded"));
        }
        if jobs.values().filter(|job| job.user == user).count() >= MAX_USER_JOBS {
            return Err(ApiError::new(ErrorCode::RateLimited)
                .with_message("You already have exports running or waiting to be downloaded"));
        }
        let job = Job {
            user: user.clone(),
            space: space.clone(),
            progress: Progress::Running,
        };
        jobs.insert(id.clone(), job);
    }
    let task = state.clone();
    let job = id.clone();
    tokio::spawn(async move {
        let name = format!("{}.zip", safe_name(&found.name, "space"));
        // Nested, so that a panic ends as a failed job and not as one that holds a slot forever.
        let work = {
            let task = task.clone();
            let (space, user) = (space.clone(), user.clone());
            tokio::spawn(async move {
                let _turn = task
                    .exports
                    .turn
                    .acquire()
                    .await
                    .map_err(io::Error::other)?;
                write(&task, &space, &user, html).await
            })
        };
        let outcome = work
            .await
            .unwrap_or_else(|error| Err(io::Error::other(error)));
        match outcome {
            Ok((file, size)) => task
                .exports
                .settle(&job, Progress::Done(Arc::new(file), size, name)),
            Err(error) => {
                warn!(%error, space, "a space export failed");
                task.exports.settle(&job, Progress::Failed);
            }
        }
        sleep(KEEP).await;
        task.exports.jobs().remove(&job);
    });
    let body = JobBody {
        id,
        kind: "space_export",
        status: "running",
        size: None,
    };
    Ok((StatusCode::ACCEPTED, Json(body)))
}

/// A job as its starter sees it: `404` for anyone else, and once they lost the space.
async fn own<S: SpaceStore, A>(
    state: &AppState<S, A>,
    user: &str,
    id: &str,
) -> Result<Option<(&'static str, Option<(Arc<StdFile>, u64, String)>)>, ApiError> {
    let (space, outcome) = {
        let jobs = state.exports.jobs();
        let Some(job) = jobs.get(id).filter(|job| job.user == user) else {
            return Err(not_found());
        };
        let outcome = match &job.progress {
            Progress::Running => ("running", None),
            Progress::Failed => ("failed", None),
            Progress::Done(file, size, name) => ("done", Some((file.clone(), *size, name.clone()))),
        };
        (job.space.clone(), outcome)
    };
    state
        .store
        .space(&space, user)
        .await?
        .ok_or_else(not_found)?;
    Ok(Some(outcome))
}

async fn status<S: SpaceStore, A: Send + Sync + 'static>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<Json<JobBody>, ApiError> {
    let found = own(&state, &current.user.id, &id).await?;
    let (status, file) = found.ok_or_else(not_found)?;
    let size = file.map(|(_, size, _)| size);
    Ok(Json(JobBody {
        id,
        kind: "space_export",
        status,
        size,
    }))
}

async fn file<S: SpaceStore, A: Send + Sync + 'static>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<Response, ApiError> {
    let found = own(&state, &current.user.id, &id).await?;
    let Some((_, Some((file, size, name)))) = found else {
        return Err(ApiError::new(ErrorCode::Conflict).with_message("The export is not done"));
    };
    let chunks = futures_util::stream::unfold(0u64, move |at| {
        let file = file.clone();
        async move {
            if at >= size {
                return None;
            }
            let want = CHUNK.min((size - at) as usize);
            let read = tokio::task::spawn_blocking(move || {
                let mut buffer = vec![0; want];
                file.read_exact_at(&mut buffer, at)
                    .map(|()| Bytes::from(buffer))
            })
            .await;
            match read {
                Ok(Ok(bytes)) => Some((Ok(bytes), at + want as u64)),
                Ok(Err(error)) => Some((Err(error), size)),
                Err(error) => Some((Err(io::Error::other(error)), size)),
            }
        }
    });
    let headers = download_headers(ZIP_TYPE, &name);
    Ok((headers, Body::from_stream(chunks)).into_response())
}

/// One folder of the tree still to read: its pages, then the folders of those that have some.
struct Level {
    parent: Option<String>,
    dir: String,
    after: Option<String>,
    /// Lower case stems of what the folder holds, so that two titles never share a file.
    stems: HashSet<String>,
}

fn unique(stems: &mut HashSet<String>, stem: String) -> String {
    let mut candidate = stem.clone();
    let mut n = 1;
    while !stems.insert(candidate.to_lowercase()) {
        n += 1;
        candidate = format!("{stem} ({n})");
    }
    candidate
}

fn zip_error(error: async_zip::error::ZipError) -> io::Error {
    io::Error::other(error)
}

/// The zip of the space, in a scratch file, and its size.
async fn write<S, A>(
    state: &AppState<S, A>,
    space: &str,
    user: &str,
    html: bool,
) -> io::Result<(StdFile, u64)>
where
    S: DocStore + PageStore + SpaceStore + AttachmentStore + RevisionStore,
    A: Send + Sync + 'static,
{
    let scratch = File::from_std(state.files.scratch()?);
    let mut zip = ZipFileWriter::with_tokio(scratch).force_no_zip64();
    let (extension, _kind) = if html {
        ("html", HTML_TYPE)
    } else {
        ("md", MARKDOWN_TYPE)
    };
    let absolute = Absolute {
        origin: state.http.public_url.origin(),
    };
    // Attachments already written, by id, with the name of their entry.
    let mut written: HashMap<String, String> = HashMap::new();
    let mut count = 0;
    let mut levels = vec![Level {
        parent: None,
        dir: String::new(),
        after: None,
        // Where the attachments go.
        stems: HashSet::from(["attachments".to_owned()]),
    }];
    while let Some(mut level) = levels.pop() {
        loop {
            let batch = state
                .store
                .tree(
                    space,
                    level.parent.as_deref(),
                    user,
                    level.after.as_deref(),
                    BATCH,
                )
                .await
                .map_err(io::Error::other)?;
            let Some(last) = batch.last() else { break };
            level.after = Some(last.position.clone());
            for page in batch {
                count += 1;
                if count > MAX_PAGES {
                    return Err(io::Error::other("the space holds too many pages"));
                }
                // A page that cannot be read (corrupt, or trashed meanwhile) must not stop the rest.
                let (title, document) = match state.registry.read(&page.id, read_page).await {
                    Ok(read) => read,
                    Err(error) => {
                        warn!(%error, page = page.id, "an exported page cannot be read");
                        continue;
                    }
                };
                let titles = Readable::read(&*state.store, user, &page.id, &document)
                    .await
                    .map_err(|error| io::Error::other(format!("{error:?}")))?;
                let mut files = Vec::new();
                for id in document.attachments() {
                    if let Some(name) = written.get(&id) {
                        files.push((id, name.clone()));
                        continue;
                    }
                    let Some(found) = state
                        .store
                        .attachment(&id, user)
                        .await
                        .map_err(io::Error::other)?
                    else {
                        continue;
                    };
                    let name = safe_name(&found.file_name, "file");
                    let mut body = match state.files.get(&id).await {
                        Ok(body) => body,
                        Err(FileError::NotFound) => {
                            warn!(id, "an exported attachment has a row and no file");
                            continue;
                        }
                        Err(error) => return Err(io::Error::other(error)),
                    };
                    let entry = format!("attachments/{id}/{name}");
                    let builder = ZipEntryBuilder::new(entry.into(), Compression::Stored);
                    let mut writer = zip.write_entry_stream(builder).await.map_err(zip_error)?;
                    while let Some(chunk) = body.bytes.next().await {
                        writer.write_all(&chunk?).await?;
                    }
                    writer.close().await.map_err(zip_error)?;
                    written.insert(id.clone(), name.clone());
                    files.push((id, name));
                }
                let up = "../".repeat(level.dir.matches('/').count());
                let links = Bundle {
                    absolute: Absolute {
                        origin: absolute.origin,
                    },
                    files: &files,
                    up: &up,
                };
                let text = if html {
                    page_html(&title, &document, &titles, &links)
                } else {
                    page_markdown(&title, document, &titles, &links)
                };
                let stem = unique(&mut level.stems, safe_name(&title, "Untitled"));
                let entry = format!("{}{stem}.{extension}", level.dir);
                let builder = ZipEntryBuilder::new(entry.into(), Compression::Stored);
                zip.write_entry_whole(builder, text.as_bytes())
                    .await
                    .map_err(zip_error)?;
                if page.has_children {
                    levels.push(Level {
                        parent: Some(page.id),
                        dir: format!("{}{stem}/", level.dir),
                        after: None,
                        stems: HashSet::new(),
                    });
                }
            }
        }
    }
    let mut file = zip.close().await.map_err(zip_error)?.into_inner();
    tokio::io::AsyncWriteExt::flush(&mut file).await?;
    let size = file.metadata().await?.len();
    Ok((file.into_std().await, size))
}
