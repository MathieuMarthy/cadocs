//! Comments of a page (V1, ADR 0012). A thread is a comment on the page with one level of replies.
//! The store keeps the body and the anchor as given: it never reads the body, nor the Yjs
//! positions. The caller reads the users a body mentions, which the store records.
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
    /// Plain text with mentions (`cadocs_content::comment`).
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
    /// The users the body mentions: those of the workspace are recorded.
    pub mentions: Vec<String>,
    pub now: i64,
}

/// A comment just changed, with where its page sits, for the event the handler publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommentChange {
    pub comment: Comment,
    pub site: PageSite,
    /// The notifications the change made, for the event each user gets.
    pub notified: Vec<Notified>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notified {
    pub id: String,
    pub user_id: String,
}

/// Why a user is told: they are mentioned, someone replied to their comment, or someone
/// commented on their page.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotificationKind {
    Mention,
    Reply,
    Comment,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: String,
    pub kind: NotificationKind,
    pub page_id: String,
    pub space_id: String,
    pub page_title: String,
    /// `None` for a mention in the page's body.
    pub comment_id: Option<String>,
    pub actor: Option<CommentUser>,
    pub read: bool,
    /// A message by mail already told of it (step 42).
    pub mailed: bool,
    pub created_at: i64,
}

/// What one message by mail tells a user: the notifications they have neither read nor been
/// mailed, newest first.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MailDigest {
    pub email: String,
    pub name: String,
    pub items: Vec<Notification>,
}

/// The job that mails a user's new notifications, with `{"user": "<id>"}`, keyed `mail:<id>`.
pub const MAIL_NOTIFICATIONS: &str = "mail_notifications";

/// The newest notifications of a user, and how many of them are unread.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inbox {
    pub items: Vec<Notification>,
    pub unread: usize,
}

/// How many replies a thread takes, so a list of threads stays bounded.
pub const MAX_REPLIES: i64 = 100;

/// How many notifications an inbox holds, the newest ones.
pub const INBOX_SIZE: u32 = 100;

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
    /// `NestedReply` for a parent that is itself a reply, `TooManyReplies` past [`MAX_REPLIES`]. A reply's anchor is the caller's to
    /// refuse.
    fn create_comment(
        &self,
        comment: &NewComment,
    ) -> impl Future<Output = Result<CommentChange, StoreError>> + Send;

    /// Replaces the body, and its mentions with `mentions`. Refused with `CommentNotFound` or
    /// `NotAllowed`.
    fn edit_comment(
        &self,
        comment_id: &str,
        actor: &str,
        body: &str,
        mentions: &[String],
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

    /// The newest `INBOX_SIZE` notifications of `user`, newest first, among those whose page the
    /// user can still read. `unread` counts those.
    fn notifications(&self, user: &str) -> impl Future<Output = Result<Inbox, StoreError>> + Send;

    /// What to mail `user`, or `None` when their choice is off, their account is disabled, or
    /// they have nothing new that they may still read.
    fn mail_digest(
        &self,
        user: &str,
    ) -> impl Future<Output = Result<Option<MailDigest>, StoreError>> + Send;

    /// Marks the notifications `ids` of `user` as told by mail.
    fn mark_mailed(
        &self,
        user: &str,
        ids: &[String],
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Marks the notifications `ids` of `user` as read, or all of theirs for `None`. Ids of
    /// other users are ignored.
    fn read_notifications(
        &self,
        user: &str,
        ids: Option<&[String]>,
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}
