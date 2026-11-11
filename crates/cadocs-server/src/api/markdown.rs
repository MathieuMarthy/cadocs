//! Markdown export and import of one page (docs/API.md#pages). An export without attachments is
//! one file whose links are absolute, so they work wherever it is opened. With attachments it is
//! a zip, written as it is sent: the page as `<title>.md`, and each attachment the reader can
//! download as `attachments/<id>/<name>`, where its links point. An import reads either back into
//! a new page, with copies of the zip's attachments, and writes it through the room.
//!
//! Zip entries are stored, not compressed: attachments mostly are already, and this keeps a
//! decompressor out of the binary. An import takes its body to a scratch file first, since a zip
//! is read from its end, and bounds the zip's directory before parsing it.

// async_zip 0.0.19 deprecates its seekable reader for a `read1` module it does not ship yet.
#![allow(deprecated)]

use std::collections::HashMap;
use std::io::{self, SeekFrom};

use async_zip::base::read::seek::ZipFileReader;
use async_zip::base::write::ZipFileWriter;
use async_zip::error::ZipError;
use async_zip::{Compression, ZipEntryBuilder};
use axum::body::{Body, Bytes};
use axum::extract::{RawQuery, State};
use axum::http::header::{CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_TYPE};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use cadocs_content::ast::{Block, Document, Inline};
use cadocs_content::markdown::{from_markdown_with, to_markdown_with};
use cadocs_content::yjs::{read_page, replace_page};
use cadocs_content::{ATTACHMENT_PATH, Links, PAGE_PATH, relative_attachment_id, relative_page_id};
use cadocs_core::permissions::SpaceRole;
use cadocs_files::{FileError, FileStore, Stored};
use cadocs_store::{
    AccountStore, AttachmentStore, DocStore, MAX_TITLE_CHARS, NewAttachment, PageStore, Placement,
    RevisionStore, SpaceStore, now_ms,
};
use futures_util::{AsyncWriteExt as _, StreamExt, TryStreamExt};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::oneshot;
use tokio_util::compat::{Compat, FuturesAsyncReadCompatExt};
use tokio_util::io::ReaderStream;
use tracing::warn;
use uuid::Uuid;

use super::attachments::{MAX_NAME_CHARS, disposition, file_error, header};
use super::pages::{self, PageBody};
use super::{ApiError, ErrorCode, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// The Markdown of one page, about what a page socket takes in one frame.
const MAX_MARKDOWN_BYTES: u64 = 2 * 1024 * 1024;
/// Entries of an imported zip, and the size of their directory, which is parsed into memory.
const MAX_ENTRIES: u16 = 1000;
const MAX_DIRECTORY_BYTES: u32 = 1024 * 1024;
const EOCD_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
const ZIP64_LOCATOR_SIGNATURE: [u8; 4] = [0x50, 0x4b, 0x06, 0x07];
const EOCD_BYTES: usize = 22;
const ATTACHMENTS_DIR: &str = "attachments/";
/// Between the zip writer and the response.
const PIPE_BYTES: usize = 64 * 1024;
/// The longest name an export gives a file, in characters.
const ENTRY_CHARS: usize = 100;
const MARKDOWN_TYPE: &str = "text/markdown; charset=utf-8";
const ZIP_TYPE: &str = "application/zip";
/// What a path segment keeps unescaped: RFC 3986's unreserved characters.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + PageStore + SpaceStore + AttachmentStore + RevisionStore,
    A: Send + Sync + 'static,
{
    // No `DefaultBodyLimit`: an import body is never read whole, and its limits apply as it
    // streams.
    Router::new()
        .route("/pages/{page}/export", get(export))
        .route("/spaces/{space}/import", post(import))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

fn invalid(field: &'static str, problem: &'static str) -> ApiError {
    ApiError::new(ErrorCode::ValidationFailed).with_field(field, problem)
}

fn not_found() -> ApiError {
    ApiError::new(ErrorCode::NotFound)
}

fn too_large() -> ApiError {
    ApiError::new(ErrorCode::TooLarge).with_message("The file is over the import limit")
}

/// The raw value of `name` in a query.
fn param<'q>(query: Option<&'q str>, name: &str) -> Option<&'q str> {
    query?
        .split('&')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

/// A query value, where `+` is a space, as `URLSearchParams` writes it.
fn decoded(raw: &str) -> Option<String> {
    let raw = raw.replace('+', " ");
    let value = percent_decode_str(&raw).decode_utf8().ok()?;
    Some(value.into_owned())
}

/// Addresses on this server, absolute, for a file opened anywhere. Reading, it also knows the
/// relative ones.
struct Absolute<'a> {
    origin: &'a str,
}

