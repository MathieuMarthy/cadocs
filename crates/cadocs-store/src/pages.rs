//! Pages and their tree.
//!
//! A method takes the user who asks and resolves their role itself, inside the transaction for a
//! write. A page the user cannot read is a `PageNotFound`, so nothing tells them it exists.

use std::future::Future;

use cadocs_core::permissions::SpaceRole;

use crate::StoreError;

/// How many levels a tree may have. Deleting a page runs nested triggers down its subtree, and
/// SQLite refuses more than 1000: a deeper tree could never be purged, nor its workspace deleted.
pub const MAX_PAGE_DEPTH: usize = 100;

/// Where a page goes among its siblings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Placement {
    First,
    Last,
    /// After this live sibling.
    After(String),
}

#[derive(Debug, Clone)]
pub struct NewPage {
    pub id: String,
    pub space_id: String,
    pub parent_id: Option<String>,
    pub placement: Placement,
    pub slug: String,
    pub actor: String,
    pub now: i64,
}

/// `space_id` is `None` to stay in the space of the page.
#[derive(Debug, Clone)]
pub struct PageMove {
    pub space_id: Option<String>,
    pub parent_id: Option<String>,
    pub placement: Placement,
}

/// A page as one user sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageRecord {
    pub id: String,
    pub workspace_id: String,
    pub space_id: String,
    pub parent_id: Option<String>,
    pub position: String,
    pub slug: String,
    pub title: String,
    /// What resolution gives this user on the space.
    pub role: SpaceRole,
    /// Whether a live page sits under this one.
    pub has_children: bool,
    pub created_by: Option<String>,
    pub updated_by: Option<String>,
    pub deleted_at: Option<i64>,
    pub deleted_by: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Where a list ordered by date starts again: after this row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DateCursor {
    pub date: i64,
    pub id: String,
}

pub trait PageStore: Send + Sync + 'static {
    /// Refused with `SpaceNotFound`, `NotAllowed`, `UnknownParent`, `UnknownSibling` or `TooDeep`.
    fn create_page(
        &self,
        page: &NewPage,
    ) -> impl Future<Output = Result<PageRecord, StoreError>> + Send;

    /// `None` unless the user can read the page.
    fn page(
        &self,
        page_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<Option<PageRecord>, StoreError>> + Send;

    /// `None` unless the user can read the page.
    fn page_by_slug(
        &self,
        workspace_id: &str,
        slug: &str,
        actor: &str,
    ) -> impl Future<Output = Result<Option<PageRecord>, StoreError>> + Send;

    /// At most `limit` live children of `parent_id`, or of the root of the space, in tree order,
    /// after the position `after`. Refused with `SpaceNotFound`, or `PageNotFound` for a parent
    /// that is not a live page of the space.
    fn tree(
        &self,
        space_id: &str,
        parent_id: Option<&str>,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<PageRecord>, StoreError>> + Send;

    /// A move to another space takes all descendants, the trashed ones too. Refused with
    /// `PageNotFound`, `SpaceNotFound`, `NotAllowed`, `OtherWorkspace`, `UnknownParent`,
    /// `PageCycle`, `UnknownSibling` or `TooDeep`.
    fn move_page(
        &self,
        page_id: &str,
        actor: &str,
        destination: &PageMove,
        now: i64,
    ) -> impl Future<Output = Result<PageRecord, StoreError>> + Send;

    /// Takes the live descendants, with one date. Refused with `PageNotFound` or `NotAllowed`.
    fn trash_page(
        &self,
        page_id: &str,
        actor: &str,
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Brings back what was trashed with the page, at the root of the space if its parent stays
    /// in the trash. Refused with `PageNotFound`, or `NotAllowed` for a page that is not in the
    /// trash.
    fn restore_page(
        &self,
        page_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<PageRecord, StoreError>> + Send;

    /// Deletes the page and everything under it. Refused with `PageNotFound`, or `NotAllowed` for
    /// a page that is not in the trash.
    fn purge_page(
        &self,
        page_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// At most `limit` trashed pages, the last trashed first. Refused with `SpaceNotFound` or
    /// `NotAllowed`.
    fn list_trashed_pages(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&DateCursor>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<PageRecord>, StoreError>> + Send;

    /// At most `limit` pages the user can read, the last changed first. Refused with
    /// `WorkspaceNotFound` for a stranger.
    fn recent_pages(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&DateCursor>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<PageRecord>, StoreError>> + Send;
}
