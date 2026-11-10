use cadocs_core::permissions::{
    InstanceAction, WorkspaceAction, WorkspaceRole, instance_allows, instance_role,
    member_change_action, workspace_allows, would_remove_last_owner,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::accounts::{INSERT_SESSION, membership, select_memberships};
use super::{BEGIN_WRITE, SqliteStore, search};
use crate::{
    Invitation, InvitationPreview, InvitedUser, MAX_SLUG_CHARS, Member, Membership, NewInvitation,
    NewSession, NewWorkspace, StoreError, Workspace, WorkspaceChanges, WorkspaceStore,
};

const SELECT_WORKSPACE: &str =
    "SELECT w.id, w.slug, w.name, m.role, w.language, w.created_at, w.updated_at
     FROM workspace_members m JOIN workspaces w ON w.id = m.workspace_id
     WHERE w.id = ?1 AND m.user_id = ?2";

const SELECT_MEMBER: &str = "SELECT u.id, u.email, u.name, m.role, u.disabled_at, m.created_at
     FROM workspace_members m JOIN users u ON u.id = m.user_id
     WHERE m.workspace_id = ?1";

const SELECT_USABLE: &str = "SELECT id, workspace_id, email, role FROM workspace_invitations
     WHERE token_hash = ?1 AND expires_at > ?2";

const INSERT_MEMBER: &str =
    "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
     VALUES (?1, ?2, ?3, ?4)";

/// A member keeps the role they have.
const INSERT_MEMBER_IF_ABSENT: &str =
    "INSERT OR IGNORE INTO workspace_members (workspace_id, user_id, role, created_at)
     VALUES (?1, ?2, ?3, ?4)";

/// Only ever reads what the `CHECK` of the column lets in.
fn role(row: &SqliteRow) -> WorkspaceRole {
    let role: String = row.get("role");
    WorkspaceRole::parse(&role).unwrap_or(WorkspaceRole::Member)
}

fn workspace(row: &SqliteRow) -> Workspace {
    Workspace {
        membership: membership(row),
        language: row.get("language"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

fn member(row: &SqliteRow) -> Member {
    Member {
        id: row.get("id"),
        email: row.get("email"),
        name: row.get("name"),
        role: role(row),
        disabled: row.get::<Option<i64>, _>("disabled_at").is_some(),
        created_at: row.get("created_at"),
    }
}

async fn role_in(
    tx: &mut SqliteConnection,
    workspace_id: &str,
    user_id: &str,
) -> Result<Option<WorkspaceRole>, StoreError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT role FROM workspace_members WHERE workspace_id = ?1 AND user_id = ?2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_optional(tx)
    .await?;
    Ok(role.as_deref().and_then(WorkspaceRole::parse))
}

/// A stranger must not learn that the workspace exists.
fn check(role: Option<WorkspaceRole>, action: WorkspaceAction) -> Result<(), StoreError> {
    if role.is_none() {
        Err(StoreError::WorkspaceNotFound)
    } else if workspace_allows(role, action) {
        Ok(())
    } else {
        Err(StoreError::NotAllowed)
    }
}

pub(super) async fn authorize(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    actor: &str,
    action: WorkspaceAction,
) -> Result<(), StoreError> {
    let role: Option<String> = sqlx::query_scalar(
        "SELECT m.role FROM workspace_members m JOIN users u ON u.id = m.user_id
         WHERE m.workspace_id = ?1 AND m.user_id = ?2 AND u.disabled_at IS NULL",
    )
    .bind(workspace_id)
    .bind(actor)
    .fetch_optional(conn)
    .await?;
    check(role.as_deref().and_then(WorkspaceRole::parse), action)
}

/// Whether `actor` may give `user_id` the role `next`, or remove them with `None`. Only enabled
/// owners count, as in `delete_user`.
async fn check_member_change(
    tx: &mut SqliteConnection,
    workspace_id: &str,
    actor: &str,
    user_id: &str,
    next: Option<WorkspaceRole>,
) -> Result<(), StoreError> {
    let actor_role = role_in(tx, workspace_id, actor).await?;
    if actor_role.is_none() {
        return Err(StoreError::WorkspaceNotFound);
    }
    let current = role_in(tx, workspace_id, user_id)
        .await?
        .ok_or(StoreError::MemberNotFound)?;

    let action = if next.is_none() && actor == user_id {
        WorkspaceAction::Leave { last_owner: false }
    } else {
        member_change_action(current, next)
    };
    check(actor_role, action)?;

    let owners_who_stay: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM workspace_members m JOIN users u ON u.id = m.user_id
         WHERE m.workspace_id = ?1 AND m.role = 'owner' AND u.disabled_at IS NULL AND u.id <> ?2",
    )
    .bind(workspace_id)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    if would_remove_last_owner(current, next, owners_who_stay as usize + 1) {
        return Err(StoreError::LastOwner);
    }
    Ok(())
}

/// Like a reset link, an invitation is a way back in, so it does not outlive the right to make it.
async fn end_links_of(
    tx: &mut SqliteConnection,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    sqlx::query("DELETE FROM workspace_invitations WHERE workspace_id = ?1 AND invited_by = ?2")
        .bind(workspace_id)
        .bind(user_id)
        .execute(tx)
        .await?;
    Ok(())
}

/// `base-<number>`, no longer than a slug may be.
pub(super) fn numbered_slug(base: &str, number: u32) -> String {
    let suffix = format!("-{number}");
    let kept: String = base.chars().take(MAX_SLUG_CHARS - suffix.len()).collect();
    format!("{}{suffix}", kept.trim_end_matches('-'))
}

/// The first of `base`, `base-2`, `base-3`... that no workspace uses.
async fn free_slug(tx: &mut SqliteConnection, base: &str) -> Result<String, StoreError> {
    let mut slug = base.to_owned();
    for number in 2.. {
        let taken: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM workspaces WHERE slug = ?1)")
                .bind(&slug)
                .fetch_one(&mut *tx)
                .await?;
        if !taken {
            break;
        }
        slug = numbered_slug(base, number);
    }
    Ok(slug)
}

/// A grant must not come back with the user: someone invited again starts from nothing.
async fn end_grants_of(
    tx: &mut SqliteConnection,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "DELETE FROM space_grants WHERE user_id = ?2
         AND space_id IN (SELECT id FROM spaces WHERE workspace_id = ?1)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "DELETE FROM group_members WHERE user_id = ?2
         AND group_id IN (SELECT id FROM groups WHERE workspace_id = ?1)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(tx)
    .await?;
    Ok(())
}

/// Adds the invited role and spends the link.
async fn join(
    tx: &mut SqliteConnection,
    invitation: &SqliteRow,
    user_id: &str,
    now: i64,
) -> Result<(), StoreError> {
    let workspace_id: &str = invitation.get("workspace_id");
    sqlx::query(INSERT_MEMBER_IF_ABSENT)
        .bind(workspace_id)
        .bind(user_id)
        .bind(invitation.get::<&str, _>("role"))
        .bind(now)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM workspace_invitations WHERE id = ?1")
        .bind(invitation.get::<&str, _>("id"))
        .execute(&mut *tx)
        .await?;
    Ok(())
}

impl WorkspaceStore for SqliteStore {
    async fn create_workspace(&self, new: &NewWorkspace) -> Result<Workspace, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let is_instance_admin: Option<bool> = sqlx::query_scalar(
            "SELECT is_instance_admin FROM users WHERE id = ?1 AND disabled_at IS NULL",
        )
        .bind(&new.owner_id)
        .fetch_optional(&mut *tx)
        .await?;
        let role = is_instance_admin.and_then(|admin| instance_role(true, admin));
        if !instance_allows(role, InstanceAction::CreateWorkspace) {
            return Err(StoreError::NotAllowed);
        }

        let slug = free_slug(&mut tx, &new.slug).await?;
        sqlx::query(
            "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?4)",
        )
        .bind(&new.id)
        .bind(&slug)
        .bind(&new.name)
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        sqlx::query(INSERT_MEMBER)
            .bind(&new.id)
            .bind(&new.owner_id)
            .bind(WorkspaceRole::Owner.as_str())
            .bind(new.now)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(SELECT_WORKSPACE)
            .bind(&new.id)
            .bind(&new.owner_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(workspace(&row))
    }

    async fn list_workspaces(
        &self,
        user_id: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Membership>, StoreError> {
        select_memberships(&self.reader, user_id, after, i64::from(limit)).await
    }

    async fn workspace(
        &self,
        workspace_id: &str,
        user_id: &str,
    ) -> Result<Option<Workspace>, StoreError> {
        let row = sqlx::query(SELECT_WORKSPACE)
            .bind(workspace_id)
            .bind(user_id)
            .fetch_optional(&self.reader)
            .await?;
        Ok(row.as_ref().map(workspace))
    }

    async fn update_workspace(
        &self,
        workspace_id: &str,
        actor: &str,
        changes: &WorkspaceChanges,
        now: i64,
    ) -> Result<Workspace, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize(&mut tx, workspace_id, actor, WorkspaceAction::Update).await?;
        let language: String = sqlx::query_scalar("SELECT language FROM workspaces WHERE id = ?1")
            .bind(workspace_id)
            .fetch_one(&mut *tx)
            .await?;
        // Stems change with the language, so every page must be stemmed again.
        let mut queued = false;
        if changes
            .language
            .as_ref()
            .is_some_and(|new| *new != language)
        {
            let filter = "p.workspace_id = ?1";
            queued = search::queue_reindex(&mut tx, filter, workspace_id, now).await?;
        }
        sqlx::query(
            "UPDATE workspaces SET
                 name = coalesce(?2, name),
                 language = coalesce(?3, language),
                 updated_at = ?4
             WHERE id = ?1",
        )
        .bind(workspace_id)
        .bind(&changes.name)
        .bind(&changes.language)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let row = sqlx::query(SELECT_WORKSPACE)
            .bind(workspace_id)
            .bind(actor)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        if queued {
            self.jobs_added.notify_one();
        }
        Ok(workspace(&row))
    }

    async fn delete_workspace(
        &self,
        workspace_id: &str,
        actor: &str,
        confirmation: &str,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize(&mut tx, workspace_id, actor, WorkspaceAction::Delete).await?;
        let deleted = sqlx::query("DELETE FROM workspaces WHERE id = ?1 AND name = ?2")
            .bind(workspace_id)
            .bind(confirmation)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if deleted == 0 {
            return Err(StoreError::NameMismatch);
        }
        tx.commit().await?;
        Ok(())
    }

    async fn list_members(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Member>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        authorize(&mut reader, workspace_id, actor, WorkspaceAction::Read).await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_MEMBER} AND u.id > coalesce(?2, '') ORDER BY u.id LIMIT ?3"
        )))
        .bind(workspace_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().map(member).collect())
    }

    async fn set_member_role(
        &self,
        workspace_id: &str,
        actor: &str,
        user_id: &str,
        role: WorkspaceRole,
    ) -> Result<Member, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        check_member_change(&mut tx, workspace_id, actor, user_id, Some(role)).await?;
        sqlx::query(
            "UPDATE workspace_members SET role = ?3 WHERE workspace_id = ?1 AND user_id = ?2",
        )
        .bind(workspace_id)
        .bind(user_id)
        .bind(role.as_str())
        .execute(&mut *tx)
        .await?;
        if role < WorkspaceRole::Admin {
            end_links_of(&mut tx, workspace_id, user_id).await?;
        }
        let row = sqlx::query(AssertSqlSafe(format!("{SELECT_MEMBER} AND u.id = ?2")))
            .bind(workspace_id)
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(member(&row))
    }

    async fn remove_member(
        &self,
        workspace_id: &str,
        actor: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        check_member_change(&mut tx, workspace_id, actor, user_id, None).await?;
        sqlx::query("DELETE FROM workspace_members WHERE workspace_id = ?1 AND user_id = ?2")
            .bind(workspace_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        end_links_of(&mut tx, workspace_id, user_id).await?;
        end_grants_of(&mut tx, workspace_id, user_id).await?;
        tx.commit().await?;
        Ok(())
    }

    async fn create_invitation(&self, new: &NewInvitation) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let action = WorkspaceAction::ManageInvitations;
        authorize(&mut tx, &new.workspace_id, &new.invited_by, action).await?;

        let member: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM workspace_members m JOIN users u ON u.id = m.user_id
                            WHERE m.workspace_id = ?1 AND u.email = ?2)",
        )
        .bind(&new.workspace_id)
        .bind(&new.email)
        .fetch_one(&mut *tx)
        .await?;
        if member {
            return Err(StoreError::AlreadyMember);
        }

        sqlx::query("DELETE FROM workspace_invitations WHERE workspace_id = ?1 AND email = ?2")
            .bind(&new.workspace_id)
            .bind(&new.email)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO workspace_invitations
                 (id, workspace_id, email, role, token_hash, invited_by, expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&new.id)
        .bind(&new.workspace_id)
        .bind(&new.email)
        .bind(new.role.as_str())
        .bind(&new.token_hash)
        .bind(&new.invited_by)
        .bind(new.expires_at)
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn list_invitations(
        &self,
        workspace_id: &str,
        actor: &str,
        now: i64,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Invitation>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let action = WorkspaceAction::ManageInvitations;
        authorize(&mut reader, workspace_id, actor, action).await?;
        let rows = sqlx::query(
            "SELECT id, email, role, invited_by, expires_at, created_at
             FROM workspace_invitations
             WHERE workspace_id = ?1 AND expires_at > ?2 AND id > coalesce(?3, '')
             ORDER BY id LIMIT ?4",
        )
        .bind(workspace_id)
        .bind(now)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows
            .iter()
            .map(|row| Invitation {
                id: row.get("id"),
                email: row.get("email"),
                role: role(row),
                invited_by: row.get("invited_by"),
                expires_at: row.get("expires_at"),
                created_at: row.get("created_at"),
            })
            .collect())
    }

    async fn revoke_invitation(&self, invitation_id: &str, actor: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let workspace_id: Option<String> =
            sqlx::query_scalar("SELECT workspace_id FROM workspace_invitations WHERE id = ?1")
                .bind(invitation_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(workspace_id) = workspace_id else {
            return Err(StoreError::InvitationNotFound);
        };
        let role = role_in(&mut tx, &workspace_id, actor).await?;
        if !workspace_allows(role, WorkspaceAction::ManageInvitations) {
            return Err(StoreError::InvitationNotFound);
        }
        sqlx::query("DELETE FROM workspace_invitations WHERE id = ?1")
            .bind(invitation_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn invitation(
        &self,
        token_hash: &str,
        now: i64,
    ) -> Result<Option<InvitationPreview>, StoreError> {
        let row = sqlx::query(
            "SELECT w.id, w.name, i.email, i.role, i.expires_at,
                    EXISTS (SELECT 1 FROM users u WHERE u.email = i.email) AS has_account
             FROM workspace_invitations i JOIN workspaces w ON w.id = i.workspace_id
             WHERE i.token_hash = ?1 AND i.expires_at > ?2",
        )
        .bind(token_hash)
        .bind(now)
        .fetch_optional(&self.reader)
        .await?;
        Ok(row.map(|row| InvitationPreview {
            workspace_id: row.get("id"),
            workspace_name: row.get("name"),
            email: row.get("email"),
            role: role(&row),
            expires_at: row.get("expires_at"),
            has_account: row.get("has_account"),
        }))
    }

    async fn accept_as_new_user(
        &self,
        token_hash: &str,
        user: &InvitedUser,
        session: &NewSession,
    ) -> Result<Vec<Membership>, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let invitation = sqlx::query(SELECT_USABLE)
            .bind(token_hash)
            .bind(session.now)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::InvitationNotFound)?;

        sqlx::query(
            "INSERT INTO users (id, email, name, password_hash, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?5)",
        )
        .bind(&user.id)
        .bind(invitation.get::<&str, _>("email"))
        .bind(&user.name)
        .bind(&user.password_hash)
        .bind(session.now)
        .execute(&mut *tx)
        .await
        .map_err(|error| match error.as_database_error() {
            Some(database) if database.is_unique_violation() => StoreError::EmailTaken,
            _ => error.into(),
        })?;
        join(&mut tx, &invitation, &user.id, session.now).await?;
        sqlx::query(INSERT_SESSION)
            .bind(&session.token_hash)
            .bind(&session.user_id)
            .bind(session.now)
            .bind(session.expires_at)
            .execute(&mut *tx)
            .await?;

        let memberships = select_memberships(&mut *tx, &user.id, None, -1).await?;
        tx.commit().await?;
        Ok(memberships)
    }

    async fn accept_as_user(
        &self,
        token_hash: &str,
        user_id: &str,
        now: i64,
    ) -> Result<Vec<Membership>, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let invitation = sqlx::query(SELECT_USABLE)
            .bind(token_hash)
            .bind(now)
            .fetch_optional(&mut *tx)
            .await?
            .ok_or(StoreError::InvitationNotFound)?;

        let invited: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM users
                            WHERE id = ?1 AND email = ?2 AND disabled_at IS NULL)",
        )
        .bind(user_id)
        .bind(invitation.get::<&str, _>("email"))
        .fetch_one(&mut *tx)
        .await?;
        if !invited {
            return Err(StoreError::NotAllowed);
        }

        join(&mut tx, &invitation, user_id, now).await?;
        let memberships = select_memberships(&mut *tx, user_id, None, -1).await?;
        tx.commit().await?;
        Ok(memberships)
    }
}
