//! Store traits and their SQLite implementation (ADR 0002).

mod accounts;
mod pages;
mod spaces;
mod sqlite;
mod workspaces;

use std::future::Future;
use std::time::{SystemTime, UNIX_EPOCH};

pub use accounts::{
    AccountStore, ActiveSession, AdminChanges, Credentials, FirstUser, Membership,
    NewPasswordReset, NewSession, UpdateBasis, User, UserChanges, UserRecord,
};
pub use pages::{
    DateCursor, MAX_PAGE_DEPTH, MAX_TITLE_CHARS, NewPage, PageMove, PageRecord, PageStore,
    Placement,
};
pub use spaces::{Grant, GrantTarget, NewGrant, NewSpace, Space, SpaceChanges, SpaceStore};
pub use sqlite::SqliteStore;
pub use workspaces::{
    Invitation, InvitationPreview, InvitedUser, MAX_SLUG_CHARS, Member, NewInvitation,
    NewWorkspace, Workspace, WorkspaceChanges, WorkspaceStore,
};

/// The current time as stored: milliseconds since the Unix epoch.
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("database error: {0}")]
    Database(#[from] sqlx::Error),
    #[error("migration error: {0}")]
    Migration(#[from] sqlx::migrate::MigrateError),
    #[error("seq {seq} of {doc_id} is covered by a snapshot, or holds other data")]
    SeqConflict { doc_id: String, seq: i64 },
    #[error("snapshot of {doc_id} covers seq {stored}, refusing an older one at {offered}")]
    StaleSnapshot {
        doc_id: String,
        stored: i64,
        offered: i64,
    },
    #[error("the instance is already set up")]
    AlreadySetUp,
    #[error("another account uses this email")]
    EmailTaken,
    #[error("the session or password behind this change is gone")]
    StaleCredentials,
    #[error("this user is the last owner of a workspace")]
    LastOwner,
    #[error("this user is the last enabled instance admin")]
    LastInstanceAdmin,
    #[error("this database belongs to another version of cadocs: start the matching server first")]
    OtherVersion,
    #[error("no such user")]
    UserNotFound,
    #[error("this account is disabled")]
    UserDisabled,
    #[error("no such workspace")]
    WorkspaceNotFound,
    #[error("no such member")]
    MemberNotFound,
    #[error("no such invitation")]
    InvitationNotFound,
    #[error("no such space")]
    SpaceNotFound,
    #[error("a grant names a user or group outside the workspace")]
    UnknownGrantTarget,
    #[error("two grants name the same user or group")]
    DuplicateGrant,
    #[error("no such page")]
    PageNotFound,
    #[error("the parent is not a live page of the space")]
    UnknownParent,
    #[error("the sibling is not a live page beside this one")]
    UnknownSibling,
    #[error("a page cannot go under itself")]
    PageCycle,
    #[error("the tree would have too many levels")]
    TooDeep,
    #[error("the space belongs to another workspace")]
    OtherWorkspace,
    #[error("this user may not do that")]
    NotAllowed,
    #[error("this is not the name of the workspace")]
    NameMismatch,
    #[error("this email belongs to a member")]
    AlreadyMember,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    /// Yjs update v1 holding the full document.
    pub state: Vec<u8>,
    /// Last `seq` this state includes.
    pub upto_seq: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StoredUpdate {
    pub seq: i64,
    /// Yjs update v1.
    pub data: Vec<u8>,
}

/// What a flush copies to the row of the page, besides the time. `None` keeps what the row holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DocEdit {
    pub title: Option<String>,
    /// The last user who wrote. An account deleted since writes as nobody.
    pub author: Option<String>,
}

impl DocEdit {
    /// A client can put any length in the Yjs title, and this copy is read with every page.
    pub fn clip_title(mut title: String) -> String {
        if let Some((end, _)) = title.char_indices().nth(MAX_TITLE_CHARS) {
            title.truncate(end);
        }
        title
    }
}

/// Everything needed to rebuild a document: apply `snapshot`, then `updates` in order.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StoredDoc {
    pub snapshot: Option<Snapshot>,
    /// Only the updates the snapshot does not cover, by ascending `seq`.
    pub updates: Vec<StoredUpdate>,
}

impl StoredDoc {
    /// The highest `seq` in use, 0 for an empty document. It keeps growing after compaction.
    pub fn last_seq(&self) -> i64 {
        let compacted = self.snapshot.as_ref().map_or(0, |s| s.upto_seq);
        self.updates.last().map_or(compacted, |u| u.seq)
    }
}

/// Persistence of Yjs documents. `cadocs-collab` only knows this trait.
pub trait DocStore: Send + Sync + 'static {
    /// Checks that the database answers, for `/health`.
    fn ping(&self) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Reads a document as of one point in time, so a concurrent compaction cannot hide updates.
    fn load(&self, doc_id: &str) -> impl Future<Output = Result<StoredDoc, StoreError>> + Send;

    /// Appends a batch in one transaction: either every update is stored, or none is. Replaying
    /// a batch that was already stored succeeds, so a flush can retry after a lost commit. A `seq`
    /// below 1, covered by the snapshot, or stored with other data is a `SeqConflict`: `load`
    /// would never return that update, so it must not pass for durable. The same transaction
    /// applies `edit` to the page. A document whose page is gone is a `PageNotFound`.
    fn append_updates(
        &self,
        doc_id: &str,
        updates: &[StoredUpdate],
        edit: &DocEdit,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Writes the snapshot and deletes the updates it covers, in one transaction. Updates after
    /// `snapshot.upto_seq` stay. A snapshot older than the stored one is refused, because the
    /// updates it would need are already gone, and so is a document whose page is gone, with
    /// `PageNotFound`.
    fn compact(
        &self,
        doc_id: &str,
        snapshot: &Snapshot,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}
