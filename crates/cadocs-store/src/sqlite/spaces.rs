use std::collections::{HashMap, HashSet};

use cadocs_core::permissions::{
    DefaultRole, Scope, SpaceAccess, SpaceAction, SpaceRole, WorkspaceAction, WorkspaceRole,
    space_role,
};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::workspaces::{authorize as authorize_workspace, numbered_slug};
use super::{BEGIN_WRITE, SqliteStore};
use crate::{Grant, GrantTarget, NewGrant, NewSpace, Space, SpaceChanges, SpaceStore, StoreError};

/// A space with all that resolution needs to know about the user `?1`.
const SELECT_SPACE: &str = "SELECT s.id, s.workspace_id, s.slug, s.name, s.description,
         s.default_role, s.deleted_at, s.created_at, s.updated_at,
         EXISTS (SELECT 1 FROM users u WHERE u.id = ?1 AND u.disabled_at IS NULL) AS active,
         (SELECT m.role FROM workspace_members m
          WHERE m.workspace_id = s.workspace_id AND m.user_id = ?1) AS workspace_role,
         (SELECT group_concat(g.role) FROM space_grants g
          WHERE g.space_id = s.id AND (g.user_id = ?1 OR g.group_id IN
              (SELECT gm.group_id FROM group_members gm WHERE gm.user_id = ?1))) AS grants
     FROM spaces s";

/// Exactly the spaces `space_role` gives a role in, as any grant does, so that `LIMIT` is right.
const HAS_ROLE: &str =
    "(workspace_role IN ('owner', 'admin') OR grants IS NOT NULL OR s.default_role IS NOT NULL)";

/// `default_role` never gives `admin`.
const IS_ADMIN: &str =
    "(workspace_role IN ('owner', 'admin') OR ',' || grants || ',' LIKE '%,admin,%')";

fn scope(row: &SqliteRow) -> Scope {
    let workspace_role: Option<String> = row.get("workspace_role");
    let default_role: Option<String> = row.get("default_role");
    let grants: Option<String> = row.get("grants");
    let grants: Vec<SpaceRole> = grants
        .iter()
        .flat_map(|grants| grants.split(','))
        .filter_map(SpaceRole::parse)
        .collect();
    let access = SpaceAccess {
        account_active: row.get("active"),
        workspace_role: workspace_role.as_deref().and_then(WorkspaceRole::parse),
        grants: &grants,
        default_role: default_role.as_deref().and_then(DefaultRole::parse),
    };
    Scope {
        role: space_role(&access),
        space_trashed: row.get::<Option<i64>, _>("deleted_at").is_some(),
        page_trashed: false,
    }
}

