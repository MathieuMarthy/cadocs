//! Public shares of a page ([`DATA_MODEL.md`]): a token anyone can open, for the page and, if the
//! share says so, its live descendants (ADR 0009).
//!
//! Editors manage shares, checked like any page action. The public lookups take a token instead
//! of a user and check no role: the token is the capability. They answer `None` alike for an
//! unknown token, an expired share, a page or space in the trash, and a page the share does not
//! cover, so a visitor learns nothing from the difference.
//!
//! [`DATA_MODEL.md`]: ../../../docs/DATA_MODEL.md#pages

use std::future::Future;

use crate::{Attachment, StoreError};

#[derive(Debug, Clone)]
pub struct NewShare {
    pub id: String,
    pub page_id: String,
    pub token: String,
    pub include_children: bool,
    pub indexable: bool,
    pub expires_at: Option<i64>,
    pub actor: String,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Share {
    pub id: String,
    pub page_id: String,
    pub token: String,
    pub include_children: bool,
    pub indexable: bool,
    pub created_by: Option<String>,
    pub expires_at: Option<i64>,
    pub created_at: i64,
}

/// A page a share covers, as a public page names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedPage {
    pub id: String,
    pub slug: String,
    pub title: String,
    pub updated_at: i64,
}

/// What a public page shows around its content.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SharedView {
    pub share: Share,
    /// The page asked for.
    pub page: SharedPage,
    /// The pages from the shared one down to the parent of `page`, empty for the shared page.
    pub trail: Vec<SharedPage>,
    /// Live children of `page` in tree order, at most the limit asked for, and none for a share
    /// without its children.
    pub children: Vec<SharedPage>,
}

pub trait ShareStore: Send + Sync + 'static {
    /// Refused with `PageNotFound`, or `NotAllowed` for a reader who is not an editor.
    fn create_share(
        &self,
        new: &NewShare,
    ) -> impl Future<Output = Result<Share, StoreError>> + Send;

    /// At most `limit` shares of the page, oldest first, after the share `after`. Refused as
    /// `create_share` is.
    fn list_shares(
        &self,
        page_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Share>, StoreError>> + Send;

    /// Refused with `ShareNotFound` for whoever cannot read its page, or `NotAllowed` for a
    /// reader who is not an editor.
    fn delete_share(
        &self,
        share_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The shared page, or with `slug` the covered page that has it. `None` when the share is
    /// closed or does not cover it.
    fn shared_view(
        &self,
        token: &str,
        slug: Option<&str>,
        now: i64,
        children_limit: u32,
    ) -> impl Future<Output = Result<Option<SharedView>, StoreError>> + Send;

    /// Those of `ids` the share covers.
    fn shared_pages(
        &self,
        token: &str,
        ids: &[String],
        now: i64,
    ) -> impl Future<Output = Result<Vec<SharedPage>, StoreError>> + Send;

    /// The attachment, if the content of a page the share covers shows it, as the last index of
    /// that page found it.
    fn shared_attachment(
        &self,
        token: &str,
        attachment_id: &str,
        now: i64,
    ) -> impl Future<Output = Result<Option<Attachment>, StoreError>> + Send;
}
