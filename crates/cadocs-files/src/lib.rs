//! Attachment bytes, stored by opaque id (ADR 0005). Names and types live in the database, so
//! nothing a user typed ever reaches a path.
//!
//! The trait is object safe, so the server holds an `Arc<dyn FileStore>` and tests can put a
//! failing store in its place. The S3 store (ADR 0020) is behind the `s3` feature.

#[cfg(feature = "s3")]
mod s3;

#[cfg(feature = "s3")]
pub use s3::{Downloads, S3Config, S3FileStore, valid_prefix};

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::time::SystemTime;

use bytes::{Bytes, BytesMut};
use futures_util::{Stream, StreamExt, stream};
use sha2::{Digest, Sha256};
use tokio::fs::{self, File};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type ByteStream<'a> = Pin<Box<dyn Stream<Item = io::Result<Bytes>> + Send + 'a>>;
pub type FileList = Pin<Box<dyn Stream<Item = Result<Vec<StoredFile>, FileError>> + Send>>;

/// Files a listing returns at a time.
pub const LIST_BATCH: usize = 100;
/// What a download reads at a time.
const READ_CHUNK: usize = 64 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("the file is over {0} bytes")]
    TooLarge(u64),
    /// The body failed, not the storage: the client went away, for instance.
    #[error("the upload was cut short: {0}")]
    Body(io::Error),
    #[error("no such file")]
    NotFound,
    #[error("not a file id")]
    InvalidId,
    #[error("file storage error: {0}")]
    Io(#[from] io::Error),
}

/// A file once it is in place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stored {
    pub size: u64,
    /// Lowercase hex.
    pub sha256: String,
}

pub struct FileBody {
    pub size: u64,
    pub bytes: ByteStream<'static>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredFile {
    pub id: String,
    pub modified: SystemTime,
}

/// What a browser must be told about a file it fetches straight from the storage.
#[derive(Debug, Clone, Copy)]
pub struct Download<'a> {
    pub content_type: &'a str,
    /// A whole `Content-Disposition` value.
    pub disposition: &'a str,
}

pub trait FileStore: Send + Sync + 'static {
    /// Streams `body` into place under `id`, hashing it on the way. Nothing is kept of a body
    /// over `max_bytes` or one that fails, and a file only shows under `id` once complete.
    fn put<'a>(
        &'a self,
        id: &'a str,
        body: ByteStream<'a>,
        max_bytes: u64,
    ) -> BoxFuture<'a, Result<Stored, FileError>>;

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<FileBody, FileError>>;

    /// Deleting a missing file succeeds.
    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), FileError>>;

    /// Every stored file, in no order, at most [`LIST_BATCH`] at a time, and never an empty batch.
    fn list(&self) -> FileList;

    /// A link the browser can fetch the file from itself, once the caller checked its access.
    /// `None` when the server must stream the file.
    fn download_url(&self, _id: &str, _download: &Download<'_>) -> Option<String> {
        None
    }

    /// The origin `download_url` links point to, for the CSP.
    fn download_origin(&self) -> Option<String> {
        None
    }

    /// An anonymous file for work in progress, such as an import being read, gone once closed.
    fn scratch(&self) -> io::Result<std::fs::File> {
        tempfile::tempfile()
    }
}

/// Files under `<data>/files`, two levels of directories down, with uploads in progress under
/// `<data>/tmp`.
pub struct LocalFileStore {
    files: PathBuf,
    tmp: PathBuf,
}

impl LocalFileStore {
    /// Empties `tmp`: what is there was cut short by a crash. Call it once the data folder is
    /// locked, since another server could be writing there.
    pub fn open(data_dir: &Path) -> io::Result<Self> {
        let files = data_dir.join("files");
        std::fs::create_dir_all(&files)?;
        let tmp = empty_tmp(data_dir)?;
        Ok(Self { files, tmp })
    }

    fn path(&self, id: &str) -> Result<PathBuf, FileError> {
        Ok(self.files.join(relative_path(id)?))
    }

    async fn put_file(
        &self,
        id: &str,
        body: ByteStream<'_>,
        max_bytes: u64,
    ) -> Result<Stored, FileError> {
        let target = self.path(id)?;
        let (temp, stored) = receive_temp(&self.tmp, id, body, max_bytes).await?;
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).await?;
        }
        fs::rename(&temp.0, &target).await?;
        Ok(stored)
    }

    async fn get_file(&self, id: &str) -> Result<FileBody, FileError> {
        let path = self.path(id)?;
        let file = File::open(&path).await.map_err(missing)?;
        let size = file.metadata().await?.len();
        Ok(FileBody {
            size,
            bytes: read_stream(file),
        })
    }

    async fn delete_file(&self, id: &str) -> Result<(), FileError> {
        match fs::remove_file(self.path(id)?).await {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error.into()),
            _ => Ok(()),
        }
    }
}

impl FileStore for LocalFileStore {
    fn put<'a>(
        &'a self,
        id: &'a str,
        body: ByteStream<'a>,
        max_bytes: u64,
    ) -> BoxFuture<'a, Result<Stored, FileError>> {
        Box::pin(self.put_file(id, body, max_bytes))
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<FileBody, FileError>> {
        Box::pin(self.get_file(id))
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(self.delete_file(id))
    }

    /// Under `<data>/tmp`, on the disk of the data folder. Unlinked at once, so a crash leaves
    /// nothing behind.
    fn scratch(&self) -> io::Result<std::fs::File> {
        tempfile::tempfile_in(&self.tmp)
    }

    fn list(&self) -> FileList {
        // A blocking walk hands its batches over a channel of one, so a listing holds one batch
        // in memory, and stops when the reader goes away.
        let (send, receive) = mpsc::channel(1);
        let files = self.files.clone();
        tokio::task::spawn_blocking(move || {
            if let Err(error) = walk(&files, &send) {
                let _ = send.blocking_send(Err(error.into()));
            }
        });
        Box::pin(stream::unfold(receive, |mut receive| async move {
            receive.recv().await.map(|batch| (batch, receive))
        }))
    }
}

