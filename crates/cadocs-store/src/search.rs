//! Full-text search (ADR 0007). The store keeps what it is given: stemming is `cadocs-search`'s,
//! and reading a page's content is the indexing job's.

use std::future::Future;

use crate::{PageRecord, StoreError};

/// The kind of the job that indexes one page, with `{"page": "<id>"}` as its payload and
/// `reindex:<id>` as its key. A flush queues it, after a delay, so an editing session costs one
/// run, and so does a change of the workspace's language, for each of its pages.
pub const REINDEX_PAGE: &str = "reindex_page";

/// What indexing a page needs to know first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexTarget {
    pub workspace_id: String,
    /// The stemming language of the workspace.
    pub language: String,
}

/// A page's index. `title` and `body` are stemmed, `plain_text` holds the real words.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageIndex<'a> {
    pub page_id: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub plain_text: &'a str,
    /// The attachments the content shows. Each one of the page's workspace gets a reference
    /// from the page, so its readers can download it.
    pub attachments: &'a [String],
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub page: PageRecord,
    /// The indexed text, for the excerpt.
    pub plain_text: String,
}

pub trait SearchStore: Send + Sync + 'static {
    /// `None` for a page that is gone. No role is checked: only the indexing job calls this and
    /// `index_page`.
    fn index_target(
        &self,
        page_id: &str,
    ) -> impl Future<Output = Result<Option<IndexTarget>, StoreError>> + Send;

    /// Replaces the page's index and adds its references, in one transaction. Answers `false`
    /// when the page is gone, and stores nothing then.
    fn index_page(
        &self,
        index: &PageIndex<'_>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// At most `limit` live pages matching `fts`, an FTS5 expression, best first, among those the
    /// user can read, in `space` only if given. Refused with `WorkspaceNotFound` for a stranger.
    fn search(
        &self,
        workspace_id: &str,
        actor: &str,
        fts: &str,
        space: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<SearchHit>, StoreError>> + Send;
}
