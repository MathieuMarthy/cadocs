use cadocs_core::permissions::SpaceAction;
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::pages::{self, SELECT_PAGE};
use super::{BEGIN_WRITE, SqliteStore};
use crate::{
    CREATE_REVISION, NewRevision, Revision, RevisionAuthor, RevisionStore, RevisionSummary,
    RevisionTiming, StoreError, now_ms,
};

const DAY_MS: i64 = 86_400_000;

/// The columns of a summary, from `page_revisions r`, with the names of the authors who still
/// have an account, in the order they were recorded.
const SUMMARY: &str = "r.number, r.title, r.created_at,
     (SELECT json_group_array(json_array(u.id, u.name))
      FROM json_each(r.authors) a JOIN users u ON u.id = a.value) AS author_names";

/// Records what a flush by `author` means for the next revision of the page: the author, and a
/// job due after the pause, pushed back by each flush up to `timing.max` after the first one.
pub(super) async fn note_edit(
    conn: &mut SqliteConnection,
    page_id: &str,
    author: Option<&str>,
    timing: RevisionTiming,
    now: i64,
) -> Result<(), StoreError> {
    if let Some(author) = author {
        sqlx::query(
            "INSERT INTO revision_authors (page_id, user_id, last_at)
             SELECT ?1, id, ?3 FROM users WHERE id = ?2
             ON CONFLICT (page_id, user_id) DO UPDATE SET last_at = excluded.last_at",
        )
        .bind(page_id)
        .bind(author)
        .bind(now)
        .execute(&mut *conn)
        .await?;
    }
    sqlx::query(AssertSqlSafe(format!(
        "INSERT INTO jobs (id, kind, dedup_key, payload, run_at, created_at)
         VALUES (lower(hex(randomblob(16))), '{CREATE_REVISION}', 'revision:' || ?1,
                 json_object('page', ?1), ?2 + ?3, ?2)
         ON CONFLICT (dedup_key) WHERE dedup_key IS NOT NULL AND status = 'pending'
         DO UPDATE SET run_at = min(jobs.created_at + ?4, excluded.run_at)"
    )))
    .bind(page_id)
    .bind(now)
    .bind(millis(timing.pause))
    .bind(millis(timing.max))
    .execute(conn)
    .await?;
    Ok(())
}

fn millis(duration: std::time::Duration) -> i64 {
    i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
}

fn summary(row: &SqliteRow) -> RevisionSummary {
    let names: String = row.get("author_names");
    let authors = serde_json::from_str::<Vec<(String, String)>>(&names)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name)| RevisionAuthor { id, name })
        .collect();
    RevisionSummary {
        number: row.get("number"),
        title: row.get("title"),
        authors,
        created_at: row.get("created_at"),
    }
}

async fn select_revision(
    conn: &mut SqliteConnection,
    page_id: &str,
    number: i64,
) -> Result<Revision, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!(
        "SELECT {SUMMARY}, r.state FROM page_revisions r WHERE r.page_id = ?1 AND r.number = ?2"
    )))
    .bind(page_id)
    .bind(number)
    .fetch_optional(conn)
    .await?
    .ok_or(StoreError::RevisionNotFound)?;
    Ok(Revision {
        summary: summary(&row),
        state: row.get("state"),
    })
}

impl SqliteStore {
    async fn checked_revision(
        &self,
        page_id: &str,
        number: i64,
        actor: &str,
        action: SpaceAction,
    ) -> Result<Revision, StoreError> {
        let mut reader = self.reader.begin().await?;
        pages::authorize(&mut reader, page_id, actor, action).await?;
        let revision = select_revision(&mut reader, page_id, number).await?;
        reader.commit().await?;
        Ok(revision)
    }
}

impl RevisionStore for SqliteStore {
    async fn latest_revision(&self, page_id: &str) -> Result<Option<Revision>, StoreError> {
        let mut reader = self.reader.begin().await?;
        let number: Option<i64> =
            sqlx::query_scalar("SELECT max(number) FROM page_revisions WHERE page_id = ?1")
                .bind(page_id)
                .fetch_one(&mut *reader)
                .await?;
        let revision = match number {
            Some(number) => Some(select_revision(&mut reader, page_id, number).await?),
            None => None,
        };
        reader.commit().await?;
        Ok(revision)
    }

