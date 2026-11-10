//! Attachments (ADR 0005). An upload is the raw file as the body, its name in the query and its
//! type in `Content-Type`, so it streams to disk with no parser in between. A download only
//! shows in the page for a few image types: anything else is sent as a file to save, and never
//! runs in the application's origin.

use std::io;

use axum::body::Body;
use axum::extract::{RawQuery, State};
use axum::http::header::{
    CACHE_CONTROL, CONTENT_DISPOSITION, CONTENT_LENGTH, CONTENT_SECURITY_POLICY, CONTENT_TYPE,
    ETAG, IF_NONE_MATCH, LOCATION,
};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use cadocs_files::{Download, FileError};
use cadocs_store::{AccountStore, Attachment, AttachmentStore, NewAttachment, StoreError, now_ms};
use futures_util::TryStreamExt;
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, percent_decode_str, utf8_percent_encode};
use serde::Serialize;
use uuid::Uuid;

use super::{ApiError, ErrorCode, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// The longest file name kept, in characters.
pub const MAX_NAME_CHARS: usize = 255;
const MAX_TYPE_BYTES: usize = 127;
const UNKNOWN_TYPE: &str = "application/octet-stream";
/// Types a browser shows as an image and never runs. SVG holds scripts, so it is not one.
const INLINE_TYPES: [&str; 5] = [
    "image/avif",
    "image/gif",
    "image/jpeg",
    "image/png",
    "image/webp",
];
/// For a download opened by itself: nothing in it may run or load.
const DOWNLOAD_POLICY: &str = "default-src 'none'; frame-ancestors 'none'; sandbox";
/// RFC 8187: what `filename*` may hold unescaped.
const ATTR_CHARS: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'!')
    .remove(b'#')
    .remove(b'$')
    .remove(b'&')
    .remove(b'+')
    .remove(b'-')
    .remove(b'.')
    .remove(b'^')
    .remove(b'_')
    .remove(b'`')
    .remove(b'|')
    .remove(b'~');

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: AccountStore + AttachmentStore,
    A: Send + Sync + 'static,
{
    // No `DefaultBodyLimit`: the body is never read whole, and the upload limit applies while it
    // streams.
    Router::new()
        .route("/pages/{page}/attachments", post(upload))
        .route("/attachments/{id}", get(download).delete(remove))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

#[derive(Serialize)]
struct AttachmentBody {
    id: String,
    workspace_id: String,
    file_name: String,
    content_type: String,
    size: i64,
    uploaded_by: Option<String>,
    created_at: i64,
}

impl From<Attachment> for AttachmentBody {
    fn from(attachment: Attachment) -> Self {
        Self {
            id: attachment.id,
            workspace_id: attachment.workspace_id,
            file_name: attachment.file_name,
            content_type: attachment.content_type,
            size: attachment.size,
            uploaded_by: attachment.uploaded_by,
            created_at: attachment.created_at,
        }
    }
}

async fn upload<S: AttachmentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
    RawQuery(query): RawQuery,
    headers: HeaderMap,
    body: Body,
) -> Result<(StatusCode, Json<AttachmentBody>), ApiError> {
    let file_name = file_name(query.as_deref())?;
    let content_type = content_type(&headers);
    let target = state.store.upload_target(&page, &current.user.id).await?;

    let limits = state.attachments;
    let left = limits
        .workspace_quota_bytes
        .map(|quota| quota.saturating_sub(target.used_bytes.max(0) as u64));
    if left == Some(0) {
        return Err(quota_reached());
    }
    let max_bytes = left.map_or(limits.max_upload_bytes, |left| {
        left.min(limits.max_upload_bytes)
    });
    let over = || {
        if max_bytes < limits.max_upload_bytes {
            quota_reached()
        } else {
            ApiError::new(ErrorCode::TooLarge).with_message("The file is over the upload limit")
        }
    };
    let announced = headers
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok());
    if announced.and_then(|value| value.parse::<u64>().ok()) > Some(max_bytes) {
        return Err(over());
    }

    let id = Uuid::now_v7().to_string();
    let stream = body.into_data_stream().map_err(io::Error::other);
    let stored = match state.files.put(&id, Box::pin(stream), max_bytes).await {
        Ok(stored) => stored,
        Err(FileError::TooLarge(_)) => return Err(over()),
        Err(error) => return Err(file_error(error)),
    };
    let new = NewAttachment {
        id,
        page_id: page,
        file_name,
        content_type,
        size: stored.size as i64,
        sha256: stored.sha256,
        actor: current.user.id.clone(),
        now: now_ms(),
    };
    let quota = limits.workspace_quota_bytes.map(|quota| quota as i64);
    match state.store.create_attachment(&new, quota).await {
        Ok(attachment) => Ok((StatusCode::CREATED, Json(attachment.into()))),
        Err(error) => {
            // Left behind, it would be an orphan with no row, which the cleanup job also
            // removes, an hour later.
            if let Err(error) = state.files.delete(&new.id).await {
                tracing::warn!(%error, id = new.id, "cannot remove an unrecorded upload");
            }
            Err(error.into())
        }
    }
}

