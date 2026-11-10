//! Everything here runs on the writer, reads too: this polls when the server is idle, and a reader
//! connection costs a thread and a page cache.

use std::sync::Arc;

use sqlx::Row;
use tokio::sync::Notify;

use super::{BEGIN_WRITE, SqliteStore};
use crate::{ExpiredPage, ExpiredSpace, Job, JobStore, NewJob, PageSite, StoreError};

/// A job that outlives its process gets this in `last_error`.
const INTERRUPTED: &str = "interrupted on every attempt";

impl JobStore for SqliteStore {
    fn jobs_added(&self) -> Arc<Notify> {
        self.jobs_added.clone()
    }

    async fn enqueue(&self, job: &NewJob) -> Result<(), StoreError> {
        let added = sqlx::query(
            "INSERT INTO jobs (id, kind, dedup_key, payload, run_at, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)
             ON CONFLICT DO NOTHING",
        )
        .bind(&job.id)
        .bind(&job.kind)
        .bind(&job.dedup_key)
        .bind(&job.payload)
        .bind(job.run_at)
        .bind(job.now)
        .execute(&self.writer)
        .await?
        .rows_affected();
        if added > 0 {
            self.jobs_added.notify_one();
        }
        Ok(())
    }

    async fn claim_job(&self, now: i64) -> Result<Option<Job>, StoreError> {
        let row = sqlx::query(
            "UPDATE jobs SET status = 'running', attempts = attempts + 1, locked_at = ?1
             WHERE id = (SELECT id FROM jobs WHERE status = 'pending' AND run_at <= ?1
                         ORDER BY run_at, id LIMIT 1)
             RETURNING id, kind, payload, attempts",
        )
        .bind(now)
        .fetch_optional(&self.writer)
        .await?;
        Ok(row.map(|row| Job {
            id: row.get("id"),
            kind: row.get("kind"),
            payload: row.get("payload"),
            attempts: row.get::<i64, _>("attempts") as u32,
        }))
    }

    async fn next_job_at(&self) -> Result<Option<i64>, StoreError> {
        let next = sqlx::query_scalar("SELECT min(run_at) FROM jobs WHERE status = 'pending'")
            .fetch_one(&self.writer)
            .await?;
        Ok(next)
    }

    async fn finish_job(&self, id: &str) -> Result<(), StoreError> {
        sqlx::query("DELETE FROM jobs WHERE id = ?1")
            .bind(id)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    async fn retry_job(&self, id: &str, run_at: i64, error: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        sqlx::query(
            "UPDATE OR IGNORE jobs
             SET status = 'pending', run_at = ?2, locked_at = NULL, last_error = ?3
             WHERE id = ?1",
        )
        .bind(id)
        .bind(run_at)
        .bind(error)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM jobs WHERE id = ?1 AND status = 'running'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn fail_job(&self, id: &str, error: &str) -> Result<(), StoreError> {
        sqlx::query("UPDATE jobs SET status = 'failed', last_error = ?2 WHERE id = ?1")
            .bind(id)
            .bind(error)
            .execute(&self.writer)
            .await?;
        Ok(())
    }

    async fn reset_running_jobs(&self, max_attempts: u32) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        sqlx::query(
            "UPDATE jobs SET status = 'failed', last_error = ?2
             WHERE status = 'running' AND attempts >= ?1",
        )
        .bind(i64::from(max_attempts))
        .bind(INTERRUPTED)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "UPDATE OR IGNORE jobs SET status = 'pending', locked_at = NULL
             WHERE status = 'running'",
        )
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM jobs WHERE status = 'running'")
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn purge_expired(&self, now: i64, failed_before: i64) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        for table in [
            "DELETE FROM sessions WHERE expires_at <= ?1",
            "DELETE FROM password_resets WHERE expires_at <= ?1",
            "DELETE FROM workspace_invitations WHERE expires_at <= ?1",
        ] {
            sqlx::query(table).bind(now).execute(&mut *tx).await?;
        }
        sqlx::query("DELETE FROM jobs WHERE status = 'failed' AND locked_at < ?1")
            .bind(failed_before)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn expired_pages(&self, before: i64, limit: u32) -> Result<Vec<ExpiredPage>, StoreError> {
        let rows = sqlx::query(
            "SELECT p.id, p.workspace_id, p.space_id
             FROM pages p LEFT JOIN pages parent ON parent.id = p.parent_id
             WHERE p.deleted_at <= ?1 AND (parent.deleted_at IS NULL OR parent.deleted_at > ?1)
             ORDER BY p.deleted_at, p.id LIMIT ?2",
        )
        .bind(before)
        .bind(limit)
        .fetch_all(&self.writer)
        .await?;
        let page = |row: sqlx::sqlite::SqliteRow| ExpiredPage {
            id: row.get("id"),
            site: PageSite {
                workspace_id: row.get("workspace_id"),
                space_id: row.get("space_id"),
            },
        };
        Ok(rows.into_iter().map(page).collect())
    }

    async fn purge_expired_page(&self, page_id: &str, before: i64) -> Result<bool, StoreError> {
        let deleted = sqlx::query("DELETE FROM pages WHERE id = ?1 AND deleted_at <= ?2")
            .bind(page_id)
            .bind(before)
            .execute(&self.writer)
            .await?;
        Ok(deleted.rows_affected() > 0)
    }

    async fn expired_spaces(
        &self,
        before: i64,
        limit: u32,
    ) -> Result<Vec<ExpiredSpace>, StoreError> {
        let rows = sqlx::query(
            "SELECT id, workspace_id FROM spaces WHERE deleted_at <= ?1
             ORDER BY deleted_at, id LIMIT ?2",
        )
        .bind(before)
        .bind(limit)
        .fetch_all(&self.writer)
        .await?;
        let space = |row: sqlx::sqlite::SqliteRow| ExpiredSpace {
            id: row.get("id"),
            workspace_id: row.get("workspace_id"),
        };
        Ok(rows.into_iter().map(space).collect())
    }

    async fn purge_expired_space(&self, space_id: &str, before: i64) -> Result<bool, StoreError> {
        let deleted = sqlx::query("DELETE FROM spaces WHERE id = ?1 AND deleted_at <= ?2")
            .bind(space_id)
            .bind(before)
            .execute(&self.writer)
            .await?;
        Ok(deleted.rows_affected() > 0)
    }
}