    async fn add_revision(&self, revision: &NewRevision<'_>) -> Result<Option<i64>, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let exists: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pages WHERE id = ?1)")
            .bind(revision.page_id)
            .fetch_one(&mut *tx)
            .await?;
        if !exists {
            return Ok(None);
        }
        let number: i64 = sqlx::query_scalar(
            "INSERT INTO page_revisions (id, page_id, number, title, state, authors, created_at)
             SELECT ?1, ?2,
                    (SELECT coalesce(max(number), 0) + 1 FROM page_revisions WHERE page_id = ?2),
                    ?3, ?4,
                    (SELECT json_group_array(user_id)
                     FROM (SELECT user_id FROM revision_authors WHERE page_id = ?2
                           ORDER BY last_at, user_id)),
                    ?5
             RETURNING number",
        )
        .bind(revision.id)
        .bind(revision.page_id)
        .bind(revision.title)
        .bind(revision.state)
        .bind(now_ms())
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM revision_authors WHERE page_id = ?1 AND last_at <= ?2")
            .bind(revision.page_id)
            .bind(revision.read_at)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Some(number))
    }

    async fn revisions(
        &self,
        page_id: &str,
        actor: &str,
        before: Option<i64>,
        limit: u32,
    ) -> Result<Vec<RevisionSummary>, StoreError> {
        let mut reader = self.reader.begin().await?;
        pages::authorize(&mut reader, page_id, actor, SpaceAction::ReadPage).await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "SELECT {SUMMARY} FROM page_revisions r
             WHERE r.page_id = ?1 AND (?2 IS NULL OR r.number < ?2)
             ORDER BY r.number DESC LIMIT ?3"
        )))
        .bind(page_id)
        .bind(before)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        reader.commit().await?;
        Ok(rows.iter().map(summary).collect())
    }

    async fn revision(
        &self,
        page_id: &str,
        number: i64,
        actor: &str,
    ) -> Result<Revision, StoreError> {
        self.checked_revision(page_id, number, actor, SpaceAction::ReadPage)
            .await
    }

    async fn revision_to_restore(
        &self,
        page_id: &str,
        number: i64,
        actor: &str,
    ) -> Result<Revision, StoreError> {
        self.checked_revision(page_id, number, actor, SpaceAction::RestoreRevision)
            .await
    }

    async fn thin_revisions(&self, before: i64) -> Result<u64, StoreError> {
        let deleted = sqlx::query(
            "DELETE FROM page_revisions WHERE id IN (
                 SELECT id FROM (
                     SELECT id, created_at,
                            row_number() OVER (PARTITION BY page_id ORDER BY number DESC)
                                AS newest,
                            row_number() OVER (PARTITION BY page_id, created_at / ?2
                                               ORDER BY number DESC) AS newest_of_day
                     FROM page_revisions
                 )
                 WHERE created_at < ?1 AND newest > 1 AND newest_of_day > 1
             )",
        )
        .bind(before)
        .bind(DAY_MS)
        .execute(&self.writer)
        .await?
        .rows_affected();
        Ok(deleted)
    }

    async fn readable_titles(
        &self,
        actor: &str,
        ids: &[String],
    ) -> Result<Vec<(String, String)>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let ids = serde_json::to_string(ids).expect("strings make JSON");
        let mut reader = self.reader.acquire().await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_PAGE} WHERE p.id IN (SELECT value FROM json_each(?2))
                 AND p.deleted_at IS NULL AND s.deleted_at IS NULL"
        )))
        .bind(actor)
        .bind(ids)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows
            .iter()
            .filter(|row| pages::scope(row).allows(SpaceAction::ReadPage))
            .map(|row| (row.get("id"), row.get("title")))
            .collect())
    }
}
