//! Archive jobs, which bulk imports (`imports.rs`) share, and space exports
//! (docs/API.md#spaces): a job that writes a zip of the space's pages, folders for
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
/// Jobs holding a slot at once, running or waiting to be downloaded: each holds a file on the disk.
const MAX_JOBS: usize = 8;
/// Of those, what one user may hold, so one cannot take every slot.
const MAX_USER_JOBS: usize = 2;
/// Ended jobs with no file kept per user, for their status.
const MAX_USER_ENDED: usize = 8;
/// How long an ended job stays, for its status and its file.
const KEEP: Duration = Duration::from_secs(3600);
/// Siblings read from the store at a time.
const BATCH: u32 = 100;
const CHUNK: usize = 64 * 1024;

const EXPORT: &str = "space_export";

#[derive(Clone)]
pub(super) enum Progress {
    Running,
    /// An export, ready to download.
    Done(Arc<StdFile>, u64, String),
    /// An import, with the number of pages it made.
    Imported(usize),
    /// With what to tell the starter, if anything.
    Failed(Option<String>),
}

struct Job {
    user: String,
    space: String,
    kind: &'static str,
    progress: Progress,
}

/// What the archive jobs hold, shared by the routes and the tasks.
pub struct Exports {
    jobs: Mutex<HashMap<String, Job>>,
    /// One archive job at a time, whoever asks.
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
pub(super) struct JobBody {
    id: String,
    kind: &'static str,
    status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pages: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    message: Option<String>,
}

impl JobBody {
    fn new(id: String, kind: &'static str, progress: &Progress) -> Self {
        let (status, size, pages, message) = match progress {
            Progress::Running => ("running", None, None, None),
            Progress::Done(_, size, _) => ("done", Some(*size), None, None),
            Progress::Imported(pages) => ("done", None, Some(*pages), None),
            Progress::Failed(message) => ("failed", None, None, message.clone()),
        };
        Self {
            id,
            kind,
            status,
            size,
            pages,
            message,
        }
    }
}

/// Runs `work` as the job `kind` of `user` on `space`, in its turn, and answers its first body.
/// Refused when too many jobs are held.
pub(super) fn run<S, A>(
    state: &AppState<S, A>,
    user: &str,
    space: &str,
    kind: &'static str,
    work: impl Future<Output = Progress> + Send + 'static,
) -> Result<(StatusCode, Json<JobBody>), ApiError>
where
    S: Send + Sync + 'static,
    A: Send + Sync + 'static,
{
    let id = Uuid::now_v7().to_string();
    {
        let mut jobs = state.exports.jobs();
        // An ended job with no file takes no slot, and the oldest of a user's go past a few, so
        // the map stays bounded however fast they fail.
        let mut ended: Vec<String> = jobs
            .iter()
            .filter(|(_, job)| {
                job.user == user
                    && matches!(job.progress, Progress::Failed(_) | Progress::Imported(_))
            })
            .map(|(id, _)| id.clone())
            .collect();
        // Version 7 ids sort by age.
        ended.sort_unstable_by(|a, b| b.cmp(a));
        for old in ended.iter().skip(MAX_USER_ENDED - 1) {
            jobs.remove(old);
        }
        let held = || {
            jobs.values()
                .filter(|job| matches!(job.progress, Progress::Running | Progress::Done(..)))
        };
        if held().count() >= MAX_JOBS {
            return Err(ApiError::new(ErrorCode::RateLimited)
                .with_message("Too many exports or imports are running or waiting"));
        }
        if held().filter(|job| job.user == user).count() >= MAX_USER_JOBS {
            return Err(ApiError::new(ErrorCode::RateLimited)
                .with_message("You already have exports or imports running or waiting"));
        }
        let job = Job {
            user: user.to_owned(),
            space: space.to_owned(),
            kind,
            progress: Progress::Running,
        };
        jobs.insert(id.clone(), job);
    }
    let exports = state.exports.clone();
    let job = id.clone();
    tokio::spawn(async move {
        // Nested, so that a panic ends as a failed job and not as one that holds a slot forever.
        let turn = exports.clone();
        let done = tokio::spawn(async move {
            match turn.turn.acquire().await {
                Ok(_turn) => work.await,
                Err(_) => Progress::Failed(None),
            }
        });
        let progress = done.await.unwrap_or_else(|error| {
            warn!(%error, kind, "an archive job panicked");
            Progress::Failed(None)
        });
        exports.settle(&job, progress);
        sleep(KEEP).await;
        exports.jobs().remove(&job);
    });
    let body = JobBody::new(id, kind, &Progress::Running);
    Ok((StatusCode::ACCEPTED, Json(body)))
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
    let task = state.clone();
    let (owner, target) = (user.clone(), space.clone());
    run(&state, &user, &space, EXPORT, async move {
        let name = format!("{}.zip", safe_name(&found.name, "space"));
        match write(&task, &target, &owner, html).await {
            Ok((file, size)) => Progress::Done(Arc::new(file), size, name),
            Err(error) => {
                warn!(%error, space = target, "a space export failed");
                Progress::Failed(None)
            }
        }
    })
}

/// A job as its starter sees it: `404` for anyone else, and once they lost the space.
async fn own<S: SpaceStore, A>(
    state: &AppState<S, A>,
    user: &str,
    id: &str,
) -> Result<(&'static str, Progress), ApiError> {
    let (space, kind, progress) = {
        let jobs = state.exports.jobs();
        let Some(job) = jobs.get(id).filter(|job| job.user == user) else {
            return Err(not_found());
        };
        (job.space.clone(), job.kind, job.progress.clone())
    };
    state
        .store
        .space(&space, user)
        .await?
        .ok_or_else(not_found)?;
    Ok((kind, progress))
}

async fn status<S: SpaceStore, A: Send + Sync + 'static>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<Json<JobBody>, ApiError> {
    let (kind, progress) = own(&state, &current.user.id, &id).await?;
    Ok(Json(JobBody::new(id, kind, &progress)))
}

async fn file<S: SpaceStore, A: Send + Sync + 'static>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<Response, ApiError> {
    let (kind, progress) = own(&state, &current.user.id, &id).await?;
    if kind != EXPORT {
        return Err(not_found());
    }
    let Progress::Done(file, size, name) = progress else {
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
