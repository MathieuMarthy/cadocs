use cadocs_core::permissions::{
    WorkspaceRole, would_remove_last_instance_admin, would_remove_last_owner,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection, SqliteExecutor};

use super::{BEGIN_WRITE, SqliteStore};
use crate::{
    AccountStore, ActiveSession, AdminChanges, CodeAttempt, Credentials, FirstUser, MAX_CODE_TRIES,
    Membership, NewEmailCode, NewPasswordReset, NewSession, NewUser, SessionStanding, StoreError,
    UpdateBasis, User, UserChanges, UserRecord,
};

pub(super) const INSERT_SESSION: &str =
    "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
     VALUES (?1, ?2, ?3, ?3, ?4)";

/// A code sent to the address of user `?1`, `?2`, was used at `?3`.
const VERIFY_ADDRESS: &str = "UPDATE users SET email_verified_at = coalesce(email_verified_at, ?3)
     WHERE id = ?1 AND email = ?2";

const DELETE_RESETS: &str = "DELETE FROM password_resets WHERE user_id = ?1";

/// The links an admin made outlive neither their role nor their account: each one can take over
/// another account, possibly another admin's.
const DELETE_RESETS_MADE_BY: &str = "DELETE FROM password_resets WHERE created_by = ?1";

/// A link that can be used now, and its user.
const USABLE_RESET: &str = "SELECT r.user_id FROM password_resets r
     JOIN users u ON u.id = r.user_id
     WHERE r.token_hash = ?1 AND r.expires_at > ?2 AND u.disabled_at IS NULL";

const SELECT_RECORD: &str =
    "SELECT id, email, name, is_instance_admin, email_2fa, language, disabled_at, created_at
     FROM users";

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

/// Ends everything that lets a user in: their sessions and their reset link.
async fn revoke_access(tx: &mut SqliteConnection, user_id: &str) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM sessions WHERE user_id = ?1")
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(DELETE_RESETS)
        .bind(user_id)
        .execute(&mut *tx)
        .await?;
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

fn user(row: &SqliteRow) -> User {
    User {
        id: row.get("id"),
        email: row.get("email"),
        name: row.get("name"),
        is_instance_admin: row.get::<i64, _>("is_instance_admin") != 0,
        email_2fa: row.get::<i64, _>("email_2fa") != 0,
        language: row.get("language"),
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
        let row = sqlx::query(
            "SELECT id, email, name, is_instance_admin, email_2fa, language, password_hash, disabled_at
             FROM users WHERE email = ?1",
        )
        .bind(email)
        .fetch_optional(&self.reader)
        .await?;
        Ok(row.map(|row| Credentials {
            user: user(&row),
            password_hash: row.get("password_hash"),
            disabled: row.get::<Option<i64>, _>("disabled_at").is_some(),
        }))
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
    ) -> Result<bool, StoreError> {
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
            "SELECT u.id, u.email, u.name, u.is_instance_admin, u.email_2fa, u.language,
                    s.last_seen_at
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
            sqlx::query("DELETE FROM sessions WHERE user_id = ?1 AND token_hash <> ?2")
                .bind(user_id)
                .bind(basis.session)
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
        // What `create_session` checks, and the address the code went to.
        let still: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users WHERE id = ?1 AND password_hash = ?2
                                                  AND disabled_at IS NULL)",
        )
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

    async fn email_2fa_users(&self) -> Result<u64, StoreError> {
        let count: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM users WHERE email_2fa = 1 AND disabled_at IS NULL",
        )
        .fetch_one(&self.reader)
        .await?;
        Ok(count as u64)
    }
}
