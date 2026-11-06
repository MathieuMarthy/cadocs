use cadocs_core::permissions::{Scope, SpaceAction, WorkspaceAction, move_allowed};
use cadocs_core::position::between;
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::spaces::{self, HAS_ROLE, space_access};
use super::workspaces::authorize as authorize_workspace;
use super::{BEGIN_WRITE, SqliteStore};
use crate::{
    DateCursor, MAX_PAGE_DEPTH, MovedPage, NewPage, PageMove, PageRecord, PageSite, PageStore,
    Placement, StoreError,
};

/// A page with all that resolution needs to know about the user `?1`.
const SELECT_PAGE: &str = concat!(
    "SELECT p.id, p.workspace_id, p.space_id, p.parent_id, p.position, p.slug, p.title,
         p.created_by, p.updated_by, p.deleted_at, p.deleted_by, p.created_at, p.updated_at,
         EXISTS (SELECT 1 FROM pages c WHERE c.parent_id = p.id AND c.deleted_at IS NULL)
             AS has_children, ",
    space_access!(),
    " FROM pages p JOIN spaces s ON s.id = p.space_id"
);

/// The page `?1` and everything under it, the trashed pages too.
const SUBTREE: &str = "WITH RECURSIVE subtree(id, depth) AS (
         SELECT ?1, 1
         UNION ALL
         SELECT p.id, subtree.depth + 1 FROM pages p JOIN subtree ON p.parent_id = subtree.id
     )";

fn scope(row: &SqliteRow) -> Scope {
    Scope {
        page_trashed: row.get::<Option<i64>, _>("deleted_at").is_some(),
        ..spaces::scope(row)
    }
}

fn site(row: &SqliteRow) -> PageSite {
    PageSite {
        workspace_id: row.get("workspace_id"),
        space_id: row.get("space_id"),
    }
}

/// `None` for a user with no role.
fn page(row: &SqliteRow) -> Option<PageRecord> {
    Some(PageRecord {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        space_id: row.get("space_id"),
        parent_id: row.get("parent_id"),
        position: row.get("position"),
        slug: row.get("slug"),
        title: row.get("title"),
        role: scope(row).role?,
        has_children: row.get("has_children"),
        created_by: row.get("created_by"),
        updated_by: row.get("updated_by"),
        deleted_at: row.get("deleted_at"),
        deleted_by: row.get("deleted_by"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    })
}

/// `403` is for a user who can see the page: the others must not learn that it exists. A trashed
/// page only shows through the trash, so only to those who can list it, and only for what the
/// trash offers.
fn check(scope: Scope, action: SpaceAction) -> Result<(), StoreError> {
    let in_trash_view = matches!(action, SpaceAction::RestorePage | SpaceAction::PurgePage)
        && scope.allows(SpaceAction::ListTrash);
    let visible = if scope.page_trashed {
        in_trash_view
    } else {
        scope.allows(SpaceAction::ReadPage)
    };
    if scope.allows(action) {
        Ok(())
    } else if visible {
        Err(StoreError::NotAllowed)
    } else {
        Err(StoreError::PageNotFound)
    }
}

async fn select_page(
    conn: &mut SqliteConnection,
    page_id: &str,
    actor: &str,
) -> Result<Option<SqliteRow>, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!("{SELECT_PAGE} WHERE p.id = ?2")))
        .bind(actor)
        .bind(page_id)
        .fetch_optional(conn)
        .await?;
    Ok(row)
}

/// The row of the page, once the user is known to be allowed `action`.
async fn authorize(
    conn: &mut SqliteConnection,
    page_id: &str,
    actor: &str,
    action: SpaceAction,
) -> Result<SqliteRow, StoreError> {
    let row = select_page(conn, page_id, actor)
        .await?
        .ok_or(StoreError::PageNotFound)?;
    check(scope(&row), action)?;
    Ok(row)
}

/// A page that has just changed, for a user who may see it.
async fn reread(
    conn: &mut SqliteConnection,
    page_id: &str,
    actor: &str,
) -> Result<PageRecord, StoreError> {
    select_page(conn, page_id, actor)
        .await?
        .as_ref()
        .and_then(page)
        .ok_or(StoreError::PageNotFound)
}

fn readable(row: Option<SqliteRow>) -> Option<PageRecord> {
    row.as_ref()
        .filter(|row| scope(row).allows(SpaceAction::ReadPage))
        .and_then(page)
}

