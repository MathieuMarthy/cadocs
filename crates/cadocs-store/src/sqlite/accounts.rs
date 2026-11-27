use cadocs_core::permissions::{
    WorkspaceRole, would_remove_last_instance_admin, would_remove_last_owner,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection, SqliteExecutor};

use super::{BEGIN_WRITE, MAIL_LANGUAGE, SqliteStore};
use crate::now_ms;
use crate::{
    AccountStore, ActiveSession, AdminChanges, CodeAttempt, Credentials, FirstUser, IdentityUser,
    MAX_CODE_TRIES, Membership, NewEmailCode, NewPasswordReset, NewSession, NewUser,
    SessionStanding, StoreError, TotpProof, UpdateBasis, User, UserChanges, UserRecord,
};

pub(super) const INSERT_SESSION: &str =
    "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
     VALUES (?1, ?2, ?3, ?3, ?4)";

/// A code sent to the address of user `?1`, `?2`, was used at `?3`.
const VERIFY_ADDRESS: &str = "UPDATE users SET email_verified_at = coalesce(email_verified_at, ?3)
     WHERE id = ?1 AND email = ?2";

const DELETE_RESETS: &str = "DELETE FROM password_resets WHERE user_id = ?1";

/// The codes mailed for user `?1`: none outlives a change of what lets them in.
const DELETE_CODES: &str = "DELETE FROM email_codes WHERE user_id = ?1";

/// Every session of user `?1` but `?2`, the one that changed what lets them in.
const DELETE_OTHER_SESSIONS: &str = "DELETE FROM sessions WHERE user_id = ?1 AND token_hash <> ?2";

/// The links an admin made outlive neither their role nor their account: each one can take over
/// another account, possibly another admin's.
const DELETE_RESETS_MADE_BY: &str = "DELETE FROM password_resets WHERE created_by = ?1";

/// A link that can be used now, and its user.
const USABLE_RESET: &str = "SELECT r.user_id FROM password_resets r
     JOIN users u ON u.id = r.user_id
     WHERE r.token_hash = ?1 AND r.expires_at > ?2 AND u.disabled_at IS NULL";

const SELECT_RECORD: &str =
    "SELECT id, email, name, is_instance_admin, email_2fa, email_notifications, language,
            browser_language,
            disabled_at, created_at,
            EXISTS (SELECT 1 FROM user_totp t WHERE t.user_id = users.id
                                               AND t.enabled_at IS NOT NULL) AS totp
     FROM users";

/// The account of the row in `users` has no authenticator app on.
const NO_APP: &str = "NOT EXISTS (SELECT 1 FROM user_totp t WHERE t.user_id = users.id
                                                  AND t.enabled_at IS NOT NULL)";

/// Turns the app of user `?1` off, or ends its setup.
const DELETE_TOTP: &str = "DELETE FROM user_totp WHERE user_id = ?1";
const DELETE_RECOVERY_CODES: &str = "DELETE FROM user_recovery_codes WHERE user_id = ?1";

fn record(row: &SqliteRow) -> UserRecord {
    UserRecord {
        user: user(row),
        disabled_at: row.get("disabled_at"),
        created_at: row.get("created_at"),
    }
}

pub(super) fn membership(row: &SqliteRow) -> Membership {
    Membership {
        workspace_id: row.get("id"),
        slug: row.get("slug"),
        name: row.get("name"),
        role: row.get("role"),
    }
}

/// At most `limit` workspaces of a user after the id `after`, -1 for all of them.
pub(super) async fn select_memberships<'e>(
    executor: impl SqliteExecutor<'e>,
    user_id: &str,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<Membership>, StoreError> {
    let rows = sqlx::query(
        "SELECT w.id, w.slug, w.name, m.role
         FROM workspace_members m JOIN workspaces w ON w.id = m.workspace_id
         WHERE m.user_id = ?1 AND w.id > coalesce(?2, '')
         ORDER BY w.id LIMIT ?3",
    )
    .bind(user_id)
    .bind(after)
    .bind(limit)
    .fetch_all(executor)
    .await?;
    Ok(rows.iter().map(membership).collect())
}

/// Whether `user_id` is an enabled instance admin, and how many of those there are.
async fn enabled_admins(
    tx: &mut SqliteConnection,
    user_id: &str,
) -> Result<(bool, usize), StoreError> {
    let (is_enabled_admin, count): (bool, i64) = sqlx::query_as(
        "SELECT coalesce(sum(id = ?1), 0) > 0, count(*) FROM users
         WHERE is_instance_admin = 1 AND disabled_at IS NULL",
    )
    .bind(user_id)
    .fetch_one(tx)
    .await?;
    Ok((is_enabled_admin, count as usize))
}

