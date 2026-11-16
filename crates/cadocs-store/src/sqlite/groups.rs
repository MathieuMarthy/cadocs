use cadocs_core::permissions::WorkspaceAction;
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::workspaces::{SELECT_MEMBER, authorize, member};
use super::{BEGIN_WRITE, SqliteStore};
use crate::{Group, GroupStore, Member, NewGroup, StoreError};

const SELECT_GROUP: &str = "SELECT g.id, g.workspace_id, g.name, g.created_at,
         (SELECT count(*) FROM group_members gm WHERE gm.group_id = g.id) AS member_count
     FROM groups g";

fn group(row: &SqliteRow) -> Group {
    Group {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        name: row.get("name"),
        member_count: row.get("member_count"),
        created_at: row.get("created_at"),
    }
}

fn name_taken(error: sqlx::Error) -> StoreError {
    match error.as_database_error() {
        Some(database) if database.is_unique_violation() => StoreError::GroupNameTaken,
        _ => error.into(),
    }
}

/// The workspace of the group, once `actor` may do `action` there. A stranger to the workspace
/// must not learn that the group exists.
async fn authorize_group(
    conn: &mut SqliteConnection,
    group_id: &str,
    actor: &str,
    action: WorkspaceAction,
) -> Result<String, StoreError> {
    let workspace_id: Option<String> =
        sqlx::query_scalar("SELECT workspace_id FROM groups WHERE id = ?1")
            .bind(group_id)
            .fetch_optional(&mut *conn)
            .await?;
    let workspace_id = workspace_id.ok_or(StoreError::GroupNotFound)?;
    match authorize(conn, &workspace_id, actor, action).await {
        Err(StoreError::WorkspaceNotFound) => Err(StoreError::GroupNotFound),
        Err(error) => Err(error),
        Ok(()) => Ok(workspace_id),
    }
}

async fn select_group(conn: &mut SqliteConnection, group_id: &str) -> Result<Group, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!("{SELECT_GROUP} WHERE g.id = ?1")))
        .bind(group_id)
        .fetch_one(conn)
        .await?;
    Ok(group(&row))
}

impl GroupStore for SqliteStore {
    async fn create_group(&self, new: &NewGroup) -> Result<Group, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let action = WorkspaceAction::ManageGroups;
        authorize(&mut tx, &new.workspace_id, &new.actor, action).await?;
        sqlx::query(
            "INSERT INTO groups (id, workspace_id, name, created_at) VALUES (?1, ?2, ?3, ?4)",
        )
        .bind(&new.id)
        .bind(&new.workspace_id)
        .bind(&new.name)
        .bind(new.now)
        .execute(&mut *tx)
        .await
        .map_err(name_taken)?;
        let created = select_group(&mut tx, &new.id).await?;
        tx.commit().await?;
        Ok(created)
    }

    async fn list_groups(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Group>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        authorize(&mut reader, workspace_id, actor, WorkspaceAction::Read).await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_GROUP} WHERE g.workspace_id = ?1 AND g.id > coalesce(?2, '')
             ORDER BY g.id LIMIT ?3"
        )))
        .bind(workspace_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().map(group).collect())
    }

    async fn rename_group(
        &self,
        group_id: &str,
        actor: &str,
        name: &str,
    ) -> Result<Group, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize_group(&mut tx, group_id, actor, WorkspaceAction::ManageGroups).await?;
        sqlx::query("UPDATE groups SET name = ?2 WHERE id = ?1")
            .bind(group_id)
            .bind(name)
            .execute(&mut *tx)
            .await
            .map_err(name_taken)?;
        let renamed = select_group(&mut tx, group_id).await?;
        tx.commit().await?;
        Ok(renamed)
    }

    async fn delete_group(&self, group_id: &str, actor: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize_group(&mut tx, group_id, actor, WorkspaceAction::ManageGroups).await?;
        // Memberships and grants go with it, by their foreign keys.
        sqlx::query("DELETE FROM groups WHERE id = ?1")
            .bind(group_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn list_group_members(
        &self,
        group_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Member>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let action = WorkspaceAction::Read;
        let workspace_id = authorize_group(&mut reader, group_id, actor, action).await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_MEMBER}
             AND u.id IN (SELECT gm.user_id FROM group_members gm WHERE gm.group_id = ?4)
             AND u.id > coalesce(?2, '') ORDER BY u.id LIMIT ?3"
        )))
        .bind(&workspace_id)
        .bind(after)
        .bind(limit)
        .bind(group_id)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().map(member).collect())
    }

    async fn add_group_member(
        &self,
        group_id: &str,
        actor: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let action = WorkspaceAction::ManageGroups;
        let workspace_id = authorize_group(&mut tx, group_id, actor, action).await?;
        // A grant only reaches members of the workspace (DATA_MODEL.md).
        let member: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM workspace_members
                            WHERE workspace_id = ?1 AND user_id = ?2)",
        )
        .bind(&workspace_id)
        .bind(user_id)
        .fetch_one(&mut *tx)
        .await?;
        if !member {
            return Err(StoreError::MemberNotFound);
        }
        sqlx::query("INSERT OR IGNORE INTO group_members (group_id, user_id) VALUES (?1, ?2)")
            .bind(group_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn remove_group_member(
        &self,
        group_id: &str,
        actor: &str,
        user_id: &str,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize_group(&mut tx, group_id, actor, WorkspaceAction::ManageGroups).await?;
        let removed = sqlx::query("DELETE FROM group_members WHERE group_id = ?1 AND user_id = ?2")
            .bind(group_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await?
            .rows_affected();
        if removed == 0 {
            return Err(StoreError::MemberNotFound);
        }
        tx.commit().await?;
        Ok(())
    }
}
