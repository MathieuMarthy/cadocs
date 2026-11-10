use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::pages::{self, SELECT_PAGE};
use super::spaces::{HAS_ROLE, space_access};
use super::workspaces::authorize as authorize_workspace;
use super::{BEGIN_WRITE, SqliteStore};
use crate::{IndexTarget, PageIndex, REINDEX_PAGE, SearchHit, SearchStore, StoreError, now_ms};
use cadocs_core::permissions::WorkspaceAction;

/// Queues a reindex, due at `run_at`, of each page `p` that `filter` keeps, with `?1` bound to
/// `value`. A page with one waiting keeps it. Answers whether any job was added.
pub(super) async fn queue_reindex(
    conn: &mut SqliteConnection,
    filter: &str,
    value: &str,
    run_at: i64,
) -> Result<bool, StoreError> {
    let added = sqlx::query(AssertSqlSafe(format!(
        "INSERT INTO jobs (id, kind, dedup_key, payload, run_at, created_at)
         SELECT lower(hex(randomblob(16))), '{REINDEX_PAGE}', 'reindex:' || p.id,
                json_object('page', p.id), ?2, ?3
         FROM pages p WHERE {filter}
         ON CONFLICT DO NOTHING"
    )))
    .bind(value)
    .bind(run_at)
    .bind(now_ms())
    .execute(conn)
    .await?
    .rows_affected();
    Ok(added > 0)
}

impl SearchStore for SqliteStore {
    async fn index_target(&self, page_id: &str) -> Result<Option<IndexTarget>, StoreError> {
        let row = sqlx::query(
            "SELECT w.id, w.language FROM pages p JOIN workspaces w ON w.id = p.workspace_id
             WHERE p.id = ?1",
        )
        .bind(page_id)
        .fetch_optional(&self.reader)
        .await?;
        Ok(row.map(|row| IndexTarget {
            workspace_id: row.get("id"),
            language: row.get("language"),
        }))
    }

    async fn index_page(&self, index: &PageIndex<'_>) -> Result<bool, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let workspace: Option<String> =
            sqlx::query_scalar("SELECT workspace_id FROM pages WHERE id = ?1")
                .bind(index.page_id)
                .fetch_optional(&mut *tx)
                .await?;
        let Some(workspace) = workspace else {
            return Ok(false);
        };
        let rowid: i64 = sqlx::query_scalar(
            "INSERT INTO page_text (page_id, plain_text, indexed_at) VALUES (?1, ?2, ?3)
             ON CONFLICT (page_id) DO UPDATE
                 SET plain_text = excluded.plain_text, indexed_at = excluded.indexed_at
             RETURNING id",
        )
        .bind(index.page_id)
        .bind(index.plain_text)
        .bind(now_ms())
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM page_fts WHERE rowid = ?1")
            .bind(rowid)
            .execute(&mut *tx)
            .await?;
        sqlx::query("INSERT INTO page_fts (rowid, title, body) VALUES (?1, ?2, ?3)")
            .bind(rowid)
            .bind(index.title)
            .bind(index.body)
            .execute(&mut *tx)
            .await?;
        // One statement each, on the writer: `forget_orphan` either deleted the attachment first,
        // and nothing is found, or finds this reference and keeps it.
        for attachment in index.attachments {
            sqlx::query(
                "INSERT INTO attachment_refs (attachment_id, page_id)
                 SELECT id, ?2 FROM attachments WHERE id = ?1 AND workspace_id = ?3
                 ON CONFLICT DO NOTHING",
            )
            .bind(attachment)
            .bind(index.page_id)
            .bind(&workspace)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(true)
    }

    async fn search(
        &self,
        workspace_id: &str,
        actor: &str,
        fts: &str,
        space: Option<&str>,
        limit: u32,
    ) -> Result<Vec<SearchHit>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        authorize_workspace(&mut reader, workspace_id, actor, WorkspaceAction::Read).await?;
        let columns = SELECT_PAGE
            .strip_prefix("SELECT ")
            .expect("SELECT_PAGE is a SELECT");
        // Ranked on ids alone, with the permission filters, so only the 20 kept rows bring their
        // text: a short prefix may match most of the database.
        let access = space_access!();
        let rows = sqlx::query(AssertSqlSafe(format!(
            "WITH ranked AS (
                 SELECT t.id AS text_id, p.id AS page_id,
                        bm25(page_fts, 5.0, 1.0) AS score, -- title, body
                        {access}
                 FROM page_fts f
                 JOIN page_text t ON t.id = f.rowid
                 JOIN pages p ON p.id = t.page_id
                 JOIN spaces s ON s.id = p.space_id
                 WHERE page_fts MATCH ?3 AND p.workspace_id = ?2 AND p.deleted_at IS NULL
                   AND s.deleted_at IS NULL AND (?4 IS NULL OR p.space_id = ?4) AND {HAS_ROLE}
                 ORDER BY score, p.id LIMIT ?5
             )
             SELECT t.plain_text, r.score, {columns}
             JOIN ranked r ON r.page_id = p.id
             JOIN page_text t ON t.id = r.text_id
             ORDER BY r.score, p.id"
        )))
        .bind(actor)
        .bind(workspace_id)
        .bind(fts)
        .bind(space)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows
            .iter()
            .filter_map(|row| {
                Some(SearchHit {
                    page: pages::page(row)?,
                    plain_text: row.get("plain_text"),
                })
            })
            .collect())
    }
}