async fn is_live_in_space(
    conn: &mut SqliteConnection,
    page_id: &str,
    space_id: &str,
) -> Result<bool, StoreError> {
    let found = sqlx::query_scalar(
        "SELECT EXISTS (SELECT 1 FROM pages
                        WHERE id = ?1 AND space_id = ?2 AND deleted_at IS NULL)",
    )
    .bind(page_id)
    .bind(space_id)
    .fetch_one(conn)
    .await?;
    Ok(found)
}

/// The parent and the pages above it, once it is known to be a live page of the space. A page at
/// the root has none.
async fn ancestors(
    tx: &mut SqliteConnection,
    space_id: &str,
    parent_id: Option<&str>,
) -> Result<Vec<String>, StoreError> {
    let Some(parent_id) = parent_id else {
        return Ok(Vec::new());
    };
    if !is_live_in_space(tx, parent_id, space_id).await? {
        return Err(StoreError::UnknownParent);
    }
    let ids = sqlx::query_scalar(
        "WITH RECURSIVE above(id, parent_id) AS (
             SELECT id, parent_id FROM pages WHERE id = ?1
             UNION ALL
             SELECT p.id, p.parent_id FROM pages p JOIN above ON p.id = above.parent_id
         )
         SELECT id FROM above",
    )
    .bind(parent_id)
    .fetch_all(tx)
    .await?;
    Ok(ids)
}

/// A key for a page among the children of `parent_id`, `moving` being left out of them. Trashed
/// siblings count: they keep their key, so that a restore cannot land on one in use.
async fn position(
    tx: &mut SqliteConnection,
    space_id: &str,
    parent_id: Option<&str>,
    placement: &Placement,
    moving: Option<&str>,
) -> Result<String, StoreError> {
    let edge = |function: &str| {
        sqlx::query_scalar::<_, Option<String>>(AssertSqlSafe(format!(
            "SELECT {function}(position) FROM pages
             WHERE space_id = ?1 AND parent_id IS ?2 AND id IS NOT ?3"
        )))
        .bind(space_id)
        .bind(parent_id)
        .bind(moving)
    };
    match placement {
        Placement::First => {
            let first = edge("min").fetch_one(tx).await?;
            Ok(between(None, first.as_deref()))
        }
        Placement::Last => {
            let last = edge("max").fetch_one(tx).await?;
            Ok(between(last.as_deref(), None))
        }
        Placement::After(sibling) => {
            let key: Option<String> = sqlx::query_scalar(
                "SELECT position FROM pages
                 WHERE id = ?1 AND space_id = ?2 AND parent_id IS ?3 AND id IS NOT ?4
                     AND deleted_at IS NULL",
            )
            .bind(sibling)
            .bind(space_id)
            .bind(parent_id)
            .bind(moving)
            .fetch_optional(&mut *tx)
            .await?;
            let key = key.ok_or(StoreError::UnknownSibling)?;
            let next: Option<String> = sqlx::query_scalar(
                "SELECT min(position) FROM pages
                 WHERE space_id = ?1 AND parent_id IS ?2 AND id IS NOT ?3 AND position > ?4",
            )
            .bind(space_id)
            .bind(parent_id)
            .bind(moving)
            .bind(&key)
            .fetch_one(tx)
            .await?;
            Ok(between(Some(&key), next.as_deref()))
        }
    }
}

/// One screen of a list ordered by `date`, a column of the page, newest first. `filter` sees `?2`.
async fn pages_by_date(
    conn: &mut SqliteConnection,
    filter_id: &str,
    actor: &str,
    filter: &str,
    date: &str,
    after: Option<&DateCursor>,
    limit: u32,
) -> Result<Vec<PageRecord>, StoreError> {
    let rows = sqlx::query(AssertSqlSafe(format!(
        "{SELECT_PAGE} WHERE {filter} AND ({date}, p.id) < (?3, ?4)
         ORDER BY {date} DESC, p.id DESC LIMIT ?5"
    )))
    .bind(actor)
    .bind(filter_id)
    .bind(after.map_or(i64::MAX, |cursor| cursor.date))
    .bind(after.map_or("", |cursor| cursor.id.as_str()))
    .bind(limit)
    .fetch_all(conn)
    .await?;
    Ok(rows.iter().filter_map(page).collect())
}

impl PageStore for SqliteStore {
    async fn create_page(&self, new: &NewPage) -> Result<PageRecord, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let space = spaces::authorize(&mut tx, &new.space_id, &new.actor, SpaceAction::EditPage);
        let space = space.await?;
        let workspace_id: &str = space.get("workspace_id");