/// `None` for a user with no role.
fn space(row: &SqliteRow) -> Option<Space> {
    let default_role: Option<String> = row.get("default_role");
    Some(Space {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        slug: row.get("slug"),
        name: row.get("name"),
        description: row.get("description"),
        default_role: default_role.as_deref().and_then(DefaultRole::parse),
        role: scope(row).role?,
        deleted_at: row.get("deleted_at"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

/// `403` is for a user who can read the space: the others must not learn that it exists.
fn check(scope: Scope, action: SpaceAction) -> Result<(), StoreError> {
    if scope.allows(action) {
        Ok(())
    } else if scope.allows(SpaceAction::ReadSpace) {
        Err(StoreError::NotAllowed)
    } else {
        Err(StoreError::SpaceNotFound)
    }
}

async fn select_space(
    conn: &mut SqliteConnection,
    space_id: &str,
    actor: &str,
) -> Result<Option<SqliteRow>, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!("{SELECT_SPACE} WHERE s.id = ?2")))
        .bind(actor)
        .bind(space_id)
        .fetch_optional(conn)
        .await?;
    Ok(row)
}

/// The row of the space, once the user is known to be allowed `action`.
pub(super) async fn authorize(
    conn: &mut SqliteConnection,
    space_id: &str,
    actor: &str,
    action: SpaceAction,
) -> Result<SqliteRow, StoreError> {
    let row = select_space(conn, space_id, actor)
        .await?
        .ok_or(StoreError::SpaceNotFound)?;
    check(scope(&row), action)?;
    Ok(row)
}

/// A space that has just changed, for a user who may see it.
async fn reread(
    conn: &mut SqliteConnection,
    space_id: &str,
    actor: &str,
) -> Result<Space, StoreError> {
    select_space(conn, space_id, actor)
        .await?
        .as_ref()
        .and_then(space)
        .ok_or(StoreError::SpaceNotFound)
}

/// The first of `base`, `base-2`, `base-3`... that no space of the workspace uses, trashed ones
/// included.
async fn free_slug(
    tx: &mut SqliteConnection,
    workspace_id: &str,
    base: &str,
) -> Result<String, StoreError> {
    let mut slug = base.to_owned();
    for number in 2.. {
        let taken: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT 1 FROM spaces WHERE workspace_id = ?1 AND slug = ?2)",
        )
        .bind(workspace_id)
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

async fn in_workspace(
    tx: &mut SqliteConnection,
    workspace_id: &str,
    target: &GrantTarget,
) -> Result<bool, StoreError> {
    let (query, id) = match target {
        GrantTarget::User(id) => (
            "SELECT EXISTS (SELECT 1 FROM workspace_members
                            WHERE workspace_id = ?1 AND user_id = ?2)",
            id,
        ),
        GrantTarget::Group(id) => (
            "SELECT EXISTS (SELECT 1 FROM groups WHERE workspace_id = ?1 AND id = ?2)",
            id,
        ),
    };
    let found = sqlx::query_scalar(query)
        .bind(workspace_id)
        .bind(id)
        .fetch_one(tx)
        .await?;
    Ok(found)
}

impl SqliteStore {
    /// The spaces of a workspace that pass `filter`, for a member.
    async fn select_spaces(
        &self,
        workspace_id: &str,
        actor: &str,
        filter: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Space>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let action = WorkspaceAction::Read;
        authorize_workspace(&mut reader, workspace_id, actor, action).await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_SPACE} WHERE s.workspace_id = ?2 AND {filter}
             AND s.id > coalesce(?3, '') ORDER BY s.id LIMIT ?4"
        )))
        .bind(actor)
        .bind(workspace_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().filter_map(space).collect())
    }
}

impl SpaceStore for SqliteStore {
    async fn create_space(&self, new: &NewSpace) -> Result<Space, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let action = WorkspaceAction::CreateSpace;
        authorize_workspace(&mut tx, &new.workspace_id, &new.actor, action).await?;

