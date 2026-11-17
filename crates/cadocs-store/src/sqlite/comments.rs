use cadocs_core::permissions::SpaceAction;
use sqlx::sqlite::SqliteRow;
use sqlx::{AssertSqlSafe, Row, SqliteConnection};

use super::pages;
use super::{BEGIN_WRITE, SqliteStore};
use crate::{
    Anchor, Comment, CommentChange, CommentStore, CommentUser, NewComment, PageSite, StoreError,
    Thread,
};

/// A comment with the names of its author and of who resolved it, while their accounts exist.
const SELECT_COMMENT: &str = "SELECT c.id, c.page_id, c.parent_id, c.body,
         c.anchor_start, c.anchor_end, c.anchor_text, c.resolved_at, c.created_at, c.updated_at,
         a.id AS author_id, a.name AS author_name, r.id AS resolver_id, r.name AS resolver_name
     FROM comments c
     LEFT JOIN users a ON a.id = c.author_id
     LEFT JOIN users r ON r.id = c.resolved_by";

fn user(row: &SqliteRow, id: &str, name: &str) -> Option<CommentUser> {
    let id: Option<String> = row.get(id);
    Some(CommentUser {
        id: id?,
        name: row.get(name),
    })
}

fn comment(row: &SqliteRow) -> Comment {
    let start: Option<Vec<u8>> = row.get("anchor_start");
    let end: Option<Vec<u8>> = row.get("anchor_end");
    let text: Option<String> = row.get("anchor_text");
    let anchor = match (start, end, text) {
        (Some(start), Some(end), Some(text)) => Some(Anchor { start, end, text }),
        _ => None,
    };
    Comment {
        id: row.get("id"),
        page_id: row.get("page_id"),
        parent_id: row.get("parent_id"),
        author: user(row, "author_id", "author_name"),
        body: row.get("body"),
        anchor,
        resolved_at: row.get("resolved_at"),
        resolved_by: user(row, "resolver_id", "resolver_name"),
        created_at: row.get("created_at"),
        updated_at: row.get("updated_at"),
    }
}

async fn select_comment(
    conn: &mut SqliteConnection,
    comment_id: &str,
) -> Result<Comment, StoreError> {
    let row = sqlx::query(AssertSqlSafe(format!("{SELECT_COMMENT} WHERE c.id = ?1")))
        .bind(comment_id)
        .fetch_optional(conn)
        .await?;
    row.as_ref().map(comment).ok_or(StoreError::CommentNotFound)
}

/// What deciding on a comment needs to know about its row.
struct Target {
    parent_id: Option<String>,
    site: PageSite,
}

/// Which action changing the comment is for `actor`: their own, or another user's.
enum Change {
    Edit,
    Resolve,
}

/// The comment's place, once `actor` may change it. A user who cannot read its page must not
/// learn that it exists.
async fn authorize_comment(
    conn: &mut SqliteConnection,
    comment_id: &str,
    actor: &str,
    change: Change,
) -> Result<Target, StoreError> {
    let row = sqlx::query("SELECT page_id, parent_id, author_id FROM comments WHERE id = ?1")
        .bind(comment_id)
        .fetch_optional(&mut *conn)
        .await?
        .ok_or(StoreError::CommentNotFound)?;
    let page_id: String = row.get("page_id");
    let author_id: Option<String> = row.get("author_id");
    let action = match change {
        Change::Resolve => SpaceAction::WriteComment,
        Change::Edit if author_id.as_deref() == Some(actor) => SpaceAction::EditOwnComment,
        Change::Edit => SpaceAction::EditOtherComment,
    };
    match pages::authorize(conn, &page_id, actor, action).await {
        Err(StoreError::PageNotFound) => Err(StoreError::CommentNotFound),
        Err(error) => Err(error),
        Ok(page) => Ok(Target {
            parent_id: row.get("parent_id"),
            site: pages::site(&page),
        }),
    }
}