impl Absolute<'_> {
    fn local<'h>(&self, href: &'h str) -> &'h str {
        href.strip_prefix(self.origin).unwrap_or(href)
    }
}

impl Links for Absolute<'_> {
    fn attachment(&self, id: &str) -> String {
        format!("{}{ATTACHMENT_PATH}{id}", self.origin)
    }

    fn page(&self, id: &str) -> String {
        format!("{}{PAGE_PATH}{id}", self.origin)
    }

    fn attachment_id(&self, href: &str) -> Option<String> {
        relative_attachment_id(self.local(href))
    }

    fn page_id(&self, href: &str) -> Option<String> {
        relative_page_id(self.local(href))
    }
}

/// An export zip: the attachments it holds are linked inside it, the others on this server.
struct Bundle<'a> {
    absolute: Absolute<'a>,
    /// The zip's attachments, with the name of their entry, in `ATTACHMENTS_DIR/<id>/`.
    files: &'a [(String, String)],
}

impl Links for Bundle<'_> {
    fn attachment(&self, id: &str) -> String {
        match self.files.iter().find(|(file, _)| file == id) {
            Some((_, name)) => {
                let name = utf8_percent_encode(name, SEGMENT);
                format!("{ATTACHMENTS_DIR}{id}/{name}")
            }
            None => self.absolute.attachment(id),
        }
    }

    fn page(&self, id: &str) -> String {
        self.absolute.page(id)
    }
}

/// An imported zip: a link to one of its entries is an attachment, known by the entry's name
/// until it is stored. Other links read as on this server.
struct Unbundle<'a> {
    absolute: Absolute<'a>,
    entries: &'a HashMap<String, Entry>,
}

impl Links for Unbundle<'_> {
    fn attachment_id(&self, href: &str) -> Option<String> {
        let name = percent_decode_str(href).decode_utf8().ok();
        match name.filter(|name| self.entries.contains_key(name.as_ref())) {
            Some(name) => Some(name.into_owned()),
            None => self.absolute.attachment_id(href),
        }
    }

    fn page_id(&self, href: &str) -> Option<String> {
        self.absolute.page_id(href)
    }
}