/// Empties `<data>/tmp`, which holds uploads in progress: what is there was cut short by a crash.
fn empty_tmp(data_dir: &Path) -> io::Result<PathBuf> {
    let tmp = data_dir.join("tmp");
    match std::fs::remove_dir_all(&tmp) {
        Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    std::fs::create_dir_all(&tmp)?;
    Ok(tmp)
}

/// Where a file lives under `files/`, and under the prefix on S3. The fan-out comes from the end
/// of the id: a UUIDv7 starts with its date, ends at random.
fn relative_path(id: &str) -> Result<String, FileError> {
    if !is_id(id) {
        return Err(FileError::InvalidId);
    }
    let end = id.len();
    Ok(format!("{}/{}/{id}", &id[end - 2..], &id[end - 4..end - 2]))
}

/// Streams `body` into a new file of `tmp`, which goes when the returned guard drops.
async fn receive_temp(
    tmp: &Path,
    id: &str,
    body: ByteStream<'_>,
    max_bytes: u64,
) -> Result<(Temp, Stored), FileError> {
    let temp = Temp(tmp.join(format!("{id}.part")));
    let file = File::create_new(&temp.0).await?;
    let stored = receive(file, body, max_bytes).await?;
    Ok((temp, stored))
}

/// A file's bytes, [`READ_CHUNK`] at a time.
fn read_stream(file: File) -> ByteStream<'static> {
    Box::pin(stream::unfold(Some(file), |file| async move {
        let mut file = file?;
        let mut buffer = BytesMut::with_capacity(READ_CHUNK);
        match file.read_buf(&mut buffer).await {
            Ok(0) => None,
            Ok(_) => Some((Ok(buffer.freeze()), Some(file))),
            Err(error) => Some((Err(error), None)),
        }
    }))
}

/// A file in `tmp`, removed when this drops: on an error, and when the server drops a request
/// whose client went away, which cancels the upload at any await. After the rename it is gone,
/// and removing it fails quietly.
struct Temp(PathBuf);

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

type BatchSender = mpsc::Sender<Result<Vec<StoredFile>, FileError>>;

/// `None` for what was deleted while the walk ran: a delete or the cleanup itself can remove a
/// file or an emptied folder at any time.
fn present<T>(found: io::Result<T>) -> io::Result<Option<T>> {
    match found {
        Ok(found) => Ok(Some(found)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn walk(files: &Path, send: &BatchSender) -> io::Result<()> {
    let mut batch = Vec::with_capacity(LIST_BATCH);
    for first in std::fs::read_dir(files)? {
        let first = first?;
        if !first.file_type()?.is_dir() {
            continue;
        }
        let Some(seconds) = present(std::fs::read_dir(first.path()))? else {
            continue;
        };
        for second in seconds {
            let second = second?;
            if !second.file_type()?.is_dir() {
                continue;
            }
            let Some(entries) = present(std::fs::read_dir(second.path()))? else {
                continue;
            };
            for entry in entries {
                let entry = entry?;
                let Ok(id) = entry.file_name().into_string() else {
                    continue;
                };
                let Some(metadata) = present(entry.metadata())? else {
                    continue;
                };
                if !metadata.is_file() || !is_id(&id) {
                    continue;
                }
                batch.push(StoredFile {
                    id,
                    modified: metadata.modified()?,
                });
                if batch.len() == LIST_BATCH {
                    let full = std::mem::replace(&mut batch, Vec::with_capacity(LIST_BATCH));
                    if send.blocking_send(Ok(full)).is_err() {
                        return Ok(());
                    }
                }
            }
        }
    }
    if !batch.is_empty() {
        let _ = send.blocking_send(Ok(batch));
    }
    Ok(())
}

/// Writes `body` to `file`, hashing it, and syncs it to disk. Public for the test that writes
/// to `/dev/full`.
pub async fn receive(
    mut file: File,
    mut body: ByteStream<'_>,
    max_bytes: u64,
) -> Result<Stored, FileError> {
    let mut hasher = Sha256::new();
    let mut size: u64 = 0;
    while let Some(chunk) = body.next().await {
        let chunk = chunk.map_err(FileError::Body)?;
        size = size.saturating_add(chunk.len() as u64);
        if size > max_bytes {
            return Err(FileError::TooLarge(max_bytes));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    // The cleanup judges a file with no row by its date: a client that held the end of its body
    // for an hour must not leave a file that looks that old before its row is written.
    let file = file.into_std().await;
    tokio::task::spawn_blocking(move || {
        file.set_modified(SystemTime::now())?;
        file.sync_all()
    })
    .await
    .map_err(io::Error::other)??;
    Ok(Stored {
        size,
        sha256: hex(&hasher.finalize()),
    })
}

/// Lowercase hex digits and dashes, ending with four digits, as a UUID does.
fn is_id(id: &str) -> bool {
    let digit = |byte: u8| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte);
    id.len() >= 4
        && id.len() <= 64
        && id.bytes().all(|byte| digit(byte) || byte == b'-')
        && id.bytes().rev().take(4).all(digit)
}

fn missing(error: io::Error) -> FileError {
    if error.kind() == io::ErrorKind::NotFound {
        FileError::NotFound
    } else {
        FileError::Io(error)
    }
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(DIGITS[usize::from(byte >> 4)] as char);
        out.push(DIGITS[usize::from(byte & 0xf)] as char);
    }
    out
}