        let slug = free_slug(&mut tx, &new.workspace_id, &new.slug).await?;
        sqlx::query(
            "INSERT INTO spaces
                 (id, workspace_id, slug, name, description, default_role, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7)",
        )
        .bind(&new.id)
        .bind(&new.workspace_id)
        .bind(&slug)
        .bind(&new.name)
        .bind(&new.description)
        .bind(new.default_role.map(DefaultRole::as_str))
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        let space = reread(&mut tx, &new.id, &new.actor).await?;
        tx.commit().await?;
        Ok(space)
    }

    async fn list_spaces(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Space>, StoreError> {
        let filter = format!("s.deleted_at IS NULL AND {HAS_ROLE}");
        self.select_spaces(workspace_id, actor, &filter, after, limit)
            .await
    }

    async fn list_trashed_spaces(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Space>, StoreError> {
        let filter = format!("s.deleted_at IS NOT NULL AND {IS_ADMIN}");
        self.select_spaces(workspace_id, actor, &filter, after, limit)
            .await
    }

    async fn space(&self, space_id: &str, actor: &str) -> Result<Option<Space>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let row = select_space(&mut reader, space_id, actor).await?;
        let readable = |row: &SqliteRow| scope(row).allows(SpaceAction::ReadSpace);
        Ok(row.as_ref().filter(|row| readable(row)).and_then(space))
    }

    async fn update_space(
        &self,
        space_id: &str,
        actor: &str,
        changes: &SpaceChanges,
        now: i64,
    ) -> Result<Space, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize(&mut tx, space_id, actor, SpaceAction::ManageSpace).await?;
        sqlx::query(
            "UPDATE spaces SET
                 name = coalesce(?2, name),
                 description = coalesce(?3, description),
                 default_role = CASE WHEN ?4 THEN ?5 ELSE default_role END,
                 updated_at = ?6
             WHERE id = ?1",
        )
        .bind(space_id)
        .bind(&changes.name)
        .bind(&changes.description)
        .bind(changes.default_role.is_some())
        .bind(changes.default_role.flatten().map(DefaultRole::as_str))
        .bind(now)
        .execute(&mut *tx)
        .await?;
        let space = reread(&mut tx, space_id, actor).await?;
        tx.commit().await?;
        Ok(space)
    }

    async fn trash_space(&self, space_id: &str, actor: &str, now: i64) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize(&mut tx, space_id, actor, SpaceAction::TrashSpace).await?;
        sqlx::query("UPDATE spaces SET deleted_at = ?2, updated_at = ?2 WHERE id = ?1")
            .bind(space_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn restore_space(
        &self,
        space_id: &str,
        actor: &str,
        now: i64,
    ) -> Result<Space, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize(&mut tx, space_id, actor, SpaceAction::RestoreSpace).await?;
        sqlx::query("UPDATE spaces SET deleted_at = NULL, updated_at = ?2 WHERE id = ?1")
            .bind(space_id)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let space = reread(&mut tx, space_id, actor).await?;
        tx.commit().await?;
        Ok(space)
    }

    async fn list_grants(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Grant>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        authorize(&mut reader, space_id, actor, SpaceAction::ReadSpace).await?;
        let rows = sqlx::query(
            "SELECT id, user_id, group_id, role, created_at FROM space_grants
             WHERE space_id = ?1 AND id > coalesce(?2, '') ORDER BY id LIMIT ?3",
        )
        .bind(space_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().filter_map(grant).collect())
    }

    async fn set_grants(
        &self,
        space_id: &str,
        actor: &str,
        grants: &[NewGrant],
        now: i64,
    ) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let row = authorize(&mut tx, space_id, actor, SpaceAction::ManageSpace).await?;
        let workspace_id: &str = row.get("workspace_id");

        let rows =
            sqlx::query("SELECT id, user_id, group_id FROM space_grants WHERE space_id = ?1")
                .bind(space_id)
                .fetch_all(&mut *tx)
                .await?;
        let mut leftover: HashMap<GrantTarget, String> = rows
            .iter()
            .filter_map(|row| Some((target(row)?, row.get("id"))))
            .collect();

        let mut seen = HashSet::new();
        for grant in grants {
            if !seen.insert(&grant.target) {
                return Err(StoreError::DuplicateGrant);
            }
            if !in_workspace(&mut tx, workspace_id, &grant.target).await? {
                return Err(StoreError::UnknownGrantTarget);
            }
            if let Some(id) = leftover.remove(&grant.target) {
                sqlx::query("UPDATE space_grants SET role = ?2 WHERE id = ?1")
                    .bind(id)
                    .bind(grant.role.as_str())
                    .execute(&mut *tx)
                    .await?;
                continue;
            }
            let (user_id, group_id) = match &grant.target {
                GrantTarget::User(id) => (Some(id), None),
                GrantTarget::Group(id) => (None, Some(id)),
            };
            sqlx::query(
                "INSERT INTO space_grants (id, space_id, user_id, group_id, role, created_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            )
            .bind(&grant.id)
            .bind(space_id)
            .bind(user_id)
            .bind(group_id)
            .bind(grant.role.as_str())
            .bind(now)
            .execute(&mut *tx)
            .await?;
        }
        for id in leftover.values() {
            sqlx::query("DELETE FROM space_grants WHERE id = ?1")
                .bind(id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// The `CHECK` of the table makes sure that one of the two is set.
fn target(row: &SqliteRow) -> Option<GrantTarget> {
    let user_id: Option<String> = row.get("user_id");
    let group_id: Option<String> = row.get("group_id");
    user_id
        .map(GrantTarget::User)
        .or(group_id.map(GrantTarget::Group))
}

/// Only ever reads what the `CHECK`s of the table let in.
fn grant(row: &SqliteRow) -> Option<Grant> {
    Some(Grant {
        id: row.get("id"),
        target: target(row)?,
        role: SpaceRole::parse(row.get("role"))?,
        created_at: row.get("created_at"),
    })
}