impl CommentStore for SqliteStore {
    async fn threads(
        &self,
        page_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Thread>, StoreError> {
        let mut reader = self.reader.begin().await?;
        pages::authorize(&mut reader, page_id, actor, SpaceAction::ReadComments).await?;
        let rows = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_COMMENT}
             WHERE c.page_id = ?1 AND c.parent_id IS NULL AND c.id > coalesce(?2, '')
             ORDER BY c.id LIMIT ?3"
        )))
        .bind(page_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        let mut threads: Vec<Thread> = rows
            .iter()
            .map(|row| Thread {
                comment: comment(row),
                replies: Vec::new(),
            })
            .collect();
        let ids: Vec<&str> = threads.iter().map(|t| t.comment.id.as_str()).collect();
        let ids = serde_json::to_string(&ids).expect("strings make JSON");
        let replies = sqlx::query(AssertSqlSafe(format!(
            "{SELECT_COMMENT}
             WHERE c.parent_id IN (SELECT value FROM json_each(?1))
             ORDER BY c.parent_id, c.id"
        )))
        .bind(ids)
        .fetch_all(&mut *reader)
        .await?;
        reader.commit().await?;
        for reply in replies.iter().map(comment) {
            let thread = threads
                .iter_mut()
                .find(|t| Some(&t.comment.id) == reply.parent_id.as_ref());
            if let Some(thread) = thread {
                thread.replies.push(reply);
            }
        }
        Ok(threads)
    }

    async fn create_comment(&self, new: &NewComment) -> Result<CommentChange, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let action = SpaceAction::WriteComment;
        let page = pages::authorize(&mut tx, &new.page_id, &new.actor, action).await?;
        if let Some(parent_id) = &new.parent_id {
            let parent =
                sqlx::query("SELECT parent_id FROM comments WHERE id = ?1 AND page_id = ?2")
                    .bind(parent_id)
                    .bind(&new.page_id)
                    .fetch_optional(&mut *tx)
                    .await?
                    .ok_or(StoreError::UnknownParent)?;
            if parent.get::<Option<String>, _>("parent_id").is_some() {
                return Err(StoreError::NestedReply);
            }
        }
        let anchor = new.anchor.as_ref();
        sqlx::query(
            "INSERT INTO comments (id, page_id, parent_id, author_id, body,
                                   anchor_start, anchor_end, anchor_text, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?9)",
        )
        .bind(&new.id)
        .bind(&new.page_id)
        .bind(&new.parent_id)
        .bind(&new.actor)
        .bind(&new.body)
        .bind(anchor.map(|a| &a.start))
        .bind(anchor.map(|a| &a.end))
        .bind(anchor.map(|a| &a.text))
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        let comment = select_comment(&mut tx, &new.id).await?;
        tx.commit().await?;
        Ok(CommentChange {
            comment,
            site: pages::site(&page),
        })
    }

    async fn edit_comment(
        &self,
        comment_id: &str,
        actor: &str,
        body: &str,
        now: i64,
    ) -> Result<CommentChange, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let target = authorize_comment(&mut tx, comment_id, actor, Change::Edit).await?;
        sqlx::query("UPDATE comments SET body = ?2, updated_at = ?3 WHERE id = ?1")
            .bind(comment_id)
            .bind(body)
            .bind(now)
            .execute(&mut *tx)
            .await?;
        let comment = select_comment(&mut tx, comment_id).await?;
        tx.commit().await?;
        Ok(CommentChange {
            comment,
            site: target.site,
        })
    }

    async fn delete_comment(
        &self,
        comment_id: &str,
        actor: &str,
    ) -> Result<CommentChange, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let target = authorize_comment(&mut tx, comment_id, actor, Change::Edit).await?;
        let comment = select_comment(&mut tx, comment_id).await?;
        // Replies go with their thread, by their foreign key.
        sqlx::query("DELETE FROM comments WHERE id = ?1")
            .bind(comment_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(CommentChange {
            comment,
            site: target.site,
        })
    }

    async fn resolve_comment(
        &self,
        comment_id: &str,
        actor: &str,
        resolved: bool,
        now: i64,
    ) -> Result<CommentChange, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let target = authorize_comment(&mut tx, comment_id, actor, Change::Resolve).await?;
        if target.parent_id.is_some() {
            return Err(StoreError::NotAThread);
        }
        let statement = if resolved {
            // A thread resolved already keeps who resolved it, and when.
            "UPDATE comments SET resolved_at = ?2, resolved_by = ?3
             WHERE id = ?1 AND resolved_at IS NULL"
        } else {
            "UPDATE comments SET resolved_at = NULL, resolved_by = NULL WHERE id = ?1"
        };
        sqlx::query(statement)
            .bind(comment_id)
            .bind(now)
            .bind(actor)
            .execute(&mut *tx)
            .await?;
        let comment = select_comment(&mut tx, comment_id).await?;
        tx.commit().await?;
        Ok(CommentChange {
            comment,
            site: target.site,
        })
    }
}