/// A name that is safe as a file name on any system: no separators, reserved or control
/// characters, no dot or space at its edges, and not too long.
fn safe_name(name: &str, fallback: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .take(ENTRY_CHARS)
        .collect();
    let trimmed = cleaned.trim_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

/// The page as Markdown: its title as the first heading, then its content.
fn page_markdown(
    title: &str,
    mut document: Document,
    titles: &dyn Fn(&str) -> Option<String>,
    links: &dyn Links,
) -> String {
    let heading = Block::Heading {
        level: 1,
        content: vec![Inline::Text {
            text: title.to_owned(),
            marks: Default::default(),
        }],
    };
    document.blocks.insert(0, heading);
    to_markdown_with(&document, &titles, links)
}

/// Takes the first block as the title when it is a top level heading, as an export writes it.
fn take_title(document: &mut Document) -> Option<String> {
    let Some(Block::Heading { level: 1, content }) = document.blocks.first() else {
        return None;
    };
    let title = content
        .iter()
        .map(|inline| match inline {
            Inline::Text { text, .. } => text.as_str(),
            Inline::HardBreak => " ",
            Inline::PageLink { .. } => "",
        })
        .collect();
    document.blocks.remove(0);
    Some(title)
}

fn download_headers(kind: &'static str, name: &str) -> [(axum::http::HeaderName, HeaderValue); 3] {
    [
        (CONTENT_TYPE, HeaderValue::from_static(kind)),
        (CONTENT_DISPOSITION, header(&disposition(false, name))),
        (CACHE_CONTROL, HeaderValue::from_static("private, no-store")),
    ]
}

async fn export<S: DocStore + PageStore + AttachmentStore + RevisionStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
    RawQuery(query): RawQuery,
) -> Result<Response, ApiError> {
    let query = query.as_deref();
    if !matches!(param(query, "format"), None | Some("md")) {
        return Err(invalid("format", "invalid"));
    }
    let bundle = match param(query, "attachments") {
        None | Some("0") => false,
        Some("1") => true,
        Some(_) => return Err(invalid("attachments", "invalid")),
    };
    let user = &current.user.id;
    state.store.page(&page, user).await?.ok_or_else(not_found)?;
    let (title, document) = state.registry.read(&page, read_page).await?;
    let linked = document.page_links();
    let titles: HashMap<String, String> = state
        .store
        .readable_titles(user, &linked)
        .await?
        .into_iter()
        .collect();
    let titles = |id: &str| titles.get(id).cloned();
    let absolute = Absolute {
        origin: state.http.public_url.origin(),
    };
    let stem = safe_name(&title, "page");
    if !bundle {
        let markdown = page_markdown(&title, document, &titles, &absolute);
        let headers = download_headers(MARKDOWN_TYPE, &format!("{stem}.md"));
        return Ok((headers, markdown).into_response());
    }

    let mut files = Vec::new();
    for id in document.attachments() {
        if let Some(found) = state.store.attachment(&id, user).await? {
            files.push((id, safe_name(&found.file_name, "file")));
        }
    }
    let links = Bundle {
        absolute,
        files: &files,
    };
    let markdown = page_markdown(&title, document, &titles, &links);
    let (pipe, reader) = tokio::io::duplex(PIPE_BYTES);
    let (done, finished) = oneshot::channel();
    let store = state.files.clone();
    let entry = format!("{stem}.md");
    tokio::spawn(async move {
        let written = write_zip(pipe, &entry, markdown, &files, &*store).await;
        if let Err(error) = &written {
            // Most often the client went away.
            warn!(%error, "an export stopped before its end");
        }
        let _ = done.send(written.is_ok());
    });
    // A zip cut short must not look complete: the body then ends in an error, which drops the
    // connection.
    let end = futures_util::stream::once(async move {
        match finished.await {
            Ok(true) => None,
            _ => Some(Err(io::Error::other("the export failed"))),
        }
    })
    .filter_map(std::future::ready);
    let body = Body::from_stream(ReaderStream::new(reader).chain(end));
    let headers = download_headers(ZIP_TYPE, &format!("{stem}.zip"));
    Ok((headers, body).into_response())
}

/// Writes the zip of an export into `pipe`, one attachment at a time. An attachment whose file
/// is missing is left out.
async fn write_zip(
    pipe: DuplexStream,
    markdown_name: &str,
    markdown: String,
    files: &[(String, String)],
    store: &dyn FileStore,
) -> io::Result<()> {
    let zip_error = io::Error::other;
    let mut zip = ZipFileWriter::with_tokio(pipe).force_no_zip64();
    let entry = ZipEntryBuilder::new(markdown_name.to_owned().into(), Compression::Stored);
    zip.write_entry_whole(entry, markdown.as_bytes())
        .await
        .map_err(zip_error)?;
    drop(markdown);
    for (id, name) in files {
        let mut file = match store.get(id).await {
            Ok(file) => file,
            Err(FileError::NotFound) => {
                warn!(id, "an exported attachment has a row and no file");
                continue;
            }
            Err(error) => return Err(io::Error::other(error)),
        };
        let path = format!("{ATTACHMENTS_DIR}{id}/{name}");
        let entry = ZipEntryBuilder::new(path.into(), Compression::Stored);
        let mut writer = zip.write_entry_stream(entry).await.map_err(zip_error)?;
        while let Some(chunk) = file.bytes.next().await {
            writer.write_all(&chunk?).await?;
        }
        writer.close().await.map_err(zip_error)?;
    }
    let mut pipe = zip.close().await.map_err(zip_error)?.into_inner();
    pipe.shutdown().await
}

/// An entry of an imported zip.
struct Entry {
    index: usize,
    size: u64,
}

/// An imported zip, read from its scratch file.
struct Archive {
    reader: ZipFileReader<Compat<BufReader<File>>>,
    /// Its files, by name.
    entries: HashMap<String, Entry>,
    /// The index of its one Markdown file.
    markdown: usize,
    /// The size of the zip, which bounds what its entries may write out together.
    length: u64,
}