async fn download<S: AttachmentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let found = state.store.attachment(&id, &current.user.id).await?;
    let found = found.ok_or(ApiError::new(ErrorCode::NotFound))?;
    let etag = format!("\"{}\"", found.sha256);
    let cached = [
        (ETAG, header(&etag)),
        (CACHE_CONTROL, HeaderValue::from_static("private, no-cache")),
    ];
    if matches_etag(&headers, &etag) {
        return Ok((StatusCode::NOT_MODIFIED, cached).into_response());
    }

    let inline = INLINE_TYPES.contains(&found.content_type.as_str());
    let disposition = disposition(inline, &found.file_name);
    let download = Download {
        content_type: &found.content_type,
        disposition: &disposition,
    };
    // The access check above runs on each view: the redirect is never cached, while the link it
    // gives stays the same for a while, so the browser keeps the file (ADR 0020). Only safe
    // images go there: the storage sends no sandbox or `nosniff`, and its origin may be shared.
    let link = inline
        .then(|| state.files.download_url(&id, &download))
        .flatten();
    if let Some(url) = link {
        let location = HeaderValue::try_from(url).map_err(ApiError::internal)?;
        let headers = [
            (LOCATION, location),
            (CACHE_CONTROL, HeaderValue::from_static("private, no-store")),
        ];
        return Ok((StatusCode::FOUND, headers).into_response());
    }

    let file = match state.files.get(&id).await {
        Ok(file) => file,
        Err(FileError::NotFound) => {
            tracing::warn!(id, "an attachment has a row and no file");
            return Err(ApiError::new(ErrorCode::NotFound));
        }
        Err(error) => return Err(file_error(error)),
    };
    let headers = [
        (CONTENT_TYPE, header(&found.content_type)),
        (CONTENT_LENGTH, HeaderValue::from(file.size)),
        (CONTENT_DISPOSITION, header(&disposition)),
        (
            CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(DOWNLOAD_POLICY),
        ),
    ];
    Ok((headers, cached, Body::from_stream(file.bytes)).into_response())
}

async fn remove<S: AttachmentStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(id): PathParam,
) -> Result<StatusCode, ApiError> {
    state.store.delete_attachment(&id, &current.user.id).await?;
    // The row is gone, so a failure here leaves a file with no row: the cleanup job removes it.
    if let Err(error) = state.files.delete(&id).await {
        tracing::warn!(%error, id, "cannot remove a deleted attachment");
    }
    Ok(StatusCode::NO_CONTENT)
}

fn quota_reached() -> ApiError {
    StoreError::QuotaExceeded.into()
}

fn file_error(error: FileError) -> ApiError {
    match error {
        FileError::TooLarge(_) => ApiError::new(ErrorCode::TooLarge),
        FileError::Body(_) => {
            ApiError::new(ErrorCode::InvalidRequest).with_message("The upload was cut short")
        }
        FileError::NotFound | FileError::InvalidId => ApiError::new(ErrorCode::NotFound),
        // A full disk among them: `docs/OPERATIONS.md` answers it with a `500`.
        FileError::Io(error) => ApiError::internal(error),
    }
}

/// Values checked when they were stored, so this never falls back in practice.
fn header(value: &str) -> HeaderValue {
    HeaderValue::from_str(value).unwrap_or(HeaderValue::from_static(UNKNOWN_TYPE))
}

fn matches_etag(headers: &HeaderMap, etag: &str) -> bool {
    let Some(value) = headers.get(IF_NONE_MATCH).and_then(|v| v.to_str().ok()) else {
        return false;
    };
    value.split(',').map(str::trim).any(|candidate| {
        candidate == "*" || candidate.strip_prefix("W/").unwrap_or(candidate) == etag
    })
}

/// `name` from the query: `+` is a space, as `URLSearchParams` writes it.
fn file_name(query: Option<&str>) -> Result<String, ApiError> {
    let invalid = |problem| {
        ApiError::new(ErrorCode::ValidationFailed)
            .with_message("The file needs a name of 1 to 255 characters")
            .with_field("name", problem)
    };
    let raw = query
        .unwrap_or_default()
        .split('&')
        .find_map(|pair| pair.strip_prefix("name="))
        .ok_or_else(|| invalid("required"))?
        .replace('+', " ");
    let decoded = percent_decode_str(&raw)
        .decode_utf8()
        .map_err(|_| invalid("invalid"))?;
    let name = decoded.trim();
    if name.is_empty() {
        return Err(invalid("required"));
    }
    if name.chars().any(char::is_control) {
        return Err(invalid("invalid"));
    }
    if name.chars().count() > MAX_NAME_CHARS {
        return Err(invalid("too_long"));
    }
    Ok(name.to_owned())
}

/// The type the client gave, without its parameters, or `application/octet-stream`.
fn content_type(headers: &HeaderMap) -> String {
    let given = headers
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase())
        .unwrap_or_default();
    let token = |part: &str| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&byte))
    };
    match given.split_once('/') {
        Some((kind, subtype)) if given.len() <= MAX_TYPE_BYTES && token(kind) && token(subtype) => {
            given
        }
        _ => UNKNOWN_TYPE.to_owned(),
    }
}

/// RFC 6266: an ASCII fallback for old clients, and the real name in `filename*`.
fn disposition(inline: bool, name: &str) -> String {
    let kind = if inline { "inline" } else { "attachment" };
    let fallback: String = name
        .chars()
        .map(|c| match c {
            ' ' => ' ',
            '"' | '\\' => '_',
            c if c.is_ascii_graphic() => c,
            _ => '_',
        })
        .collect();
    let encoded = utf8_percent_encode(name, ATTR_CHARS);
    format!("{kind}; filename=\"{fallback}\"; filename*=UTF-8''{encoded}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_read_from_the_query() {
        assert_eq!(file_name(Some("x=1&name=a+b%2Fc")).unwrap(), "a b/c");
        assert!(file_name(Some("filename=a")).is_err());
    }

    #[test]
    fn an_etag_matches_in_a_list_or_weak() {
        let mut headers = HeaderMap::new();
        headers.insert(IF_NONE_MATCH, HeaderValue::from_static("\"a\", W/\"b\""));
        assert!(matches_etag(&headers, "\"b\""));
        assert!(!matches_etag(&headers, "\"c\""));
    }
}
