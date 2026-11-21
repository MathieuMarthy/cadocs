//! Mentions of users by a page's body or by a comment, kept in step with what that text names.

use sqlx::SqliteConnection;

use crate::StoreError;

/// Where mentions come from: the body of a page, or one of its comments.
pub(super) struct Place<'a> {
    pub page_id: &'a str,
    /// `None` for the page's body.
    pub comment_id: Option<&'a str>,
    pub workspace_id: &'a str,
}

/// Makes the mentions of `place` those of `users`: a user no longer named, or no longer a member,
/// loses their row, a user named anew gets one by `author`, and a user still named keeps theirs as
/// it was, read or not. A user named anew, `author` apart, is told of it (`notifications`). Only members of the workspace are recorded: anyone else is an id no one
/// may be told about.
pub(super) async fn replace(
    conn: &mut SqliteConnection,
    place: &Place<'_>,
    users: &[String],
    author: Option<&str>,
    now: i64,
) -> Result<(), StoreError> {
    let users = serde_json::to_string(users).expect("strings make JSON");
    sqlx::query(
        "DELETE FROM mentions WHERE page_id = ?1 AND comment_id IS ?2
             AND (user_id NOT IN (SELECT value FROM json_each(?3))
                  OR user_id NOT IN (SELECT user_id FROM workspace_members
                                     WHERE workspace_id = ?4))",
    )
    .bind(place.page_id)
    .bind(place.comment_id)
    .bind(&users)
    .bind(place.workspace_id)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "DELETE FROM notifications WHERE kind = 'mention' AND page_id = ?1 AND comment_id IS ?2
             AND (user_id NOT IN (SELECT value FROM json_each(?3))
                  OR user_id NOT IN (SELECT user_id FROM workspace_members
                                     WHERE workspace_id = ?4))",
    )
    .bind(place.page_id)
    .bind(place.comment_id)
    .bind(&users)
    .bind(place.workspace_id)
    .execute(&mut *conn)
    .await?;
    // A user named anew who was told of this comment as a reply or a page's comment is told of
    // the mention instead: one notification per comment.
    sqlx::query(
        "DELETE FROM notifications WHERE comment_id = ?2 AND kind <> 'mention'
             AND user_id IN (SELECT value FROM json_each(?3))
             AND NOT EXISTS (SELECT 1 FROM mentions x
                             WHERE x.comment_id = ?2 AND x.user_id = notifications.user_id)",
    )
    .bind(place.page_id)
    .bind(place.comment_id)
    .bind(&users)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO notifications (id, user_id, kind, page_id, comment_id, actor_id, created_at)
         SELECT lower(hex(randomblob(16))), m.user_id, 'mention', ?1, ?2, ?5, ?6
         FROM workspace_members m
         WHERE m.workspace_id = ?4 AND m.user_id IN (SELECT value FROM json_each(?3))
           AND m.user_id IS NOT ?5
           AND NOT EXISTS (SELECT 1 FROM mentions x
                           WHERE x.page_id = ?1 AND x.comment_id IS ?2 AND x.user_id = m.user_id)",
    )
    .bind(place.page_id)
    .bind(place.comment_id)
    .bind(&users)
    .bind(place.workspace_id)
    .bind(author)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    sqlx::query(
        "INSERT INTO mentions (id, user_id, page_id, comment_id, author_id, created_at)
         SELECT lower(hex(randomblob(16))), m.user_id, ?1, ?2, ?5, ?6
         FROM workspace_members m
         WHERE m.workspace_id = ?4 AND m.user_id IN (SELECT value FROM json_each(?3))
           AND NOT EXISTS (SELECT 1 FROM mentions x
                           WHERE x.page_id = ?1 AND x.comment_id IS ?2 AND x.user_id = m.user_id)",
    )
    .bind(place.page_id)
    .bind(place.comment_id)
    .bind(&users)
    .bind(place.workspace_id)
    .bind(author)
    .bind(now)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Drops every mention of `user_id` on the pages of a workspace they left.
pub(super) async fn forget_member(
    conn: &mut SqliteConnection,
    workspace_id: &str,
    user_id: &str,
) -> Result<(), StoreError> {
    sqlx::query(
        "DELETE FROM mentions WHERE user_id = ?2
             AND page_id IN (SELECT id FROM pages WHERE workspace_id = ?1)",
    )
    .bind(workspace_id)
    .bind(user_id)
    .execute(&mut *conn)
    .await?;
    Ok(())
}
