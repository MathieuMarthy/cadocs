use cadocs_core::permissions::SpaceAction;
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::attachments::attachment;
use super::pages::authorize;
use super::{BEGIN_WRITE, SqliteStore};
use crate::{Attachment, NewShare, Share, ShareStore, SharedPage, SharedView, StoreError};

const SHARE_COLUMNS: &str = "sh.id, sh.page_id, sh.token, sh.include_children, sh.indexable,
                             sh.created_by, sh.expires_at, sh.created_at";

const SHARED_PAGE: &str = "SELECT p.id, p.slug, p.title, p.updated_at FROM pages p";

/// Pages `?2` covers, from the seed `up(start, id)` it is given: each page climbs to its parent
/// until it meets `?2`, the shared page, and only with `?3`, the share's `include_children`. A
/// live page never sits under a trashed one, so the climb needs no check of the trash.
const COVERED_BY_CLIMB: &str = "
    UNION ALL
    SELECT up.start, p.parent_id FROM up JOIN pages p ON p.id = up.id
    WHERE up.id <> ?2 AND p.parent_id IS NOT NULL AND ?3
)";

fn share(row: &SqliteRow) -> Share {
    Share {
        id: row.get("id"),
        page_id: row.get("page_id"),
        token: row.get("token"),
        include_children: row.get("include_children"),
        indexable: row.get("indexable"),
        created_by: row.get("created_by"),
        expires_at: row.get("expires_at"),
        created_at: row.get("created_at"),
    }
}

fn shared_page(row: &SqliteRow) -> SharedPage {
    SharedPage {
        id: row.get("id"),
        slug: row.get("slug"),
        title: row.get("title"),
        updated_at: row.get("updated_at"),
    }
}

/// A share that opens now: not expired, its page and space out of the trash, its creator still
/// allowed to manage it (ADR 0030). Also answers the workspace of the page, where slugs are
/// unique.
async fn open_share(
    conn: &mut SqliteConnection,
    token: &str,
    now: i64,
) -> Result<Option<(Share, String)>, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!(
        "SELECT {SHARE_COLUMNS}, p.workspace_id FROM page_shares sh
         JOIN pages p ON p.id = sh.page_id JOIN spaces s ON s.id = p.space_id
         WHERE sh.token = ?1 AND (sh.expires_at IS NULL OR sh.expires_at > ?2)
           AND p.deleted_at IS NULL AND s.deleted_at IS NULL"
    )))
    .bind(token)
    .bind(now)
    .fetch_optional(&mut *conn)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let share = share(&row);
    // A creator whose account is gone leaves `created_by` empty.
    let Some(creator) = &share.created_by else {
        return Ok(None);
    };
    match authorize(conn, &share.page_id, creator, SpaceAction::ManageShares).await {
        Ok(_) => Ok(Some((share, row.get("workspace_id")))),
        Err(StoreError::PageNotFound | StoreError::NotAllowed) => Ok(None),
        Err(error) => Err(error),
    }
}

async fn select_share(conn: &mut SqliteConnection, share_id: &str) -> Result<Share, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!(
        "SELECT {SHARE_COLUMNS} FROM page_shares sh WHERE sh.id = ?1"
    )))
    .bind(share_id)
    .fetch_one(conn)
    .await?;
    Ok(share(&row))
}

async fn page_by_id(
    conn: &mut SqliteConnection,
    page_id: &str,
) -> Result<Option<SharedPage>, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!("{SHARED_PAGE} WHERE p.id = ?1")))
        .bind(page_id)
        .fetch_optional(conn)
        .await?;
    Ok(row.as_ref().map(shared_page))
}

/// The pages from the shared one down to the parent of `page_id`, or `None` if the climb never
/// meets it.
async fn trail(
    conn: &mut SqliteConnection,
    page_id: &str,
    root: &str,
) -> Result<Option<Vec<SharedPage>>, StoreError> {
    let rows = sqlx::query(
        "WITH RECURSIVE up(id, depth) AS (
             SELECT parent_id, 1 FROM pages WHERE id = ?1
             UNION ALL
             SELECT p.parent_id, up.depth + 1 FROM pages p JOIN up ON p.id = up.id
             WHERE up.id <> ?2
         )
         SELECT p.id, p.slug, p.title, p.updated_at FROM up JOIN pages p ON p.id = up.id
         ORDER BY up.depth DESC",
    )
    .bind(page_id)
    .bind(root)
    .fetch_all(conn)
    .await?;
    let trail: Vec<SharedPage> = rows.iter().map(shared_page).collect();
    Ok(trail
        .first()
        .is_some_and(|top| top.id == root)
        .then_some(trail))
}