fn unreadable_zip(error: ZipError) -> ApiError {
    match error {
        ZipError::CompressionNotSupported(_) => invalid("file", "compressed")
            .with_message("Only zips whose entries are stored, not compressed, can be imported"),
        _ => invalid("file", "invalid").with_message("The zip cannot be read"),
    }
}

impl Archive {
    async fn open(mut file: File) -> Result<Self, ApiError> {
        let length = check_directory(&mut file).await?;
        file.seek(SeekFrom::Start(0))
            .await
            .map_err(ApiError::internal)?;
        let reader = ZipFileReader::with_tokio(BufReader::new(file))
            .await
            .map_err(unreadable_zip)?;
        let mut entries = HashMap::new();
        let mut markdown = Vec::new();
        for (index, entry) in reader.file().entries().iter().enumerate() {
            let Ok(name) = entry.filename().as_str() else {
                continue;
            };
            if entry.dir().unwrap_or(true) {
                continue;
            }
            if entry.compression() != Compression::Stored {
                return Err(unreadable_zip(ZipError::CompressionNotSupported(0)));
            }
            if !name.contains('/') && name.to_lowercase().ends_with(".md") {
                markdown.push(index);
            }
            // Stored, so both sizes are the bytes the entry writes out.
            let size = entry.uncompressed_size();
            if entry.compressed_size() != size {
                return Err(unreadable_zip(ZipError::UnableToLocateEOCDR));
            }
            entries.insert(name.to_owned(), Entry { index, size });
        }
        let markdown = match markdown[..] {
            [] => return Err(invalid("file", "no_markdown")),
            [one] => one,
            _ => return Err(invalid("file", "several_markdown")),
        };
        Ok(Self {
            reader,
            entries,
            length,
            markdown,
        })
    }

    async fn markdown(&mut self) -> Result<Vec<u8>, ApiError> {
        let entry = self
            .reader
            .reader_without_entry(self.markdown)
            .await
            .map_err(unreadable_zip)?;
        let mut bytes = Vec::new();
        entry
            .compat()
            .take(MAX_MARKDOWN_BYTES + 1)
            .read_to_end(&mut bytes)
            .await
            .map_err(|_| invalid("file", "invalid"))?;
        if bytes.len() as u64 > MAX_MARKDOWN_BYTES {
            return Err(too_large());
        }
        Ok(bytes)
    }

    /// Stores entry `index` as file `id`.
    async fn put(
        &mut self,
        index: usize,
        id: &str,
        files: &dyn FileStore,
        max_bytes: u64,
    ) -> Result<Stored, FileError> {
        let entry = self
            .reader
            .reader_without_entry(index)
            .await
            .map_err(|error| FileError::Body(io::Error::other(error)))?;
        let stream = ReaderStream::new(entry.compat());
        files.put(id, Box::pin(stream), max_bytes).await
    }
}

/// Bounds what parsing the zip's directory reads, before it is parsed: few entries, a small
/// directory right before its end record, and no zip64 records, whose sizes would be elsewhere.
/// The end record is found as the parser finds it, the last one in the file.
async fn check_directory(file: &mut File) -> Result<u64, ApiError> {
    let unreadable = || invalid("file", "invalid").with_message("The zip cannot be read");
    let length = file
        .seek(SeekFrom::End(0))
        .await
        .map_err(ApiError::internal)?;
    let tail_length = length.min((EOCD_BYTES + 4 + usize::from(u16::MAX)) as u64);
    let start = length - tail_length;
    file.seek(SeekFrom::Start(start))
        .await
        .map_err(ApiError::internal)?;
    let mut tail = vec![0; tail_length as usize];
    file.read_exact(&mut tail)
        .await
        .map_err(ApiError::internal)?;
    let at = tail
        .windows(4)
        .rposition(|window| window == EOCD_SIGNATURE)
        .filter(|at| tail.len() - at >= EOCD_BYTES)
        .ok_or_else(unreadable)?;
    if at >= 20 && tail[at - 20..at - 16] == ZIP64_LOCATOR_SIGNATURE {
        return Err(unreadable());
    }
    let record = &tail[at..];
    let entries = u16::from_le_bytes([record[10], record[11]]);
    let size = u32::from_le_bytes([record[12], record[13], record[14], record[15]]);
    let offset = u32::from_le_bytes([record[16], record[17], record[18], record[19]]);
    if entries > MAX_ENTRIES {
        return Err(invalid("file", "too_many_entries"));
    }
    if size > MAX_DIRECTORY_BYTES || u64::from(offset) + u64::from(size) != start + at as u64 {
        return Err(unreadable());
    }
    Ok(length)
}

