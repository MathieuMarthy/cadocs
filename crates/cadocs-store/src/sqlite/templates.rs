use cadocs_core::permissions::SpaceAction;
use sqlx::sqlite::SqliteRow;
use sqlx::{Row, SqliteConnection};

use super::spaces::authorize;
use super::{BEGIN_WRITE, SqliteStore};
use crate::{MAX_TEMPLATES, NewTemplate, StoreError, Template, TemplateStore, TemplateToUse};

const COLUMNS: &str = "t.id, t.space_id, t.title, t.created_by, t.created_at";

fn template(row: &SqliteRow) -> Template {
    Template {
        id: row.get("id"),
        space_id: row.get("space_id"),
        title: row.get("title"),
        created_by: row.get("created_by"),
        created_at: row.get("created_at"),
    }
}

/// The space a template sits in, `TemplateNotFound` for one that is gone.
async fn space_of(conn: &mut SqliteConnection, template_id: &str) -> Result<String, StoreError> {
    sqlx::query_scalar("SELECT space_id FROM templates WHERE id = ?1")
        .bind(template_id)
        .fetch_optional(conn)
        .await?
        .ok_or(StoreError::TemplateNotFound)
}

/// What a space the user cannot read hides: it is as absent as the template.
fn hidden(error: StoreError) -> StoreError {
    match error {
        StoreError::SpaceNotFound => StoreError::TemplateNotFound,
        error => error,
    }
}

impl TemplateStore for SqliteStore {
    async fn create_template(&self, new: &NewTemplate) -> Result<Template, StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        authorize(&mut tx, &new.space_id, &new.actor, SpaceAction::EditPage).await?;
        let held: i64 = sqlx::query_scalar("SELECT count(*) FROM templates WHERE space_id = ?1")
            .bind(&new.space_id)
            .fetch_one(&mut *tx)
            .await?;
        if held >= MAX_TEMPLATES {
            return Err(StoreError::TooManyTemplates);
        }
        sqlx::query(
            "INSERT INTO templates (id, space_id, title, state, created_by, created_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        )
        .bind(&new.id)
        .bind(&new.space_id)
        .bind(&new.title)
        .bind(&new.state)
        .bind(&new.actor)
        .bind(new.now)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(Template {
            id: new.id.clone(),
            space_id: new.space_id.clone(),
            title: new.title.clone(),
            created_by: Some(new.actor.clone()),
            created_at: new.now,
        })
    }

    async fn list_templates(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> Result<Vec<Template>, StoreError> {
        let mut reader = self.reader.acquire().await?;
        authorize(&mut reader, space_id, actor, SpaceAction::ReadPage).await?;
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS} FROM templates t
             WHERE t.space_id = ?1 AND t.id > coalesce(?2, '') ORDER BY t.id LIMIT ?3"
        )))
        .bind(space_id)
        .bind(after)
        .bind(limit)
        .fetch_all(&mut *reader)
        .await?;
        Ok(rows.iter().map(template).collect())
    }

    async fn template_to_use(
        &self,
        template_id: &str,
        target: Option<&str>,
        actor: &str,
    ) -> Result<TemplateToUse, StoreError> {
        let mut reader = self.reader.acquire().await?;
        let space_id = space_of(&mut reader, template_id).await?;
        let source = authorize(&mut reader, &space_id, actor, SpaceAction::ReadPage)
            .await
            .map_err(hidden)?;
        if let Some(target) = target.filter(|target| *target != space_id) {
            let row = authorize(&mut reader, target, actor, SpaceAction::EditPage).await?;
            let (from, to): (String, String) =
                (source.get("workspace_id"), row.get("workspace_id"));
            if from != to {
                return Err(StoreError::OtherWorkspace);
            }
        } else {
            authorize(&mut reader, &space_id, actor, SpaceAction::EditPage).await?;
        }
        let row = sqlx::query(sqlx::AssertSqlSafe(format!(
            "SELECT {COLUMNS}, t.state FROM templates t WHERE t.id = ?1"
        )))
        .bind(template_id)
        .fetch_optional(&mut *reader)
        .await?
        .ok_or(StoreError::TemplateNotFound)?;
        Ok(TemplateToUse {
            template: template(&row),
            state: row.get("state"),
        })
    }

    async fn delete_template(&self, template_id: &str, actor: &str) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let space_id = space_of(&mut tx, template_id).await?;
        authorize(&mut tx, &space_id, actor, SpaceAction::EditPage)
            .await
            .map_err(hidden)?;
        sqlx::query("DELETE FROM templates WHERE id = ?1")
            .bind(template_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }
}
