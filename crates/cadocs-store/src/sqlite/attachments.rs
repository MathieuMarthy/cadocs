use cadocs_core::permissions::{Scope, SpaceAction, delete_allowed, download_allowed};
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, QueryBuilder, Row, SqliteConnection};

use super::pages::{self, SELECT_PAGE};
use super::{BEGIN_WRITE, SqliteStore};
use crate::{Attachment, AttachmentStore, NewAttachment, StoreError, UploadTarget};

const SELECT_ATTACHMENT: &str = "SELECT id, workspace_id, file_name, content_type, size, sha256,
                                        uploaded_by, created_at
                                 FROM attachments WHERE id = ?1";

fn attachment(row: &SqliteRow) -> Attachment {
    Attachment {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        file_name: row.get("file_name"),
        content_type: row.get("content_type"),
        size: row.get("size"),
        sha256: row.get("sha256"),
        uploaded_by: row.get("uploaded_by"),
        created_at: row.get("created_at"),
    }
}

/// What the user `actor` is worth on each page that references the attachment.
async fn referencing_scopes(
    conn: &mut SqliteConnection,
    id: &str,
    actor: &str,
) -> Result<Vec<Scope>, StoreError> {
    let rows = sqlx::query(AssertSqlSafe(format!(
        "{SELECT_PAGE} JOIN attachment_refs r ON r.page_id = p.id WHERE r.attachment_id = ?2"
    )))
    .bind(actor)
    .bind(id)
    .fetch_all(conn)
    .await?;
    Ok(rows.iter().map(pages::scope).collect())
}

async fn used_bytes(conn: &mut SqliteConnection, workspace_id: &str) -> Result<i64, StoreError> {
    let used = sqlx::query_scalar(
        "SELECT coalesce(sum(size), 0) FROM attachments WHERE workspace_id = ?1",
    )
    .bind(workspace_id)
    .fetch_one(conn)
    .await?;
    Ok(used)
}

impl AttachmentStore for SqliteStore {
    async fn upload_target(&self, page_id: &str, actor: &str) -> Result<UploadTarget, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let row = pages::authorize(&mut reader, page_id, actor, SpaceAction::ManageAttachments);
        let row = row.await?;
        let workspace_id: String = row.get("workspace_id");
        let used_bytes = used_bytes(&mut reader, &workspace_id).await?;
        Ok(UploadTarget {
            workspace_id,
            used_bytes,
        })
    }

    async fn create_attachment(
        &self,
        new: &NewAttachment,
        quota_bytes: Option<i64>,
    ) -> Result<Attachment, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let row = pages::authorize(
            &mut tx,
            &new.page_id,
            &new.actor,
            SpaceAction::ManageAttachments,
        );
        let row = row.await?;
        let workspace_id: String = row.get("workspace_id");
        if let Some(quota) = quota_bytes {
            let used = used_bytes(&mut tx, &workspace_id).await?;
            if used.saturating_add(new.size) > quota {
                return Err(StoreError::QuotaExceeded);
            }
        }
        sqlx::query(
            "INSERT INTO attachments (id, workspace_id, file_name, content_type, size, sha256,
                                      uploaded_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        )
        .bind(&new.id)
        .bind(&workspace_id)
        .bind(&new.file_name)
        .bind(&new.content_type)
        .bind(new.size)
        .bind(&new.sha256)
        .bind(&new.actor)
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO attachment_refs (attachment_id, page_id) VALUES (?1, ?2)")
            .bind(&new.id)
            .bind(&new.page_id)
            .execute(&mut *tx)
            .await?;
        let row = sqlx::query(SELECT_ATTACHMENT)
            .bind(&new.id)
            .fetch_one(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(attachment(&row))
    }

    async fn attachment(&self, id: &str, actor: &str) -> Result<Option<Attachment>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        if !download_allowed(referencing_scopes(&mut reader, id, actor).await?) {
            return Ok(None);
        }
        let row = sqlx::query(SELECT_ATTACHMENT)
            .bind(id)
            .fetch_optional(&mut *reader)
            .await?;
        Ok(row.as_ref().map(attachment))
    }

    async fn delete_attachment(&self, id: &str, actor: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let scopes = referencing_scopes(&mut tx, id, actor).await?;
        if !delete_allowed(scopes.iter().copied()) {
            return Err(if download_allowed(scopes) {
                StoreError::NotAllowed
            } else {
                StoreError::AttachmentNotFound
            });
        }
        sqlx::query("DELETE FROM attachments WHERE id = ?1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    async fn orphan_attachments(&self, limit: u32) -> Result<Vec<String>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let ids = sqlx::query_scalar(
            "SELECT a.id FROM attachments a
             WHERE NOT EXISTS (SELECT 1 FROM attachment_refs r WHERE r.attachment_id = a.id)
             LIMIT ?1",
        )
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(ids)
    }

    async fn forget_orphan(&self, id: &str) -> Result<bool, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let deleted = sqlx::query(
            "DELETE FROM attachments WHERE id = ?1
             AND NOT EXISTS (SELECT 1 FROM attachment_refs r WHERE r.attachment_id = ?1)",
        )
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
        tx.commit().await?;
        Ok(deleted > 0)
    }

    async fn known_attachments(&self, ids: &[String]) -> Result<Vec<String>, StoreError> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let mut reader = self.reader.acquire().await?;
        let mut query = QueryBuilder::new("SELECT id FROM attachments WHERE id IN (");
        let mut list = query.separated(", ");
        for id in ids {
            list.push_bind(id);
        }
        query.push(")");
        let known = query.build_query_scalar().fetch_all(&mut *reader).await?;
        Ok(known)
    }
}