/// Reads at most `limit` bytes of `body`.
async fn read_limited(body: Body, limit: u64) -> Result<Vec<u8>, ApiError> {
    let mut stream = body.into_data_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk: Bytes = chunk.map_err(|_| cut_short())?;
        if (bytes.len() + chunk.len()) as u64 > limit {
            return Err(too_large());
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn cut_short() -> ApiError {
    ApiError::new(ErrorCode::InvalidRequest).with_message("The upload was cut short")
}

/// Streams `body` into a scratch file, at most `limit` bytes, and answers it rewound.
async fn receive(files: &dyn FileStore, body: Body, limit: u64) -> Result<File, ApiError> {
    let mut file = File::from_std(files.scratch().map_err(ApiError::internal)?);
    let mut stream = body.into_data_stream().map_err(io::Error::other);
    let mut written = 0u64;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| cut_short())?;
        written += chunk.len() as u64;
        if written > limit {
            return Err(too_large());
        }
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
    }
    file.flush().await.map_err(ApiError::internal)?;
    Ok(file)
}

fn is_zip(headers: &HeaderMap) -> bool {
    let kind = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase());
    matches!(
        kind.as_deref(),
        Some("application/zip" | "application/x-zip-compressed")
    )
}

/// The type of an imported attachment, from its extension: a zip keeps no types.
fn guess_type(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, ext)| ext.to_ascii_lowercase());
    match extension.as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        Some("svg") => "image/svg+xml",
        Some("pdf") => "application/pdf",
        Some("txt") => "text/plain",
        Some("md") => "text/markdown",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}

/// The title a file name gives: without its extension.
fn title_from(name: &str) -> String {
    let lower = name.to_lowercase();
    [".md", ".markdown", ".zip"]
        .iter()
        .find(|extension| lower.ends_with(*extension))
        .map_or(name, |extension| &name[..name.len() - extension.len()])
        .to_owned()
}

/// What an import has read, before anything is stored.
struct Parsed {
    title: String,
    document: Document,
    /// The size of the Markdown, about what the page's room takes in.
    bytes: usize,
    /// Entries to store as attachments, by name.
    attachments: Vec<(String, usize)>,
}

async fn import<
    S: DocStore + PageStore + SpaceStore + AttachmentStore,
    A: Send + Sync + 'static,
>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(space): PathParam,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<PageBody>), ApiError> {
    let query = query.as_deref();
    let name = param(query, "name").and_then(decoded).unwrap_or_default();
    // An id needs no decoding.
    let parent_id = param(query, "parent_id").map(str::to_owned);
    let user = current.user.id;
    let found = state.store.space(&space, &user).await?;
    if found.ok_or_else(not_found)?.role < SpaceRole::Editor {
        return Err(ApiError::new(ErrorCode::Forbidden));
    }

    let limits = state.attachments;
    let mut archive = None;
    let markdown = if is_zip(&headers) {
        let file = receive(&*state.files, body, limits.max_import_bytes).await?;
        let mut opened = Archive::open(file).await?;
        let markdown = opened.markdown().await?;
        archive = Some(opened);
        markdown
    } else {
        read_limited(body, MAX_MARKDOWN_BYTES.min(limits.max_import_bytes)).await?
    };
    let parsed = parse(&state, &name, markdown, archive.as_ref())?;
    if let Some(archive) = &archive {
        let mut total = 0u64;
        for (entry, _) in &parsed.attachments {
            let size = archive.entries[entry].size;
            if size > limits.max_upload_bytes {
                return Err(ApiError::new(ErrorCode::TooLarge)
                    .with_message("An attachment is over the upload limit"));
            }
            total = total.saturating_add(size);
        }
        // Entries may share their bytes, which would write out more than was received.
        if total > archive.length {
            return Err(unreadable_zip(ZipError::UnableToLocateEOCDR));
        }
    }

    let mut page = pages::add(&state, space, parent_id, Placement::Last, user.clone()).await?;
    // Run to its end even if the client leaves, so a failure still moves the page to the trash.
    let task = state.clone();
    let id = page.id.clone();
    let filled = tokio::spawn(async move {
        let filled = fill(&task, &id, &user, &parsed, archive).await;
        if filled.is_err()
            && let Err(trashed) = pages::move_to_trash(&task, &id, &user).await
        {
            warn!(?trashed, page = id, "cannot trash a failed import");
        }
        filled.map(|()| parsed.title)
    });
    page.title = filled.await.map_err(ApiError::internal)??;
    Ok((StatusCode::CREATED, Json(page.into())))
}

