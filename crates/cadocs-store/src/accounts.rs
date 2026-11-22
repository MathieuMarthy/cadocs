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
    /// Whether a login needs a code by mail after the password (ADR 0026).
    pub email_2fa: bool,
    /// Whether a message by mail tells of the notifications the user has not read (step 42).
    pub email_notifications: bool,
    /// Whether a login needs a code from an authenticator app (RFC 6238). It replaces the code by
    /// mail when both are on.
    pub totp: bool,
    /// The language of the interface, `None` to follow the browser's.
    pub language: Option<String>,
}

/// What stands in for the code of the app at login.
#[derive(Debug, Clone, Copy)]
pub enum TotpProof<'a> {
    /// The time step whose code matched, and the secret it was checked against.
    App { secret: &'a [u8], step: i64 },
    /// The hash of a recovery code.
    Recovery { code_hash: &'a str },
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
    /// The secret of the authenticator app, once it is on.
    pub totp_secret: Option<Vec<u8>>,
}

impl Credentials {
    /// Whether a login needs a code by mail, given whether the instance sends any. Without a
    /// relay the factor is not asked (ADR 0026).
    pub fn needs_code(&self, mail: bool) -> bool {
        mail && self.user.email_2fa
    }
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
    /// Only to turn the factor off: turning it on needs a code (`enable_email_2fa`).
    pub email_2fa_off: bool,
    /// Turns the authenticator app off, or ends its setup, and deletes the recovery codes.
    pub totp_off: bool,
    /// `Some(None)` clears it.
    pub language: Option<Option<String>>,
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

/// The account a registration makes: no workspace, no instance role, a verified address.
#[derive(Debug, Clone)]
pub struct NewUser {
    pub id: String,
    pub email: String,
    pub name: String,
    pub password_hash: String,
}

/// The account an OpenID Connect provider's first login makes: no password, no workspace, no
/// instance role, and the identity that logs into it from then on.
#[derive(Debug, Clone)]
pub struct IdentityUser {
    pub id: String,
    pub email: String,
    pub name: String,
    /// Whether the provider says it proved the address.
    pub email_verified: bool,
    pub provider: String,
    pub subject: String,
}

/// Wrong tries that kill a code (ADR 0026).
pub const MAX_CODE_TRIES: i64 = 5;

/// What a code sent by mail proves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodePurpose {
    Register,
    /// The second factor of a login.
    Login,
    /// That the account's holder reads an address: its own, to turn the second factor on, or a
    /// new one, to move to it.
    Email,
}

impl CodePurpose {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Register => "register",
            Self::Login => "login",
            Self::Email => "email",
        }
    }
}

/// A code to store in place of the one of the same purpose and address.
#[derive(Debug, Clone)]
pub struct NewEmailCode {
    pub purpose: CodePurpose,
    /// Lowercased.
    pub email: String,
    /// The account it is for, `None` for a registration: the code goes with the account.
    pub user_id: Option<String>,
    pub code_hash: String,
    pub now: i64,
    pub expires_at: i64,
}