/// Ends everything that lets a user in: their sessions, their reset link and their codes.
async fn revoke_access(tx: &mut SqliteConnection, user_id: &str) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for query in [DELETE_RESETS, DELETE_CODES] {
        sqlx::query(query).bind(user_id).execute(&mut *tx).await?;
    }
    Ok(())
}

/// Whether `attempt` is the code of its purpose and address, if that code still works. A right
/// code is deleted. A wrong one counts a try, and the last try deletes it.
async fn use_code(
    tx: &mut SqliteConnection,
    attempt: &CodeAttempt<'_>,
) -> Result<bool, StoreError> {
    let purpose = attempt.purpose.as_str();
    let stored: Option<(String, i64)> = sqlx::query_as(
        "SELECT code_hash, tries FROM email_codes
         WHERE purpose = ?1 AND email = ?2 AND expires_at > ?3",
    )
    .bind(purpose)
    .bind(attempt.email)
    .bind(attempt.now)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((code_hash, tries)) = stored else {
        return Ok(false);
    };
    let right = code_hash == attempt.code_hash;
    let statement = if right || tries + 1 >= MAX_CODE_TRIES {
        "DELETE FROM email_codes WHERE purpose = ?1 AND email = ?2"
    } else {
        "UPDATE email_codes SET tries = tries + 1 WHERE purpose = ?1 AND email = ?2"
    };
    sqlx::query(statement)
        .bind(purpose)
        .bind(attempt.email)
        .execute(&mut *tx)
        .await?;
    Ok(right)
}

/// Whether the session behind a change is still there, and the password it was checked against
/// still the account's: hashing takes a while.
async fn check_basis(
    tx: &mut SqliteConnection,
    user_id: &str,
    basis: &UpdateBasis<'_>,
) -> Result<(), StoreError> {
    let holds: bool = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM sessions s JOIN users u ON u.id = s.user_id
                        WHERE s.token_hash = ?1 AND u.id = ?2
                          AND (?3 IS NULL OR u.password_hash = ?3))",
    )
    .bind(basis.session)
    .bind(user_id)
    .bind(basis.checked_hash)
    .fetch_one(&mut *tx)
    .await?;
    if holds {
        Ok(())
    } else {
        Err(StoreError::StaleCredentials)
    }
}

