//! Attachments and the pages that reference them ([`DATA_MODEL.md`]). The bytes live in a file
//! store, under the attachment's id.
//!
//! [`DATA_MODEL.md`]: ../../../docs/DATA_MODEL.md#attachments

use std::future::Future;

use crate::StoreError;

/// What an upload records once its file is in place.
#[derive(Debug, Clone)]
pub struct NewAttachment {
    pub id: String,
    /// The first page that references it.
    pub page_id: String,
    pub file_name: String,
    pub content_type: String,
    pub size: i64,
    pub sha256: String,
    pub actor: String,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    pub id: String,
    pub workspace_id: String,
    pub file_name: String,
    pub content_type: String,
    pub size: i64,
    pub sha256: String,
    pub uploaded_by: Option<String>,
    pub created_at: i64,
}

/// Where an upload would go, checked before its body is read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadTarget {
    pub workspace_id: String,
    /// What the workspace's attachments take now, in bytes.
    pub used_bytes: i64,
}

pub trait AttachmentStore: Send + Sync + 'static {
    /// Refused with `PageNotFound`, or `NotAllowed` for a reader who cannot upload.
    fn upload_target(
        &self,
        page_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<UploadTarget, StoreError>> + Send;

    /// Records the attachment and its first reference, checking the role again and the quota
    /// of the workspace, in bytes, with the file counted. Refused with `PageNotFound`,
    /// `NotAllowed` or `QuotaExceeded`.
    fn create_attachment(
        &self,
        new: &NewAttachment,
        quota_bytes: Option<i64>,
    ) -> impl Future<Output = Result<Attachment, StoreError>> + Send;

    /// `None` unless the user can read a page that references it.
    fn attachment(
        &self,
        id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<Option<Attachment>, StoreError>> + Send;

    /// Deletes the attachment and every reference to it, for an editor of a page that references
    /// it. The caller deletes the file. Refused with `AttachmentNotFound`, or `NotAllowed` for a
    /// reader who is not an editor.
    fn delete_attachment(
        &self,
        id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// At most `limit` attachments that no page references any more. No role is checked: only
    /// the cleanup job calls this and the two below.
    fn orphan_attachments(
        &self,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send;

    /// Deletes the attachment if it is still an orphan, and answers whether it did.
    fn forget_orphan(&self, id: &str) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Those of `ids` that have a row.
    fn known_attachments(
        &self,
        ids: &[String],
    ) -> impl Future<Output = Result<Vec<String>, StoreError>> + Send;
}