/// A code someone typed, for an address.
#[derive(Debug, Clone, Copy)]
pub struct CodeAttempt<'a> {
    pub purpose: CodePurpose,
    pub email: &'a str,
    pub code_hash: &'a str,
    pub now: i64,
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
    /// `checked_hash`: hashing takes a while, and the password may change in the meantime. Nor
    /// when a second factor went on meanwhile: the authenticator app, or the code by mail when
    /// `mail` says the instance asks for it. Returns whether the session exists.
    fn create_session(
        &self,
        session: &NewSession,
        checked_hash: &str,
        mail: bool,
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
    /// ends every session of the user but `basis.session`. A new email is one nobody proved: it
    /// clears `email_verified_at`, turns the second factor off and voids the reset link. An email another account uses
    /// is an `EmailTaken`, and a `basis` that no longer holds is a `StaleCredentials`.
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

    /// Stores a code in place of the one of the same purpose and address, and deletes the codes
    /// that expired.
    fn create_email_code(
        &self,
        code: &NewEmailCode,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Uses a registration code, then makes the account, verified, and its session, in one
    /// transaction. A code that is missing, expired or other is a `WrongCode`, and a wrong one
    /// counts a try that is kept: the `MAX_CODE_TRIES`th deletes the code. An address another
    /// account took meanwhile is an `EmailTaken`, and the code stays. Before first-run setup every
    /// code is a `WrongCode`.
    fn register_user(
        &self,
        user: &NewUser,
        attempt: &CodeAttempt<'_>,
        session: &NewSession,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Uses a login code of the account `session.user_id`, then opens the session, in one
    /// transaction. Returns `false`, with no try counted, when `create_session` would: the user
    /// is disabled, or their password is no longer `checked_hash`. A code that is missing, expired
    /// or other is a `WrongCode`, counted as for `register_user`. The code proves the address.
    fn create_session_with_code(
        &self,
        session: &NewSession,
        checked_hash: &str,
        attempt: &CodeAttempt<'_>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Uses a code sent to the account's address, then turns the second factor on and marks the
    /// address verified. A code that is missing, expired or other, or for an address the account
    /// no longer has, is a `WrongCode`, counted as for `register_user`.
    fn enable_email_2fa(
        &self,
        user_id: &str,
        attempt: &CodeAttempt<'_>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Uses an `email` code that went to `attempt.email` for this account, then makes it the
    /// account's address, verified, and voids its reset link. A code that is missing, expired, other or of another account
    /// is a `WrongCode`, counted as for `register_user` when it is this account's. An address
    /// another account took meanwhile is an `EmailTaken`, and the code stays.
    fn change_email(
        &self,
        user_id: &str,
        attempt: &CodeAttempt<'_>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Turns the mail of notifications on or off for a user.
    fn set_email_notifications(
        &self,
        user_id: &str,
        on: bool,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// How many enabled accounts have the second factor on, for the notice at start.
    fn email_2fa_users(&self) -> impl Future<Output = Result<u64, StoreError>> + Send;

    /// Stores a new secret for the authenticator app, waiting for its first code, in place of a
    /// setup not finished. Refused with `TotpOn` while the app is on, and `StaleCredentials` as
    /// `update_user`.
    fn start_totp(
        &self,
        user_id: &str,
        secret: &[u8],
        basis: &UpdateBasis<'_>,
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The secret of a setup waiting for its first code.
    fn pending_totp(
        &self,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<Vec<u8>>, StoreError>> + Send;

    /// Turns the app on with the code of `step`, if the setup still holds `secret`, and stores
    /// the recovery codes. Anything else is a `WrongCode`.
    fn enable_totp(
        &self,
        user_id: &str,
        secret: &[u8],
        step: i64,
        recovery_hashes: &[String],
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Opens a session with a second factor from the app, as `create_session_with_code` does with a
    /// code by mail. A code of a step at or before the last one used, from a secret the account no
    /// longer has, or a recovery code it does not hold is a `WrongCode`. A recovery code goes.
    fn create_session_with_totp(
        &self,
        session: &NewSession,
        checked_hash: &str,
        proof: &TotpProof<'_>,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Replaces the recovery codes of an account whose app is on, else `TotpOff`.
    /// `StaleCredentials` as `update_user`.
    fn replace_recovery_codes(
        &self,
        user_id: &str,
        recovery_hashes: &[String],
        basis: &UpdateBasis<'_>,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Turns the app off with no password, for the command line.
    fn turn_totp_off(&self, user_id: &str) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The account the subject `subject` of `provider` logs into, disabled or not.
    fn identity_user(
        &self,
        provider: &str,
        subject: &str,
    ) -> impl Future<Output = Result<Option<Credentials>, StoreError>> + Send;

    /// Opens a session with no password, unless the user is disabled: for a login a provider
    /// vouched for. Returns whether the session exists.
    fn create_identity_session(
        &self,
        session: &NewSession,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// Makes the account, its identity and its session in one transaction. An address another
    /// account holds is an `EmailTaken`, and before first-run setup it is a `NotAllowed`.
    fn register_identity(
        &self,
        user: &IdentityUser,
        session: &NewSession,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// The providers `user_id` logs in with, and whether the account has a password.
    fn identities(
        &self,
        user_id: &str,
    ) -> impl Future<Output = Result<(Vec<String>, bool), StoreError>> + Send;

    /// Ties the subject of `provider` to the account. A subject of any account, or a provider the
    /// account already has, is an `IdentityTaken`; a disabled account is a `UserDisabled`.
    fn link_identity(
        &self,
        user_id: &str,
        provider: &str,
        subject: &str,
        now: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Unties `provider`, and says whether it was tied. An account with no password and no other
    /// identity keeps it: it is a `LastLogin`.
    fn unlink_identity(
        &self,
        user_id: &str,
        provider: &str,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}
