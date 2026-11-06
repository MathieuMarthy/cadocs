//! Spaces and their grants.
//!
//! A method takes the user who asks and resolves their role itself, inside the transaction for a
//! write. A space the user cannot read is a `SpaceNotFound`, so nothing tells them it exists.

use std::future::Future;

use cadocs_core::permissions::{DefaultRole, SpaceRole};

use crate::StoreError;

#[derive(Debug, Clone)]
pub struct NewSpace {
    pub id: String,
    pub workspace_id: String,
    /// What the slug is built from. A taken one gets a number.
    pub slug: String,
    pub name: String,
    pub description: String,
    pub default_role: Option<DefaultRole>,
    pub actor: String,
    pub now: i64,
}

/// A space as one user sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Space {
    pub id: String,
    pub workspace_id: String,
    pub slug: String,
    pub name: String,
    pub description: String,
    pub default_role: Option<DefaultRole>,
    /// What resolution gives this user.
    pub role: SpaceRole,
    pub deleted_at: Option<i64>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Fields left to `None` keep their value. `Some(None)` clears the default role.
#[derive(Debug, Clone, Default)]
pub struct SpaceChanges {
    pub name: Option<String>,
    pub description: Option<String>,
    pub default_role: Option<Option<DefaultRole>>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum GrantTarget {
    User(String),
    Group(String),
}

#[derive(Debug, Clone)]
pub struct NewGrant {
    pub id: String,
    pub target: GrantTarget,
    pub role: SpaceRole,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub id: String,
    pub target: GrantTarget,
    pub role: SpaceRole,
    pub created_at: i64,
}

pub trait SpaceStore: Send + Sync + 'static {
    /// Refused with `WorkspaceNotFound` for a stranger and `NotAllowed` for a member.
    fn create_space(
        &self,
        space: &NewSpace,
    ) -> impl Future<Output = Result<Space, StoreError>> + Send;

    /// At most `limit` spaces the user can read, oldest first, after the one with the id `after`.
    /// Refused with `WorkspaceNotFound` for a stranger.
    fn list_spaces(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Space>, StoreError>> + Send;

    /// Like `list_spaces`, for the trashed spaces the user can restore.
    fn list_trashed_spaces(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Space>, StoreError>> + Send;

    /// `None` unless the user can read the space.
    fn space(
        &self,
        space_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<Option<Space>, StoreError>> + Send;

    /// Refused with `SpaceNotFound` or `NotAllowed`.
    fn update_space(
        &self,
        space_id: &str,
        actor: &str,
        changes: &SpaceChanges,
        now: i64,
    ) -> impl Future<Output = Result<Space, StoreError>> + Send;

    /// Refused with `SpaceNotFound` or `NotAllowed`.
    fn trash_space(
        &self,
        space_id: &str,
        actor: &str,
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Refused with `SpaceNotFound`, or `NotAllowed` for a space that is not in the trash.
    fn restore_space(
        &self,
        space_id: &str,
        actor: &str,
        now: i64,
    ) -> impl Future<Output = Result<Space, StoreError>> + Send;

    /// At most `limit` grants, oldest first, after the one with the id `after`. Refused with
    /// `SpaceNotFound`.
    fn list_grants(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Grant>, StoreError>> + Send;

    /// Makes these the grants of the space. A target that already has one keeps its id and date.
    /// Refused with `SpaceNotFound`, `NotAllowed`, `DuplicateGrant`, or `UnknownGrantTarget` for a
    /// user or group outside the workspace.
    fn set_grants(
        &self,
        space_id: &str,
        actor: &str,
        grants: &[NewGrant],
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}