fn parse<S, A>(
    state: &AppState<S, A>,
    name: &str,
    markdown: Vec<u8>,
    archive: Option<&Archive>,
) -> Result<Parsed, ApiError> {
    let text = String::from_utf8(markdown).map_err(|_| invalid("file", "not_utf8"))?;
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let absolute = Absolute {
        origin: state.http.public_url.origin(),
    };
    let mut document = match archive {
        Some(archive) => {
            let links = Unbundle {
                absolute,
                entries: &archive.entries,
            };
            from_markdown_with(text, &links)
        }
        None => from_markdown_with(text, &absolute),
    };
    let title = take_title(&mut document)
        .map(|title| title.trim().to_owned())
        .filter(|title| !title.is_empty())
        .unwrap_or_else(|| title_from(name.trim()));
    let title: String = title.trim().chars().take(MAX_TITLE_CHARS).collect();
    let attachments = match archive {
        Some(archive) => document
            .attachments()
            .into_iter()
            .filter_map(|key| {
                let index = archive.entries.get(&key)?.index;
                Some((key, index))
            })
            .collect(),
        None => Vec::new(),
    };
    Ok(Parsed {
        title,
        document,
        bytes: text.len(),
        attachments,
    })
}

/// Stores the zip's attachments for `page`, then writes the content through its room.
async fn fill<S: DocStore + PageStore + AttachmentStore, A>(
    state: &AppState<S, A>,
    page: &str,
    user: &str,
    parsed: &Parsed,
    archive: Option<Archive>,
) -> Result<(), ApiError> {
    let mut ids = HashMap::new();
    if let Some(mut archive) = archive {
        let limits = state.attachments;
        for (entry, index) in &parsed.attachments {
            let id = Uuid::now_v7().to_string();
            // At most what the directory declared, which `import` checked against the zip's size.
            let declared = archive.entries[entry].size;
            let stored = archive
                .put(*index, &id, &*state.files, declared)
                .await
                .map_err(file_error)?;
            let last = entry.rsplit('/').next().unwrap_or_default();
            let file_name: String = last
                .trim()
                .chars()
                .map(|c| if c.is_control() { '_' } else { c })
                .take(MAX_NAME_CHARS)
                .collect();
            let file_name = if file_name.is_empty() {
                "file".to_owned()
            } else {
                file_name
            };
            let new = NewAttachment {
                content_type: guess_type(&file_name).to_owned(),
                id,
                page_id: page.to_owned(),
                file_name,
                size: stored.size as i64,
                sha256: stored.sha256,
                actor: user.to_owned(),
                now: now_ms(),
            };
            let quota = limits.workspace_quota_bytes.map(|quota| quota as i64);
            if let Err(error) = state.store.create_attachment(&new, quota).await {
                if let Err(error) = state.files.delete(&new.id).await {
                    warn!(%error, id = new.id, "cannot remove an unrecorded import");
                }
                return Err(error.into());
            }
            ids.insert(entry.clone(), new.id);
        }
    }
    let mut document = parsed.document.clone();
    document.map_attachments(&mut |key| ids.get(key).cloned());
    // Checked again once the room is open, as a load can take long.
    state.registry.open(page).await?;
    state.store.page_to_edit(page, user).await?;
    state
        .registry
        .rewrite(page, user, parsed.bytes, |txn| {
            replace_page(txn, &parsed.title, &document)
        })
        .await?;
    Ok(())
}
