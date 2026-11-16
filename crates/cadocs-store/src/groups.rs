//! Groups of a workspace and their members (V1). A grant to a group gives its role to every member
//! of the group, as resolution in `cadocs-core` reads it.
//!
//! Every member of the workspace reads its groups; changing them is `ManageGroups`. A route that
//! names a group answers `GroupNotFound` to whoever is not a member of its workspace, and
//! `NotAllowed` to a member who may not change it.

use std::future::Future;

use crate::{Member, StoreError};

#[derive(Debug, Clone)]
pub struct NewGroup {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub actor: String,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Group {
    pub id: String,
    pub workspace_id: String,
    pub name: String,
    pub member_count: i64,
    pub created_at: i64,
}

pub trait GroupStore: Send + Sync + 'static {
    /// Refused with `WorkspaceNotFound`, `NotAllowed`, or `GroupNameTaken`.
    fn create_group(
        &self,
        group: &NewGroup,
    ) -> impl Future<Output = Result<Group, StoreError>> + Send;

    /// At most `limit` groups, oldest first, after the one with the id `after`. Refused with
    /// `WorkspaceNotFound` for a stranger.
    fn list_groups(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Group>, StoreError>> + Send;

    /// Refused with `GroupNotFound`, `NotAllowed`, or `GroupNameTaken`.
    fn rename_group(
        &self,
        group_id: &str,
        actor: &str,
        name: &str,
    ) -> impl Future<Output = Result<Group, StoreError>> + Send;

    /// Deletes the group with its memberships and grants. Refused with `GroupNotFound` or
    /// `NotAllowed`.
    fn delete_group(
        &self,
        group_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// At most `limit` members of the group, oldest account first, after the one with the id
    /// `after`. Refused with `GroupNotFound`.
    fn list_group_members(
        &self,
        group_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Member>, StoreError>> + Send;

    /// Adds a member of the group's workspace, or keeps one already there. Refused with
    /// `GroupNotFound`, `NotAllowed`, or `MemberNotFound` for a user outside the workspace.
    fn add_group_member(
        &self,
        group_id: &str,
        actor: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Refused with `GroupNotFound`, `NotAllowed`, or `MemberNotFound` for a user who is not in
    /// the group.
    fn remove_group_member(
        &self,
        group_id: &str,
        actor: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}
