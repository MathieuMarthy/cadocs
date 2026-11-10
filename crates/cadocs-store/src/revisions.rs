//! Revisions of a page (docs/DATA_MODEL.md#documents). The store keeps what it is given: reading,
//! comparing and compressing a document is the revision job's.

use std::future::Future;
use std::time::Duration;

use crate::StoreError;

/// The kind of the job that takes a revision of one page, with `{"page": "<id>"}` as its payload
/// and `revision:<id>` as its key. A flush queues it `pause` after itself, and each later flush
/// pushes it back, but never past `max` after the first one.
pub const CREATE_REVISION: &str = "create_revision";
/// The recurring job that thins revisions older than the retention to one per day.
pub const THIN_REVISIONS: &str = "thin_revisions";

/// When the job of a flush runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RevisionTiming {
    /// A pause in editing this long takes a revision.
    pub pause: Duration,
    /// Editing without such a pause takes one this often.
    pub max: Duration,
}

impl Default for RevisionTiming {
    fn default() -> Self {
        Self {
            pause: Duration::from_secs(120),
            max: Duration::from_secs(600),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionAuthor {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevisionSummary {
    /// 1 for the first revision of the page, then one more each time.
    pub number: i64,
    pub title: String,
    /// Who wrote since the revision before, those whose account still exists.
    pub authors: Vec<RevisionAuthor>,
    pub created_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Revision {
    pub summary: RevisionSummary,
    /// The zstd-compressed Yjs state.
    pub state: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewRevision<'a> {
    pub id: &'a str,
    pub page_id: &'a str,
    pub title: &'a str,
    /// The zstd-compressed Yjs state.
    pub state: &'a [u8],
    /// When the document was read: authors added later stay for the next revision.
    pub read_at: i64,
}

pub trait RevisionStore: Send + Sync + 'static {
    /// The page's most recent revision, `None` if it has none. No role is checked: only the
    /// revision job and a restore, once checked, call this and `add_revision`.
    fn latest_revision(
        &self,
        page_id: &str,
    ) -> impl Future<Output = Result<Option<Revision>, StoreError>> + Send;

    /// Stores the revision after the latest one, with the authors since then, and answers its
    /// number. `None` when the page is gone, and nothing is stored then.
    fn add_revision(
        &self,
        revision: &NewRevision<'_>,
    ) -> impl Future<Output = Result<Option<i64>, StoreError>> + Send;

    /// At most `limit` revisions of the page, the most recent first, those numbered below
    /// `before` if given. Refused as for reading the page.
    fn revisions(
        &self,
        page_id: &str,
        actor: &str,
        before: Option<i64>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<RevisionSummary>, StoreError>> + Send;

    /// One revision, for a reader of the page. `RevisionNotFound` if the page has no such number.
    fn revision(
        &self,
        page_id: &str,
        number: i64,
        actor: &str,
    ) -> impl Future<Output = Result<Revision, StoreError>> + Send;

    /// The same, for a user who may restore it: `NotAllowed` for a reader who may not.
    fn revision_to_restore(
        &self,
        page_id: &str,
        number: i64,
        actor: &str,
    ) -> impl Future<Output = Result<Revision, StoreError>> + Send;

    /// Keeps the last revision of each day among those created before `before`, and the most
    /// recent revision of each page. Answers how many went.
    fn thin_revisions(&self, before: i64) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// The titles of the live pages among `ids` that the user can read, for a renderer's links.
    fn readable_titles(
        &self,
        actor: &str,
        ids: &[String],
    ) -> impl Future<Output = Result<Vec<(String, String)>, StoreError>> + Send;
}