impl ShareStore for SqliteStore {
    async fn create_share(&self, new: &NewShare) -> Result<Share, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize(&mut tx, &new.page_id, &new.actor, SpaceAction::ManageShares).await?;
        sqlx::query(
            "INSERT INTO page_shares (id, page_id, token, include_children, indexable, created_by,
                                      expires_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&new.id)
        .bind(&new.page_id)
        .bind(&new.token)
        .bind(new.include_children)
        .bind(new.indexable)
        .bind(&new.actor)
        .bind(new.expires_at)
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        let created = select_share(&mut tx, &new.id).await?;
        tx.commit().await?;
        Ok(created)
    }

    async fn list_shares(
        &self,
        page_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
        now: i64,
    ) -> Result<Vec<Share>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        authorize(&mut reader, page_id, actor, SpaceAction::ManageShares).await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "SELECT {SHARE_COLUMNS} FROM page_shares sh
             WHERE sh.page_id = ?1 AND sh.id > coalesce(?2, '')
               AND (sh.expires_at IS NULL OR sh.expires_at > ?4)
             ORDER BY sh.id LIMIT ?3"
        )))
        .bind(page_id)
        .bind(after)
        .bind(limit)
        .bind(now)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().map(share).collect())
    }

    async fn delete_share(&self, share_id: &str, actor: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let page_id: Option<String> =
            sqlx::query_scalar("SELECT page_id FROM page_shares WHERE id = ?1")
                .bind(share_id)
                .fetch_optional(&mut *tx)
                .await?;
        let page_id = page_id.ok_or(StoreError::ShareNotFound)?;
        match authorize(&mut tx, &page_id, actor, SpaceAction::ManageShares).await {
            Err(StoreError::PageNotFound) => return Err(StoreError::ShareNotFound),
            Err(error) => return Err(error),
            Ok(_) => {}
        }
        sqlx::query("DELETE FROM page_shares WHERE id = ?1")
            .bind(share_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn shared_view(
        &self,
        token: &str,
        slug: Option<&str>,
        now: i64,
        children_limit: u32,
    ) -> Result<Option<SharedView>, StoreError> {
        // One snapshot for every query below.
        let mut tx = self.reader.begin().await?;
        let Some((share, workspace_id)) = open_share(&mut tx, token, now).await? else {
            return Ok(None);
        };
        let root = page_by_id(&mut tx, &share.page_id)
            .await?
            .ok_or(StoreError::PageNotFound)?;
        let (page, trail) = match slug {
            Some(slug) if slug != root.slug => {
                if !share.include_children {
                    return Ok(None);
                }
                let row = sqlx::query(AssertSqlSafe(format!(
                    "{SHARED_PAGE} WHERE p.workspace_id = ?1 AND p.slug = ?2
                     AND p.deleted_at IS NULL"
                )))
                .bind(&workspace_id)
                .bind(slug)
                .fetch_optional(&mut *tx)
                .await?;
                let Some(page) = row.as_ref().map(shared_page) else {
                    return Ok(None);
                };
                let Some(trail) = self::trail(&mut tx, &page.id, &root.id).await? else {
                    return Ok(None);
                };
                (page, trail)
            }
            _ => (root.clone(), Vec::new()),
        };

        let children = if share.include_children {
            let rows = sqlx::query(AssertSqlSafe(format!(
                "{SHARED_PAGE} WHERE p.parent_id = ?1 AND p.deleted_at IS NULL
                 ORDER BY p.position LIMIT ?2"
            )))
            .bind(&page.id)
            .bind(children_limit)
            .fetch_all(&mut *tx)
            .await?;
            rows.iter().map(shared_page).collect()
        } else {
            Vec::new()
        };
        Ok(Some(SharedView {
            share,
            page,
            trail,
            children,
        }))
    }

    async fn shared_pages(
        &self,
        token: &str,
        ids: &[String],
        now: i64,
    ) -> Result<Vec<SharedPage>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut tx = self.reader.begin().await?;
        let Some((share, workspace_id)) = open_share(&mut tx, token, now).await? else {
            return Ok(Vec::new());
        };
        let ids = serde_json::to_string(ids).expect("strings serialize");
        let rows = sqlx::query(AssertSqlSafe(format!(
            "WITH RECURSIVE up(start, id) AS (
                 SELECT p.id, p.id FROM pages p
                 WHERE p.id IN (SELECT value FROM json_each(?1))
                   AND p.workspace_id = ?4 AND p.deleted_at IS NULL
                 {COVERED_BY_CLIMB}
             {SHARED_PAGE} WHERE p.id IN (SELECT start FROM up WHERE id = ?2)"
        )))
        .bind(ids)
        .bind(&share.page_id)
        .bind(share.include_children)
        .bind(&workspace_id)
        .fetch_all(&mut *tx)
        .await?;
        Ok(rows.iter().map(shared_page).collect())
    }

    async fn shared_attachment(
        &self,
        token: &str,
        attachment_id: &str,
        now: i64,
    ) -> Result<Option<Attachment>, StoreError> {
        let mut tx = self.reader.begin().await?;
        let Some((share, _)) = open_share(&mut tx, token, now).await? else {
            return Ok(None);
        };
        // What the content shows now, not `attachment_refs`, which keeps files a page once showed.
        let row = sqlx::query(AssertSqlSafe(format!(
            "WITH RECURSIVE up(start, id) AS (
                 SELECT p.id, p.id FROM page_attachments r JOIN pages p ON p.id = r.page_id
                 WHERE r.attachment_id = ?1 AND p.deleted_at IS NULL
                 {COVERED_BY_CLIMB}
             SELECT a.id, a.workspace_id, a.file_name, a.content_type, a.size, a.sha256,
                    a.uploaded_by, a.created_at
             FROM attachments a
             WHERE a.id = ?1 AND EXISTS (SELECT 1 FROM up WHERE up.id = ?2)"
        )))
        .bind(attachment_id)
        .bind(&share.page_id)
        .bind(share.include_children)
        .fetch_optional(&mut *tx)
        .await?;
        Ok(row.as_ref().map(attachment))
    }
}
