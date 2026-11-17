//! Comments of a page (V1, ADR 0012). A thread is a comment on the page with one level of replies.
//! The store keeps the body and the anchor as given: it never reads the Markdown, nor the Yjs
//! positions.
//!
//! Reading comments is `ReadComments` on the page, writing one or resolving a thread is
//! `WriteComment`, changing one's own is `EditOwnComment`, another's `EditOtherComment`. A comment
//! on a page the user cannot read, or on a trashed one, is `CommentNotFound`.

use std::future::Future;

use crate::{PageSite, StoreError};

/// A user named by a comment, `None` where the account is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentUser {
    pub id: String,
    pub name: String,
}

/// Where a comment points in the document: two encoded Yjs `RelativePosition`, and the quoted
/// text shown once they no longer resolve.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anchor {
    pub start: Vec<u8>,
    pub end: Vec<u8>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: String,
    pub page_id: String,
    /// The thread a reply belongs to, `None` for a thread.
    pub parent_id: Option<String>,
    pub author: Option<CommentUser>,
    /// Markdown.
    pub body: String,
    pub anchor: Option<Anchor>,
    pub resolved_at: Option<i64>,
    pub resolved_by: Option<CommentUser>,
    pub created_at: i64,
    pub updated_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Thread {
    pub comment: Comment,
    /// Oldest first.
    pub replies: Vec<Comment>,
}

#[derive(Debug, Clone)]
pub struct NewComment {
    pub id: String,
    pub page_id: String,
    pub parent_id: Option<String>,
    pub actor: String,
    pub body: String,
    pub anchor: Option<Anchor>,
    pub now: i64,
}

/// A comment just changed, with where its page sits, for the event the handler publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentChange {
    pub comment: Comment,
    pub site: PageSite,
}

pub trait CommentStore: Send + Sync + 'static {
    /// At most `limit` threads of the page, oldest first, after the one with the id `after`, each
    /// with all its replies. Refused with `PageNotFound`.
    fn threads(
        &self,
        page_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Thread>, StoreError>> + Send;

    /// A thread, or a reply to a thread of the same page. Refused with `PageNotFound`,
    /// `NotAllowed`, `UnknownParent` for a parent that is not a comment of the page, or
    /// `NestedReply` for a parent that is itself a reply. A reply's anchor is the caller's to
    /// refuse.
    fn create_comment(
        &self,
        comment: &NewComment,
    ) -> impl Future<Output = Result<CommentChange, StoreError>> + Send;

    /// Replaces the body. Refused with `CommentNotFound` or `NotAllowed`.
    fn edit_comment(
        &self,
        comment_id: &str,
        actor: &str,
        body: &str,
        now: i64,
    ) -> impl Future<Output = Result<CommentChange, StoreError>> + Send;

    /// Deletes the comment, a thread with its replies, and answers what it was. Refused with
    /// `CommentNotFound` or `NotAllowed`.
    fn delete_comment(
        &self,
        comment_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<CommentChange, StoreError>> + Send;

    /// Resolves or reopens a thread. Resolving a resolved thread keeps who did it first. Refused
    /// with `CommentNotFound`, `NotAllowed`, or `NotAThread` for a reply.
    fn resolve_comment(
        &self,
        comment_id: &str,
        actor: &str,
        resolved: bool,
        now: i64,
    ) -> impl Future<Output = Result<CommentChange, StoreError>> + Send;
}