async fn turn_totp_off(tx: &mut SqliteConnection, user_id: &str) -> Result<(), StoreError> {
    for statement in [DELETE_TOTP, DELETE_RECOVERY_CODES] {
        sqlx::query(statement)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

async fn insert_recovery_codes(
    tx: &mut SqliteConnection,
    user_id: &str,
    hashes: &[String],
) -> Result<(), StoreError> {
    sqlx::query(DELETE_RECOVERY_CODES)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    for hash in hashes {
        sqlx::query("INSERT INTO user_recovery_codes (user_id, code_hash) VALUES (?1, ?2)")
            .bind(user_id)
            .bind(hash)
            .execute(&mut *tx)
            .await?;
    }
    Ok(())
}

const SELECT_CREDENTIALS: &str =
    "SELECT u.id, u.email, u.name, u.is_instance_admin, u.email_2fa, u.email_notifications, u.language,
            u.browser_language,
            u.password_hash, u.disabled_at, t.secret AS totp_secret, t.secret IS NOT NULL AS totp
     FROM users u LEFT JOIN user_totp t ON t.user_id = u.id AND t.enabled_at IS NOT NULL";

fn credentials(row: &SqliteRow) -> Credentials {
    Credentials {
        user: user(row),
        password_hash: row.get("password_hash"),
        disabled: row.get::<Option<i64>, _>("disabled_at").is_some(),
        totp_secret: row.get("totp_secret"),
    }
}

fn user(row: &SqliteRow) -> User {
    User {
        id: row.get("id"),
        email: row.get("email"),
        name: row.get("name"),
        is_instance_admin: row.get::<i64, _>("is_instance_admin") != 0,
        email_2fa: row.get::<i64, _>("email_2fa") != 0,
        email_notifications: row.get::<i64, _>("email_notifications") != 0,
        totp: row.get("totp"),
        language: row.get("language"),
        browser_language: row.get("browser_language"),
    }
}

impl AccountStore for SqliteStore {
    async fn has_users(&self) -> Result<bool, StoreError> {
        let found: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
            .fetch_one(&self.reader)
            .await?;
        Ok(found)
    }

    async fn create_first_user(
        &self,
        first: &FirstUser,
        session: &NewSession,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let found: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
            .fetch_one(&mut *tx)
            .await?;
        if found {
            return Err(StoreError::AlreadySetUp);
        }

        sqlx::query(
            "INSERT INTO users
                 (id, email, name, password_hash, is_instance_admin, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, 1, ?5, ?5)",
        )
        .bind(&first.id)
        .bind(&first.email)
        .bind(&first.name)
        .bind(&first.password_hash)
        .bind(session.now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
        )
        .bind(&first.workspace_id)
        .bind(&first.workspace_slug)
        .bind(&first.workspace_name)
        .bind(session.now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             VALUES (?1, ?2, 'owner', ?3)",
        )
        .bind(&first.workspace_id)
        .bind(&first.id)
        .bind(session.now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(INSERT_SESSION)
            .bind(&session.token_hash)
            .bind(&session.user_id)
            .bind(session.now)
            .bind(session.expires_at)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    async fn credentials(&self, email: &str) -> Result<Option<Credentials>, StoreError> {
        let row = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_CREDENTIALS} WHERE u.email = ?1"
        )))
        .bind(email)
        .fetch_optional(&self.reader)
        .await?;
        Ok(row.as_ref().map(credentials))
    }

    async fn password_hash(&self, user_id: &str) -> Result<Option<String>, StoreError> {
        let hash = sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
            .bind(user_id)
            .fetch_optional(&self.reader)
            .await?;
        Ok(hash.flatten())
    }

    async fn create_session(
        &self,
        session: &NewSession,
        checked_hash: &str,
        mail: bool,
    ) -> Result<bool, StoreError> {
        // The handler chose no factor from what it read before the hash: one turned on since
        // must not be skipped.
        let created = sqlx::query(AssertSqlSafe(format!(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
             SELECT ?1, id, ?3, ?3, ?4 FROM users
             WHERE id = ?2 AND password_hash = ?5 AND disabled_at IS NULL
               AND NOT (?6 AND email_2fa = 1) AND {NO_APP}"
        )))
        .bind(&session.token_hash)
        .bind(&session.user_id)
        .bind(session.now)
        .bind(session.expires_at)
        .bind(checked_hash)
        .bind(mail)
        .execute(&self.writer)
        .await?
        .rows_affected();
        Ok(created == 1)
    }

    async fn session(
        &self,
        token_hash: &str,
        now: i64,
    ) -> Result<Option<ActiveSession>, StoreError> {
        let row = sqlx::query(
            "SELECT u.id, u.email, u.name, u.is_instance_admin, u.email_2fa, u.email_notifications, u.language,
                    u.browser_language,
                    s.last_seen_at,
                    EXISTS (SELECT 1 FROM user_totp t WHERE t.user_id = u.id
                                                       AND t.enabled_at IS NOT NULL) AS totp
             FROM sessions s JOIN users u ON u.id = s.user_id
             WHERE s.token_hash = ?1 AND s.expires_at > ?2 AND u.disabled_at IS NULL",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.reader)
        .await?;
        Ok(row.map(|row| ActiveSession {
            user: user(&row),
            last_seen_at: row.get("last_seen_at"),
        }))
    }

    async fn session_standing(
        &self,
        token_hash: &str,
        user_id: &str,
        now: i64,
    ) -> Result<SessionStanding, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let row = sqlx::query(
            "SELECT u.disabled_at IS NOT NULL AS disabled,
                 EXISTS (SELECT 1 FROM sessions s WHERE s.token_hash = ?1 AND s.user_id = u.id
                                                    AND s.expires_at > ?3) AS live
             FROM users u WHERE u.id = ?2",
        )
        .bind(token_hash)
        .bind(user_id)
        .bind(now)
        .fetch_optional(&mut *reader)
        .await?;
        Ok(match row {
            Some(row) if row.get::<bool, _>("disabled") => SessionStanding::Disabled,
            Some(row) if row.get::<bool, _>("live") => SessionStanding::Active,
            _ => SessionStanding::Ended,
        })
    }

    async fn touch_session(
        &self,
        token_hash: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<(), StoreError> {
        sqlx::query("UPDATE sessions SET last_seen_at = ?2, expires_at = ?3 WHERE token_hash = ?1")
            .bind(token_hash)
            .bind(now)
            .bind(expires_at)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    async fn delete_session(&self, token_hash: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM sessions WHERE token_hash = ?1")
            .bind(token_hash)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    async fn memberships(&self, user_id: &str) -> Result<Vec<Membership>, StoreError> {
        select_memberships(&self.reader, user_id, None, -1).await
    }

    async fn update_user(
        &self,
        user_id: &str,
        changes: &UserChanges,
        basis: &UpdateBasis<'_>,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let session_alive: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM sessions WHERE token_hash = ?1 AND user_id = ?2)",
        )
        .bind(basis.session)
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        let stored: Option<(Option<String>, String)> =
            sqlx::query_as("SELECT password_hash, email FROM users WHERE id = ?1")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        let (stored, old_email) = stored.unzip();
        let password_moved = basis
            .checked_hash
            .is_some_and(|checked| stored.flatten().as_deref() != Some(checked));
        let email_moved = changes.email.is_some() && changes.email != old_email;
        if !session_alive || password_moved {
            return Err(StoreError::StaleCredentials);
        }

        sqlx::query(
            "UPDATE users SET
                 name = COALESCE(?2, name),
                 email = COALESCE(?3, email),
                 password_hash = COALESCE(?4, password_hash),
                 email_2fa = CASE WHEN ?6 OR ?3 <> email THEN 0 ELSE email_2fa END,
                 email_verified_at = CASE WHEN ?3 <> email THEN NULL
                                          ELSE email_verified_at END,
                 language = CASE WHEN ?7 THEN ?8 ELSE language END,
                 updated_at = ?5
             WHERE id = ?1",
        )
        .bind(user_id)
        .bind(&changes.name)
        .bind(&changes.email)
        .bind(&changes.password_hash)
        .bind(now)
        .bind(changes.email_2fa_off)
        .bind(changes.language.is_some())
        .bind(changes.language.clone().flatten())
        .execute(&mut *tx)
        .await
        .map_err(|error| match error.as_database_error() {
            Some(database) if database.is_unique_violation() => StoreError::EmailTaken,
            _ => error.into(),
        })?;

        if changes.password_hash.is_some() {
            sqlx::query(DELETE_OTHER_SESSIONS)
                .bind(user_id)
                .bind(basis.session)
                .execute(&mut *tx)
                .await?;
            sqlx::query(DELETE_CODES)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        // A link mailed to the old address must not reach the account any more.
        if changes.password_hash.is_some() || email_moved {
            sqlx::query(DELETE_RESETS)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        if changes.totp_off {
            turn_totp_off(&mut tx, user_id).await?;
        }
        tx.commit().await?;
        Ok(())
    }

    async fn delete_user(&self, user_id: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;

        // The owners who would stay. A disabled one cannot act, so they do not count, and this
        // user goes whether or not they are enabled.
        let owners_left_in_each_workspace: Vec<i64> = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM workspace_members o JOIN users u ON u.id = o.user_id
                     WHERE o.workspace_id = m.workspace_id AND o.role = 'owner'
                       AND u.disabled_at IS NULL AND u.id <> ?1)
             FROM workspace_members m WHERE m.user_id = ?1 AND m.role = 'owner'",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        let last_owner = owners_left_in_each_workspace
            .into_iter()
            .any(|left| would_remove_last_owner(WorkspaceRole::Owner, None, left as usize + 1));
        if last_owner {
            return Err(StoreError::LastOwner);
        }

        let (is_enabled_admin, admins) = enabled_admins(&mut tx, user_id).await?;
        if would_remove_last_instance_admin(is_enabled_admin, admins) {
            return Err(StoreError::LastInstanceAdmin);
        }

        sqlx::query(DELETE_RESETS_MADE_BY)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        let deleted = sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if deleted == 0 {
            return Err(StoreError::UserNotFound);
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_users(
        &self,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<UserRecord>, StoreError> {
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_RECORD} WHERE id > coalesce(?1, '') ORDER BY id LIMIT ?2"
        )))
        .bind(after)
        .bind(limit)
        .fetch_all(&self.reader)
        .await?;
        Ok(rows.iter().map(record).collect())
    }

    async fn administer_user(
        &self,
        user_id: &str,
        changes: AdminChanges,
        now: i64,
    ) -> Result<UserRecord, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;

        let loses_admin =
            changes.disabled == Some(true) || changes.is_instance_admin == Some(false);
        if loses_admin {
            let (is_enabled_admin, admins) = enabled_admins(&mut tx, user_id).await?;
            if would_remove_last_instance_admin(is_enabled_admin, admins) {
                return Err(StoreError::LastInstanceAdmin);
            }
        }

        let updated = sqlx::query(
            "UPDATE users SET
                 disabled_at = CASE ?2 WHEN 1 THEN coalesce(disabled_at, ?4)
                                       WHEN 0 THEN NULL
                                       ELSE disabled_at END,
                 is_instance_admin = coalesce(?3, is_instance_admin),
                 updated_at = ?4
             WHERE id = ?1",
        )
        .bind(user_id)
        .bind(changes.disabled)
        .bind(changes.is_instance_admin)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if updated == 0 {
            return Err(StoreError::UserNotFound);
        }

        if loses_admin {
            sqlx::query(DELETE_RESETS_MADE_BY)
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }
        if changes.disabled == Some(true) {
            revoke_access(&mut tx, user_id).await?;
            // Like a reset link, an invitation is a way back in. Deleting a user cascades to them.
            sqlx::query("DELETE FROM workspace_invitations WHERE invited_by = ?1")
                .bind(user_id)
                .execute(&mut *tx)
                .await?;
        }

        let row = sqlx::query(AssertSqlSafe(format!("{SELECT_RECORD} WHERE id = ?1")))
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(record(&row))
    }

    async fn create_password_reset(&self, reset: &NewPasswordReset) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let disabled_at: Option<Option<i64>> =
            sqlx::query_scalar("SELECT disabled_at FROM users WHERE id = ?1")
                .bind(&reset.user_id)
                .fetch_optional(&mut *tx)
                .await?;
        match disabled_at {
            None => return Err(StoreError::UserNotFound),
            Some(Some(_)) => return Err(StoreError::UserDisabled),
            Some(None) => {}
        }

        sqlx::query(DELETE_RESETS)
            .bind(&reset.user_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO password_resets (token_hash, user_id, created_by, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
        )
        .bind(&reset.token_hash)
        .bind(&reset.user_id)
        .bind(&reset.created_by)
        .bind(reset.expires_at)
        .bind(reset.now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn password_reset_usable(&self, token_hash: &str, now: i64) -> Result<bool, StoreError> {
        let user_id: Option<String> = sqlx::query_scalar(USABLE_RESET)
            .bind(token_hash)
            .bind(now)
            .fetch_optional(&self.reader)
            .await?;
        Ok(user_id.is_some())
    }

    async fn use_password_reset(
        &self,
        token_hash: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<bool, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let user_id: Option<String> = sqlx::query_scalar(USABLE_RESET)
            .bind(token_hash)
            .bind(now)
            .fetch_optional(&mut *tx)
            .await?;
        let Some(user_id) = user_id else {
            return Ok(false);
        };

        sqlx::query("UPDATE users SET password_hash = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(&user_id)
            .bind(password_hash)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        revoke_access(&mut tx, &user_id).await?;
        // A reset proves who holds the account: a provider tied by someone else goes with it.
        sqlx::query("DELETE FROM user_identities WHERE user_id = ?1")
            .bind(&user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn create_email_code(&self, code: &NewEmailCode) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        sqlx::query("DELETE FROM email_codes WHERE expires_at <= ?1")
            .bind(code.now)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO email_codes (purpose, email, code_hash, expires_at, created_at, user_id)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT (purpose, email) DO UPDATE SET
                 code_hash = excluded.code_hash, tries = 0, user_id = excluded.user_id,
                 expires_at = excluded.expires_at, created_at = excluded.created_at",
        )
        .bind(code.purpose.as_str())
        .bind(&code.email)
        .bind(&code.code_hash)
        .bind(code.expires_at)
        .bind(code.now)
        .bind(&code.user_id)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn register_user(
        &self,
        user: &NewUser,
        attempt: &CodeAttempt<'_>,
        session: &NewSession,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        // First-run setup makes the first account, never a registration.
        let set_up: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
            .fetch_one(&mut *tx)
            .await?;
        if !set_up {
            return Err(StoreError::WrongCode);
        }
        if !use_code(&mut tx, attempt).await? {
            // The try it counted stays.
            tx.commit().await?;
            return Err(StoreError::WrongCode);
        }
        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, email_verified_at,
                                created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5, ?5)",
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(&user.password_hash)
        .bind(session.now)
        .execute(&mut *tx)
        .await
        .map_err(|error| match error.as_database_error() {
            // Rolled back with the rest, so the code stays.
            Some(database) if database.is_unique_violation() => StoreError::EmailTaken,
            _ => error.into(),
        })?;
        sqlx::query(INSERT_SESSION)
            .bind(&session.token_hash)
            .bind(&session.user_id)
            .bind(session.now)
            .bind(session.expires_at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn create_session_with_code(
        &self,
        session: &NewSession,
        checked_hash: &str,
        attempt: &CodeAttempt<'_>,
    ) -> Result<bool, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        // What `create_session` checks, the app included: it replaces the code by mail.
        let still: bool = sqlx::query_scalar(AssertSqlSafe(format!(
            "SELECT EXISTS (SELECT 1 FROM users WHERE id = ?1 AND password_hash = ?2
                                                  AND disabled_at IS NULL AND {NO_APP})"
        )))
        .bind(&session.user_id)
        .bind(checked_hash)
        .fetch_one(&mut *tx)
        .await?;
        if !still {
            return Ok(false);
        }
        if !use_code(&mut tx, attempt).await? {
            // The try it counted stays.
            tx.commit().await?;
            return Err(StoreError::WrongCode);
        }
        sqlx::query(VERIFY_ADDRESS)
            .bind(&session.user_id)
            .bind(attempt.email)
            .bind(session.now)
            .execute(&mut *tx)
            .await?;
        sqlx::query(INSERT_SESSION)
            .bind(&session.token_hash)
            .bind(&session.user_id)
            .bind(session.now)
            .bind(session.expires_at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }

    async fn enable_email_2fa(
        &self,
        user_id: &str,
        attempt: &CodeAttempt<'_>,
        session: &str,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let right = use_code(&mut tx, attempt).await?;
        let changed = if right {
            sqlx::query(
                "UPDATE users SET email_2fa = 1, updated_at = ?3,
                     email_verified_at = coalesce(email_verified_at, ?3)
                 WHERE id = ?1 AND email = ?2",
            )
            .bind(user_id)
            .bind(attempt.email)
            .bind(attempt.now)
            .execute(&mut *tx)
            .await?
            .rows_affected()
        } else {
            0
        };
        if changed == 1 {
            sqlx::query(DELETE_OTHER_SESSIONS)
                .bind(user_id)
                .bind(session)
                .execute(&mut *tx)
                .await?;
        }
        // A wrong try stays counted, and a right code for an old address stays used.
        tx.commit().await?;
        if changed == 1 {
            Ok(())
        } else {
            Err(StoreError::WrongCode)
        }
    }

    async fn change_email(
        &self,
        user_id: &str,
        attempt: &CodeAttempt<'_>,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        // A code another account asked for is none of this one's to guess.
        let owner: Option<Option<String>> = sqlx::query_scalar(
            "SELECT user_id FROM email_codes
             WHERE purpose = ?1 AND email = ?2 AND expires_at > ?3",
        )
        .bind(attempt.purpose.as_str())
        .bind(attempt.email)
        .bind(attempt.now)
        .fetch_optional(&mut *tx)
        .await?;
        if owner.flatten().as_deref() != Some(user_id) {
            return Err(StoreError::WrongCode);
        }
        if !use_code(&mut tx, attempt).await? {
            // The try it counted stays.
            tx.commit().await?;
            return Err(StoreError::WrongCode);
        }
        sqlx::query(
            "UPDATE users SET email = ?2, email_verified_at = ?3, updated_at = ?3
             WHERE id = ?1",
        )
        .bind(user_id)
        .bind(attempt.email)
        .bind(attempt.now)
        .execute(&mut *tx)
        .await
        .map_err(|error| match error.as_database_error() {
            // Rolled back with the rest, so the code stays.
            Some(database) if database.is_unique_violation() => StoreError::EmailTaken,
            _ => error.into(),
        })?;
        // A link mailed to the old address must not reach the account any more.
        sqlx::query(DELETE_RESETS)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn set_browser_language(&self, user_id: &str, language: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE users SET browser_language = ?2 WHERE id = ?1")
            .bind(user_id)
            .bind(language)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    async fn mail_language(&self, user_id: &str) -> Result<Option<String>, StoreError> {
        let query = format!("SELECT {MAIL_LANGUAGE} FROM users u WHERE u.id = ?1");
        let language: Option<Option<String>> = sqlx::query_scalar(AssertSqlSafe(query))
            .bind(user_id)
            .fetch_optional(&self.reader)
            .await?;
        Ok(language.flatten())
    }

    async fn set_email_notifications(&self, user_id: &str, on: bool) -> Result<(), StoreError> {
        sqlx::query("UPDATE users SET email_notifications = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(user_id)
            .bind(on)
            .bind(now_ms())
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    async fn email_2fa_users(&self) -> Result<u64, StoreError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users WHERE email_2fa = 1 AND disabled_at IS NULL",
        )
        .fetch_one(&self.reader)
        .await?;
        Ok(count as u64)
    }

    async fn start_totp(
        &self,
        user_id: &str,
        secret: &[u8],
        basis: &UpdateBasis<'_>,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        check_basis(&mut tx, user_id, basis).await?;
        let stored = sqlx::query(
            "INSERT INTO user_totp (user_id, secret, created_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (user_id) DO UPDATE SET
                 secret = excluded.secret, created_at = excluded.created_at, last_step = 0
             WHERE enabled_at IS NULL",
        )
        .bind(user_id)
        .bind(secret)
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if stored == 0 {
            return Err(StoreError::TotpOn);
        }
        tx.commit().await?;
        Ok(())
    }

    async fn pending_totp(&self, user_id: &str) -> Result<Option<Vec<u8>>, StoreError> {
        let secret = sqlx::query_scalar(
            "SELECT secret FROM user_totp WHERE user_id = ?1 AND enabled_at IS NULL",
        )
        .bind(user_id)
        .fetch_optional(&self.reader)
        .await?;
        Ok(secret)
    }

    async fn enable_totp(
        &self,
        user_id: &str,
        secret: &[u8],
        step: i64,
        recovery_hashes: &[String],
        session: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let enabled = sqlx::query(
            "UPDATE user_totp SET enabled_at = ?3, last_step = ?4
             WHERE user_id = ?1 AND secret = ?2 AND enabled_at IS NULL",
        )
        .bind(user_id)
        .bind(secret)
        .bind(now)
        .bind(step)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if enabled == 0 {
            return Err(StoreError::WrongCode);
        }
        insert_recovery_codes(&mut tx, user_id, recovery_hashes).await?;
        sqlx::query(DELETE_OTHER_SESSIONS)
            .bind(user_id)
            .bind(session)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn create_session_with_totp(
        &self,
        session: &NewSession,
        checked_hash: &str,
        proof: &TotpProof<'_>,
    ) -> Result<bool, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let created = sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
             SELECT ?1, id, ?3, ?3, ?4 FROM users
             WHERE id = ?2 AND password_hash = ?5 AND disabled_at IS NULL",
        )
        .bind(&session.token_hash)
        .bind(&session.user_id)
        .bind(session.now)
        .bind(session.expires_at)
        .bind(checked_hash)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if created == 0 {
            return Ok(false);
        }
        let used = match proof {
            // A later step only, so a code seen over a shoulder or in a log is spent.
            TotpProof::App { secret, step } => sqlx::query(
                "UPDATE user_totp SET last_step = ?3
                 WHERE user_id = ?1 AND secret = ?2 AND enabled_at IS NOT NULL
                   AND last_step < ?3",
            )
            .bind(&session.user_id)
            .bind(*secret)
            .bind(*step),
            TotpProof::Recovery { code_hash } => sqlx::query(
                "DELETE FROM user_recovery_codes WHERE user_id = ?1 AND code_hash = ?2
                   AND EXISTS (SELECT 1 FROM user_totp WHERE user_id = ?1
                                                         AND enabled_at IS NOT NULL)",
            )
            .bind(&session.user_id)
            .bind(*code_hash),
        }
        .execute(&mut *tx)
        .await?
        .rows_affected();
        if used == 0 {
            return Err(StoreError::WrongCode);
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn replace_recovery_codes(
        &self,
        user_id: &str,
        recovery_hashes: &[String],
        basis: &UpdateBasis<'_>,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        check_basis(&mut tx, user_id, basis).await?;
        let on: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_totp WHERE user_id = ?1
                                                     AND enabled_at IS NOT NULL)",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if !on {
            return Err(StoreError::TotpOff);
        }
        insert_recovery_codes(&mut tx, user_id, recovery_hashes).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn turn_totp_off(&self, user_id: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        turn_totp_off(&mut tx, user_id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn identity_user(
        &self,
        provider: &str,
        subject: &str,
    ) -> Result<Option<Credentials>, StoreError> {
        let row = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_CREDENTIALS} JOIN user_identities i ON i.user_id = u.id
             WHERE i.provider = ?1 AND i.subject = ?2"
        )))
        .bind(provider)
        .bind(subject)
        .fetch_optional(&self.reader)
        .await?;
        Ok(row.as_ref().map(credentials))
    }

    async fn create_identity_session(&self, session: &NewSession) -> Result<bool, StoreError> {
        let created = sqlx::query(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
             SELECT ?1, id, ?3, ?3, ?4 FROM users WHERE id = ?2 AND disabled_at IS NULL",
        )
        .bind(&session.token_hash)
        .bind(&session.user_id)
        .bind(session.now)
        .bind(session.expires_at)
        .execute(&self.writer)
        .await?
        .rows_affected();
        Ok(created == 1)
    }

    async fn register_identity(
        &self,
        user: &IdentityUser,
        session: &NewSession,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        // First-run setup makes the first account, with the code only the operator sees.
        let set_up: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM users)")
            .fetch_one(&mut *tx)
            .await?;
        if !set_up {
            return Err(StoreError::NotAllowed);
        }
        sqlx::query(
            "INSERT INTO users (id, email, name, email_verified_at, created_at, updated_at)
             VALUES (?1, ?2, ?3, CASE WHEN ?4 THEN ?5 END, ?5, ?5)",
        )
        .bind(&user.id)
        .bind(&user.email)
        .bind(&user.name)
        .bind(user.email_verified)
        .bind(session.now)
        .execute(&mut *tx)
        .await
        .map_err(|error| match error.as_database_error() {
            Some(database) if database.is_unique_violation() => StoreError::EmailTaken,
            _ => error.into(),
        })?;
        sqlx::query(
            "INSERT INTO user_identities (provider, subject, user_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&user.provider)
        .bind(&user.subject)
        .bind(&user.id)
        .bind(session.now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(INSERT_SESSION)
            .bind(&session.token_hash)
            .bind(&session.user_id)
            .bind(session.now)
            .bind(session.expires_at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
    async fn identities(&self, user_id: &str) -> Result<(Vec<String>, bool), StoreError> {
        let providers = sqlx::query_scalar(
            "SELECT provider FROM user_identities WHERE user_id = ?1 ORDER BY provider",
        )
        .bind(user_id)
        .fetch_all(&self.reader)
        .await?;
        let password: Option<bool> =
            sqlx::query_scalar("SELECT password_hash IS NOT NULL FROM users WHERE id = ?1")
                .bind(user_id)
                .fetch_optional(&self.reader)
                .await?;
        Ok((providers, password.ok_or(StoreError::UserNotFound)?))
    }

    async fn link_identity(
        &self,
        user_id: &str,
        provider: &str,
        subject: &str,
        now: i64,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let disabled: Option<bool> =
            sqlx::query_scalar("SELECT disabled_at IS NOT NULL FROM users WHERE id = ?1")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?;
        match disabled {
            None => return Err(StoreError::UserNotFound),
            Some(true) => return Err(StoreError::UserDisabled),
            Some(false) => {}
        }
        let held: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_identities WHERE provider = ?1
                            AND (subject = ?2 OR user_id = ?3))",
        )
        .bind(provider)
        .bind(subject)
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if held {
            return Err(StoreError::IdentityTaken);
        }
        sqlx::query(
            "INSERT INTO user_identities (provider, subject, user_id, created_at)
             VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(provider)
        .bind(subject)
        .bind(user_id)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn unlink_identity(&self, user_id: &str, provider: &str) -> Result<bool, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let tied: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_identities WHERE user_id = ?1 AND provider = ?2)",
        )
        .bind(user_id)
        .bind(provider)
        .fetch_one(&mut *tx)
        .await?;
        if !tied {
            return Ok(false);
        }
        let other: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM user_identities WHERE user_id = ?1 AND provider <> ?2)
                 OR (SELECT password_hash IS NOT NULL FROM users WHERE id = ?1)",
        )
        .bind(user_id)
        .bind(provider)
        .fetch_one(&mut *tx)
        .await?;
        if !other {
            return Err(StoreError::LastLogin);
        }
        sqlx::query("DELETE FROM user_identities WHERE user_id = ?1 AND provider = ?2")
            .bind(user_id)
            .bind(provider)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(true)
    }
}