        let parent_id = new.parent_id.as_deref();
        let above = ancestors(&mut tx, &new.space_id, parent_id).await?;
        if above.len() >= MAX_PAGE_DEPTH {
            return Err(StoreError::TooDeep);
        }
        let key = position(&mut tx, &new.space_id, parent_id, &new.placement, None).await?;
        sqlx::query(
            "INSERT INTO pages (id, workspace_id, space_id, parent_id, position, slug,
                                created_by, updated_by, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?7, ?8, ?8)",
        )
        .bind(&new.id)
        .bind(workspace_id)
        .bind(&new.space_id)
        .bind(parent_id)
        .bind(&key)
        .bind(&new.slug)
        .bind(&new.actor)
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        let page = reread(&mut tx, &new.id, &new.actor).await?;
        tx.commit().await?;
        Ok(page)
    }

    async fn page(&self, page_id: &str, actor: &str) -> Result<Option<PageRecord>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        Ok(readable(select_page(&mut reader, page_id, actor).await?))
    }

    async fn page_scope(&self, page_id: &str, actor: &str) -> Result<Option<Scope>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let row = select_page(&mut reader, page_id, actor).await?;
        Ok(row.as_ref().map(scope))
    }

    async fn page_to_edit(&self, page_id: &str, actor: &str) -> Result<PageRecord, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let row = authorize(&mut reader, page_id, actor, SpaceAction::EditPage).await?;
        page(&row).ok_or(StoreError::PageNotFound)
    }

    async fn page_by_slug(
        &self,
        workspace_id: &str,
        slug: &str,
        actor: &str,
    ) -> Result<Option<PageRecord>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let row = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_PAGE} WHERE p.workspace_id = ?2 AND p.slug = ?3"
        )))
        .bind(actor)
        .bind(workspace_id)
        .bind(slug)
        .fetch_optional(&mut *reader)
        .await?;
        Ok(readable(row))
    }

    async fn tree(
        &self,
        space_id: &str,
        parent_id: Option<&str>,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<PageRecord>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        spaces::authorize(&mut reader, space_id, actor, SpaceAction::ReadSpace).await?;
        if let Some(parent_id) = parent_id
            && !is_live_in_space(&mut reader, parent_id, space_id).await?
        {
            return Err(StoreError::PageNotFound);
        }
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_PAGE} WHERE p.space_id = ?2 AND p.parent_id IS ?3 AND p.deleted_at IS NULL
                 AND p.position > coalesce(?4, '')
             ORDER BY p.position LIMIT ?5"
        )))
        .bind(actor)
        .bind(space_id)
        .bind(parent_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().filter_map(page).collect())
    }

    async fn move_page(
        &self,
        page_id: &str,
        actor: &str,
        destination: &PageMove,
        now: i64,
    ) -> Result<MovedPage, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let row = authorize(&mut tx, page_id, actor, SpaceAction::EditPage).await?;
        let source: &str = row.get("space_id");
        let target = destination.space_id.as_deref().unwrap_or(source);
        if target != source {
            let space = spaces::authorize(&mut tx, target, actor, SpaceAction::EditPage).await?;
            let same_workspace =
                space.get::<&str, _>("workspace_id") == row.get::<&str, _>("workspace_id");
            if !move_allowed(scope(&row), spaces::scope(&space), same_workspace) {
                return Err(StoreError::OtherWorkspace);
            }
        }

        let parent_id = destination.parent_id.as_deref();
        let above = ancestors(&mut tx, target, parent_id).await?;
        if above.iter().any(|id| id == page_id) {
            return Err(StoreError::PageCycle);
        }
        let height: i64 = sqlx::query_scalar(AssertSqlSafe(format!(
            "{SUBTREE} SELECT max(depth) FROM subtree"
        )))
        .bind(page_id)
        .fetch_one(&mut *tx)
        .await?;
        if above.len() + height as usize > MAX_PAGE_DEPTH {
            return Err(StoreError::TooDeep);
        }

        let placement = &destination.placement;
        let key = position(&mut tx, target, parent_id, placement, Some(page_id)).await?;
        sqlx::query(
            "UPDATE pages SET space_id = ?2, parent_id = ?3, position = ?4, updated_by = ?5,
                 updated_at = ?6
             WHERE id = ?1",
        )
        .bind(page_id)
        .bind(target)
        .bind(parent_id)
        .bind(&key)
        .bind(actor)
        .bind(now)
        .execute(&mut *tx)
        .await?;
        if target != source {
            sqlx::query(AssertSqlSafe(format!(
                "{SUBTREE} UPDATE pages SET space_id = ?2 WHERE id IN (SELECT id FROM subtree)"
            )))
            .bind(page_id)
            .bind(target)
            .execute(&mut *tx)
            .await?;
        }
        let page = reread(&mut tx, page_id, actor).await?;
        let from_space = source.to_owned();
        tx.commit().await?;
        Ok(MovedPage { page, from_space })
    }

    async fn trash_page(
        &self,
        page_id: &str,
        actor: &str,
        now: i64,
    ) -> Result<PageSite, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let row = authorize(&mut tx, page_id, actor, SpaceAction::EditPage).await?;
        // A restore goes by the date alone: it must differ from any date already in the subtree.
        let latest: Option<i64> = sqlx::query_scalar(AssertSqlSafe(format!(
            "{SUBTREE} SELECT max(deleted_at) FROM pages WHERE id IN (SELECT id FROM subtree)"
        )))
        .bind(page_id)
        .fetch_one(&mut *tx)
        .await?;
        let date = latest.map_or(now, |latest| now.max(latest + 1));
        sqlx::query(AssertSqlSafe(format!(
            "{SUBTREE} UPDATE pages SET deleted_at = ?2, deleted_by = ?3
             WHERE deleted_at IS NULL AND id IN (SELECT id FROM subtree)"
        )))
        .bind(page_id)
        .bind(date)
        .bind(actor)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(site(&row))
    }

    async fn restore_page(&self, page_id: &str, actor: &str) -> Result<PageRecord, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let row = authorize(&mut tx, page_id, actor, SpaceAction::RestorePage).await?;
        let space_id: &str = row.get("space_id");
        let parent_id: Option<&str> = row.get("parent_id");
        let date: i64 = row.get("deleted_at");

        if let Some(parent_id) = parent_id
            && !is_live_in_space(&mut tx, parent_id, space_id).await?
        {
            let key = position(&mut tx, space_id, None, &Placement::Last, Some(page_id)).await?;
            sqlx::query("UPDATE pages SET parent_id = NULL, position = ?2 WHERE id = ?1")
                .bind(page_id)
                .bind(&key)
                .execute(&mut *tx)
                .await?;
        }
        sqlx::query(
            "WITH RECURSIVE same(id) AS (
                 SELECT ?1
                 UNION ALL
                 SELECT p.id FROM pages p JOIN same ON p.parent_id = same.id
                 WHERE p.deleted_at = ?2
             )
             UPDATE pages SET deleted_at = NULL, deleted_by = NULL
             WHERE id IN (SELECT id FROM same)",
        )
        .bind(page_id)
        .bind(date)
        .execute(&mut *tx)
        .await?;
        let page = reread(&mut tx, page_id, actor).await?;
        tx.commit().await?;
        Ok(page)
    }

    async fn purge_page(&self, page_id: &str, actor: &str) -> Result<PageSite, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let row = authorize(&mut tx, page_id, actor, SpaceAction::PurgePage).await?;
        sqlx::query("DELETE FROM pages WHERE id = ?1")
            .bind(page_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(site(&row))
    }

    async fn list_trashed_pages(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&DateCursor>,
        limit: u32,
    ) -> Result<Vec<PageRecord>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        spaces::authorize(&mut reader, space_id, actor, SpaceAction::ListTrash).await?;
        let filter = "p.space_id = ?2 AND p.deleted_at IS NOT NULL";
        let date = "p.deleted_at";
        pages_by_date(&mut reader, space_id, actor, filter, date, after, limit).await
    }

    async fn recent_pages(
        &self,
        workspace_id: &str,
        actor: &str,
        after: Option<&DateCursor>,
        limit: u32,
    ) -> Result<Vec<PageRecord>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let action = WorkspaceAction::Read;
        authorize_workspace(&mut reader, workspace_id, actor, action).await?;
        let filter = format!(
            "p.workspace_id = ?2 AND p.deleted_at IS NULL AND s.deleted_at IS NULL AND {HAS_ROLE}"
        );
        let date = "p.updated_at";
        pages_by_date(
            &mut reader,
            workspace_id,
            actor,
            &filter,
            date,
            after,
            limit,
        )
        .await
    }
}
