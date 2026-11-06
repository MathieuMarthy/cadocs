//! Users, sessions and the workspaces a user belongs to.

use std::future::Future;

use crate::StoreError;

/// The account made at first run: an instance admin who owns the first workspace.
#[derive(Debug, Clone)]
pub struct FirstUser {
    pub id: String,
    pub email: String,
    pub name: String,
    pub password_hash: String,
    pub workspace_id: String,
    pub workspace_slug: String,
    pub workspace_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: String,
    pub email: String,
    pub name: String,
    pub is_instance_admin: bool,
}

/// An account as an instance admin sees it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UserRecord {
    pub user: User,
    pub disabled_at: Option<i64>,
    pub created_at: i64,
}

/// What a login needs. A disabled user is returned too, so that the caller does the same work for
/// everyone and timing does not tell them apart.
#[derive(Debug, Clone)]
pub struct Credentials {
    pub user: User,
    pub password_hash: Option<String>,
    pub disabled: bool,
}

#[derive(Debug, Clone)]
pub struct NewSession {
    pub token_hash: String,
    pub user_id: String,
    pub now: i64,
    pub expires_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveSession {
    pub user: User,
    pub last_seen_at: i64,
}

/// What an open socket's session has become.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStanding {
    Active,
    /// The account stays, and a reversible decision keeps it out.
    Disabled,
    /// Logged out, expired, revoked, or the account is gone.
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Membership {
    pub workspace_id: String,
    pub slug: String,
    pub name: String,
    pub role: String,
}

/// Fields left to `None` keep their value.
#[derive(Debug, Clone, Default)]
pub struct UserChanges {
    pub name: Option<String>,
    pub email: Option<String>,
    pub password_hash: Option<String>,
}

/// Fields left to `None` keep their value.
#[derive(Debug, Clone, Copy, Default)]
pub struct AdminChanges {
    pub disabled: Option<bool>,
    pub is_instance_admin: Option<bool>,
}

#[derive(Debug, Clone)]
pub struct NewPasswordReset {
    pub token_hash: String,
    pub user_id: String,
    /// `None` when it comes from the command line.
    pub created_by: Option<String>,
    pub now: i64,
    pub expires_at: i64,
}

/// What a change was authorized with. Checking a password takes a while, and another session may
/// change it or revoke this one in the meantime.
#[derive(Debug, Clone, Copy)]
pub struct UpdateBasis<'a> {
    /// The hash of the session making the change.
    pub session: &'a str,
    /// The password hash the caller verified, if the change needed that.
    pub checked_hash: Option<&'a str>,
}

pub trait AccountStore: Send + Sync + 'static {
    /// Whether first-run setup is over.
    fn has_users(&self) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Creates the user, their workspace and their session in one transaction. Only the first
    /// call succeeds, the others get `AlreadySetUp`.
    fn create_first_user(
        &self,
        user: &FirstUser,
        session: &NewSession,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Finds an account by its lowercased email.
    fn credentials(
        &self,
        email: &str,
    ) -> impl Future<Output = Result<Option<Credentials>, StoreError>> + Send;

    fn password_hash(
        &self,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<String>, StoreError>> + Send;

    /// Opens a session, unless the user is disabled or their password is no longer
    /// `checked_hash`: hashing takes a while, and the password may change in the meantime. Returns
    /// whether the session exists.
    fn create_session(
        &self,
        session: &NewSession,
        checked_hash: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Returns a session only if it has not expired and its user is enabled.
    fn session(
        &self,
        token_hash: &str,
        now: i64,
    ) -> impl Future<Output = Result<Option<ActiveSession>, StoreError>> + Send;

    /// `user_id` is who opened the session: disabling an account deletes its sessions, and the
    /// answer must still tell that apart from a logout.
    fn session_standing(
        &self,
        token_hash: &str,
        user_id: &str,
        now: i64,
    ) -> impl Future<Output = Result<SessionStanding, StoreError>> + Send;

    /// Records a use of the session and extends it.
    fn touch_session(
        &self,
        token_hash: &str,
        now: i64,
        expires_at: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    fn delete_session(
        &self,
        token_hash: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The workspaces of a user, oldest first.
    fn memberships(
        &self,
        user_id: &str,
    ) -> impl Future<Output = Result<Vec<Membership>, StoreError>> + Send;

    /// Applies `changes` in one transaction. A new password also voids the user's reset link and
    /// ends every session of the user but `basis.session`. An email another account uses is an `EmailTaken`, and a `basis` that no
    /// longer holds is a `StaleCredentials`.
    fn update_user(
        &self,
        user_id: &str,
        changes: &UserChanges,
        basis: &UpdateBasis<'_>,
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Deletes an account and its sessions. Refused with `LastOwner` or `LastInstanceAdmin`, and
    /// checked inside the transaction, so that two deletions at once cannot both pass. An account
    /// that does not exist is a `UserNotFound`. The reset links they made for others go too.
    fn delete_user(&self, user_id: &str) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// At most `limit` accounts, oldest first, after the one with the id `after`.
    fn list_users(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<UserRecord>, StoreError>> + Send;

    /// Applies `changes` and returns the account. Disabling also ends the user's sessions and
    /// voids their reset link, so that enabling them again revives neither. Disabling or demoting
    /// voids the links they made for others. Refused with `LastInstanceAdmin` or `UserNotFound`,
    /// checked inside the transaction.
    fn administer_user(
        &self,
        user_id: &str,
        changes: AdminChanges,
        now: i64,
    ) -> impl Future<Output = Result<UserRecord, StoreError>> + Send;

    /// Stores a reset link in place of the user's previous one. Refused with `UserNotFound` or
    /// `UserDisabled`.
    fn create_password_reset(
        &self,
        reset: &NewPasswordReset,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Whether a reset link can be used now. Only `use_password_reset` decides: this spares a hash
    /// to a token that has no chance.
    fn password_reset_usable(
        &self,
        token_hash: &str,
        now: i64,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Sets the password, ends every session of the user and voids their link, in one transaction.
    /// Returns whether the link was still usable: hashing takes a while, and it may have been
    /// used, replaced or expired in the meantime.
    fn use_password_reset(
        &self,
        token_hash: &str,
        password_hash: &str,
        now: i64,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}
