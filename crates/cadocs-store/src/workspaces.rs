//! Workspaces, their members and their invitations.
//!
//! A write takes the user who asks for it and checks their role inside its transaction, so that
//! two changes at once cannot each pass on what the other is about to undo.

use std::future::Future;

use cadocs_core::permissions::WorkspaceRole;

use crate::{Membership, NewSession, StoreError};

/// Slugs stay this short, numbers included.
pub const MAX_SLUG_CHARS: usize = 64;

#[derive(Debug, Clone)]
pub struct NewWorkspace {
    pub id: String,
    /// What the slug is built from. A taken one gets a number.
    pub slug: String,
    pub name: String,
    pub owner_id: String,
    pub now: i64,
}

/// A workspace as one of its members sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    pub membership: Membership,
    pub language: String,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Fields left to `None` keep their value.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceChanges {
    pub name: Option<String>,
    pub language: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub id: String,
    pub email: String,
    pub name: String,
    pub role: WorkspaceRole,
    pub disabled: bool,
    pub created_at: i64,
}

#[derive(Debug, Clone)]
pub struct NewInvitation {
    pub id: String,
    pub workspace_id: String,
    pub email: String,
    pub role: WorkspaceRole,
    pub token_hash: String,
    pub invited_by: String,
    pub now: i64,
    pub expires_at: i64,
}

/// An invitation as the admins of its workspace see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    pub id: String,
    pub email: String,
    pub role: WorkspaceRole,
    pub invited_by: String,
    pub expires_at: i64,
    pub created_at: i64,
}

/// What a link shows to whoever holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InvitationPreview {
    pub workspace_name: String,
    pub email: String,
    pub role: WorkspaceRole,
    pub expires_at: i64,
    /// Whether the invited email already has an account, which then has to log in.
    pub has_account: bool,
}

/// The account a link makes.
#[derive(Debug, Clone)]
pub struct InvitedUser {
    pub id: String,
    pub name: String,
    pub password_hash: String,
}

pub trait WorkspaceStore: Send + Sync + 'static {
    /// Creates a workspace owned by `owner_id`, who must be an enabled instance admin, or it is a
    /// `NotAllowed`.
    fn create_workspace(
        &self,
        workspace: &NewWorkspace,
    ) -> impl Future<Output = Result<Workspace, StoreError>> + Send;

    /// At most `limit` workspaces of a user, oldest first, after the one with the id `after`.
    fn list_workspaces(
        &self,
        user_id: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Membership>, StoreError>> + Send;

    /// `None` unless the user is a member.
    fn workspace(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<Workspace>, StoreError>> + Send;

    /// Refused with `WorkspaceNotFound` for a stranger and `NotAllowed` for a member.
    fn update_workspace(
        &self,
        workspace_id: &str,
        actor: &str,
        changes: &WorkspaceChanges,
        now: i64,
    ) -> impl Future<Output = Result<Workspace, StoreError>> + Send;

    /// Refused with `WorkspaceNotFound`, `NotAllowed`, or `NameMismatch` when `confirmation` is
    /// not the name of the workspace.
    fn delete_workspace(
        &self,
        workspace_id: &str,
        actor: &str,
        confirmation: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// At most `limit` members, oldest account first, after the one with the id `after`. Refused
    /// with `WorkspaceNotFound` for a stranger.
    fn list_members(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Member>, StoreError>> + Send;

    /// Refused with `WorkspaceNotFound`, `MemberNotFound`, `NotAllowed` or `LastOwner`. A user who
    /// can no longer invite loses the links they made here.
    fn set_member_role(
        &self,
        workspace_id: &str,
        actor: &str,
        user_id: &str,
        role: WorkspaceRole,
    ) -> impl Future<Output = Result<Member, StoreError>> + Send;

    /// Refused like `set_member_role`, and ends the links of the user the same way. A member can
    /// always remove themself, unless `LastOwner`.
    fn remove_member(
        &self,
        workspace_id: &str,
        actor: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Stores an invitation in place of the last one for that email. Refused with
    /// `WorkspaceNotFound`, `NotAllowed`, or `AlreadyMember`.
    fn create_invitation(
        &self,
        invitation: &NewInvitation,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// At most `limit` invitations that can still be used, oldest first. Refused with
    /// `WorkspaceNotFound` or `NotAllowed`.
    fn list_invitations(
        &self,
        workspace_id: &str,
        actor: &str,
        now: i64,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Invitation>, StoreError>> + Send;

    /// An `InvitationNotFound` for whoever may not manage its workspace, so that nothing tells
    /// them it exists.
    fn revoke_invitation(
        &self,
        invitation_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// `None` unless the link can be used now.
    fn invitation(
        &self,
        token_hash: &str,
        now: i64,
    ) -> impl Future<Output = Result<Option<InvitationPreview>, StoreError>> + Send;

    /// Uses the link for a new account, and opens its session, in one transaction. Returns the
    /// workspaces of the account. Refused with `InvitationNotFound` or `EmailTaken`: hashing takes
    /// a while, and the link may be gone, or the email taken, in the meantime.
    fn accept_as_new_user(
        &self,
        token_hash: &str,
        user: &InvitedUser,
        session: &NewSession,
    ) -> impl Future<Output = Result<Vec<Membership>, StoreError>> + Send;

    /// Uses the link for an account that has the invited email, and returns its workspaces. A
    /// user who is already a member keeps their role. Refused with `InvitationNotFound`, or with
    /// `NotAllowed` for any other account.
    fn accept_as_user(
        &self,
        token_hash: &str,
        user_id: &str,
        now: i64,
    ) -> impl Future<Output = Result<Vec<Membership>, StoreError>> + Send;
}
