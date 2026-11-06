use cadocs_core::permissions::{
    WorkspaceRole, would_remove_last_instance_admin, would_remove_last_owner,
};
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

use super::{BEGIN_WRITE, SqliteStore};
use crate::{
    AccountStore, ActiveSession, Credentials, FirstUser, Membership, NewSession, StoreError,
    UpdateBasis, User, UserChanges,
};

const INSERT_SESSION: &str =
    "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
     VALUES (?1, ?2, ?3, ?3, ?4)";

fn user(row: &SqliteRow) -> User {
    User {
        id: row.get("id"),
        email: row.get("email"),
        name: row.get("name"),
        is_instance_admin: row.get::<i64, _>("is_instance_admin") != 0,
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
            "SELECT id, email, name, is_instance_admin, password_hash, disabled_at
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
            "SELECT u.id, u.email, u.name, u.is_instance_admin, s.last_seen_at
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
        let rows = sqlx::query(
            "SELECT w.id, w.slug, w.name, m.role
             FROM workspace_members m JOIN workspaces w ON w.id = m.workspace_id
             WHERE m.user_id = ?1 ORDER BY w.id",
        )
        .bind(user_id)
        .fetch_all(&self.reader)
        .await?;
        Ok(rows
            .into_iter()
            .map(|row| Membership {
                workspace_id: row.get("id"),
                slug: row.get("slug"),
                name: row.get("name"),
                role: row.get("role"),
            })
            .collect())
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
        let stored: Option<String> =
            sqlx::query_scalar("SELECT password_hash FROM users WHERE id = ?1")
                .bind(user_id)
                .fetch_optional(&mut *tx)
                .await?
                .flatten();
        let password_moved = basis
            .checked_hash
            .is_some_and(|checked| stored.as_deref() != Some(checked));
        if !session_alive || password_moved {
            return Err(StoreError::StaleCredentials);
        }

        sqlx::query(
            "UPDATE users SET
                 name = COALESCE(?2, name),
                 email = COALESCE(?3, email),
                 password_hash = COALESCE(?4, password_hash),
                 updated_at = ?5
             WHERE id = ?1",
        )
        .bind(user_id)
        .bind(&changes.name)
        .bind(&changes.email)
        .bind(&changes.password_hash)
        .bind(now)
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
        tx.commit().await?;
        Ok(())
    }

    async fn delete_user(&self, user_id: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;

        let owners_of_each_workspace: Vec<i64> = sqlx::query_scalar(
            "SELECT (SELECT count(*) FROM workspace_members o JOIN users u ON u.id = o.user_id
                     WHERE o.workspace_id = m.workspace_id AND o.role = 'owner'
                       AND u.disabled_at IS NULL)
             FROM workspace_members m WHERE m.user_id = ?1 AND m.role = 'owner'",
        )
        .bind(user_id)
        .fetch_all(&mut *tx)
        .await?;
        let last_owner = owners_of_each_workspace
            .into_iter()
            .any(|owners| would_remove_last_owner(WorkspaceRole::Owner, None, owners as usize));
        if last_owner {
            return Err(StoreError::LastOwner);
        }

        let (is_enabled_admin, enabled_admins): (bool, i64) = sqlx::query_as(
            "SELECT coalesce(sum(id = ?1), 0) > 0, count(*) FROM users
             WHERE is_instance_admin = 1 AND disabled_at IS NULL",
        )
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if would_remove_last_instance_admin(is_enabled_admin, enabled_admins as usize) {
            return Err(StoreError::LastInstanceAdmin);
        }

        sqlx::query("DELETE FROM users WHERE id = ?1")
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
