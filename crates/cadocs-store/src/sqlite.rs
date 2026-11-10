mod accounts;
mod attachments;
mod jobs;
mod pages;
mod spaces;
mod workspaces;

use std::path::Path;
use std::time::Duration;

use sqlx::Row;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use crate::{DocEdit, DocStore, PageSite, Snapshot, StoreError, StoredDoc, StoredUpdate, now_ms};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");

/// Each connection has a thread and a page cache, so they are few and closed when idle.
const READER_CONNECTIONS: u32 = 2;

/// Takes the write lock up front. A deferred transaction that reads first would fail at once when
/// another process writes, because upgrading a read does not use the busy timeout.
const BEGIN_WRITE: &str = "BEGIN IMMEDIATE";

/// SQLite allows one writer at a time: a single connection queues writes in the pool instead of
/// making them wait on `SQLITE_BUSY`, and reads never wait behind them (ADR 0002).
#[derive(Debug, Clone)]
pub struct SqliteStore {
    writer: SqlitePool,
    reader: SqlitePool,
}

impl SqliteStore {
    /// Opens the database, creating it if needed, and runs the migrations. A database newer than
    /// this binary is refused, as its applied migrations are unknown here.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let writer = writer(
            connect_options(path)
                .create_if_missing(true)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
        MIGRATOR.run(&writer).await?;
        Ok(Self::with_writer(path, writer))
    }

    /// Opens a database as an ordinary client, beside a server that may be running. It creates
    /// nothing and migrates nothing: that is for `serve`, which holds the data folder. So a
    /// schema this binary does not know exactly is an `OtherVersion`.
    pub async fn open_existing(path: &Path) -> Result<Self, StoreError> {
        let writer = writer(connect_options(path)).await?;
        let applied: Option<i64> =
            sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success = 1")
                .fetch_one(&writer)
                .await?;
        if applied != MIGRATOR.iter().map(|migration| migration.version).max() {
            return Err(StoreError::OtherVersion);
        }
        Ok(Self::with_writer(path, writer))
    }

    fn with_writer(path: &Path, writer: SqlitePool) -> Self {
        let reader = SqlitePoolOptions::new()
            .max_connections(READER_CONNECTIONS)
            .test_before_acquire(false)
            .connect_lazy_with(connect_options(path).read_only(true));
        Self { writer, reader }
    }
}

async fn writer(options: SqliteConnectOptions) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(options)
        .await
}

fn connect_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true)
        // 512 KiB per connection: the OS page cache does the real caching.
        .pragma("cache_size", "-512")
}

impl DocStore for SqliteStore {
    async fn ping(&self) -> Result<(), StoreError> {
        // Reads the schema page, so a broken file fails here and not only at the next flush.
        sqlx::query("SELECT count(*) FROM sqlite_master")
            .execute(&self.reader)
            .await?;
        Ok(())
    }

    async fn load(&self, doc_id: &str) -> Result<StoredDoc, StoreError> {
        let mut tx = self.reader.begin().await?;

        let snapshot = sqlx::query("SELECT state, upto_seq FROM doc_snapshots WHERE page_id = ?1")
            .bind(doc_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(|row| Snapshot {
                state: row.get("state"),
                upto_seq: row.get("upto_seq"),
            });

        let after = snapshot.as_ref().map_or(0, |s| s.upto_seq);
        let updates = sqlx::query(
            "SELECT seq, data FROM doc_updates WHERE page_id = ?1 AND seq > ?2 ORDER BY seq",
        )
        .bind(doc_id)
        .bind(after)
        .fetch_all(&mut *tx)
        .await?
        .into_iter()
        .map(|row| StoredUpdate {
            seq: row.get("seq"),
            data: row.get("data"),
        })
        .collect();

        tx.commit().await?;
        Ok(StoredDoc { snapshot, updates })
    }

    async fn append_updates(
        &self,
        doc_id: &str,
        updates: &[StoredUpdate],
        edit: &DocEdit,
    ) -> Result<Option<PageSite>, StoreError> {
        if updates.is_empty() {
            return Ok(None);
        }

        let created_at = now_ms();
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        // Most flushes keep the title, and only a new one is worth telling anyone.
        let renamed =
            sqlx::query("SELECT workspace_id, space_id FROM pages WHERE id = ?1 AND title <> ?2")
                .bind(doc_id)
                .bind(&edit.title)
                .fetch_optional(&mut *tx)
                .await?
                .map(|row| PageSite {
                    workspace_id: row.get("workspace_id"),
                    space_id: row.get("space_id"),
                });
        // Reading the author from `users`: a foreign key error here would fail every retry.
        let page = sqlx::query(
            "UPDATE pages SET
                 title = coalesce(?2, title),
                 updated_by = coalesce((SELECT id FROM users WHERE id = ?3), updated_by),
                 updated_at = ?4
             WHERE id = ?1",
        )
        .bind(doc_id)
        .bind(&edit.title)
        .bind(&edit.author)
        .bind(created_at)
        .execute(&mut *tx)
        .await?;
        if page.rows_affected() == 0 {
            return Err(StoreError::PageNotFound);
        }

        let covered: i64 =
            sqlx::query_scalar("SELECT upto_seq FROM doc_snapshots WHERE page_id = ?1")
                .bind(doc_id)
                .fetch_optional(&mut *tx)
                .await?
                .unwrap_or(0);

        for update in updates {
            let conflict = || StoreError::SeqConflict {
                doc_id: doc_id.to_owned(),
                seq: update.seq,
            };
            if update.seq < 1 || update.seq <= covered {
                return Err(conflict());
            }

            let inserted = sqlx::query(
                "INSERT INTO doc_updates (page_id, seq, data, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (page_id, seq) DO NOTHING",
            )
            .bind(doc_id)
            .bind(update.seq)
            .bind(&update.data)
            .bind(created_at)
            .execute(&mut *tx)
            .await?
            .rows_affected();

            if inserted == 0 {
                let stored: Vec<u8> = sqlx::query_scalar(
                    "SELECT data FROM doc_updates WHERE page_id = ?1 AND seq = ?2",
                )
                .bind(doc_id)
                .bind(update.seq)
                .fetch_one(&mut *tx)
                .await?;
                if stored != update.data {
                    return Err(conflict());
                }
            }
        }
        tx.commit().await?;
        Ok(renamed)
    }

    async fn compact(&self, doc_id: &str, snapshot: &Snapshot) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;

        let page_exists: bool =
            sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM pages WHERE id = ?1)")
                .bind(doc_id)
                .fetch_one(&mut *tx)
                .await?;
        if !page_exists {
            return Err(StoreError::PageNotFound);
        }

        let stored: Option<i64> =
            sqlx::query_scalar("SELECT upto_seq FROM doc_snapshots WHERE page_id = ?1")
                .bind(doc_id)
                .fetch_optional(&mut *tx)
                .await?;
        if let Some(stored) = stored.filter(|stored| *stored > snapshot.upto_seq) {
            return Err(StoreError::StaleSnapshot {
                doc_id: doc_id.to_owned(),
                stored,
                offered: snapshot.upto_seq,
            });
        }

        sqlx::query(
            "INSERT INTO doc_snapshots (page_id, state, upto_seq, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (page_id) DO UPDATE SET
                 state = excluded.state,
                 upto_seq = excluded.upto_seq,
                 updated_at = excluded.updated_at",
        )
        .bind(doc_id)
        .bind(&snapshot.state)
        .bind(snapshot.upto_seq)
        .bind(now_ms())
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM doc_updates WHERE page_id = ?1 AND seq <= ?2")
            .bind(doc_id)
            .bind(snapshot.upto_seq)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use tokio::time::timeout;

    use super::*;

    const SHORT: Duration = Duration::from_millis(200);

    async fn open() -> (SqliteStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("cadocs.db"))
            .await
            .unwrap();
        sqlx::raw_sql(
            "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
                 VALUES ('ws', 'ws', 'Workspace', 1, 1);
             INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
                 VALUES ('space', 'ws', 'space', 'Space', 1, 1);
             INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
                 VALUES ('doc', 'ws', 'space', 'a', 'doc', 1, 1);",
        )
        .execute(&store.writer)
        .await
        .unwrap();
        (store, dir)
    }

    fn update() -> StoredUpdate {
        StoredUpdate {
            seq: 1,
            data: vec![1],
        }
    }

    #[tokio::test]
    async fn writes_queue_on_one_connection_and_reads_do_not_wait() {
        let (store, _dir) = open().await;
        let held = store.writer.acquire().await.unwrap();

        let updates = [update()];
        assert!(
            timeout(
                SHORT,
                store.append_updates("doc", &updates, &DocEdit::default())
            )
            .await
            .is_err()
        );
        timeout(SHORT, store.load("doc"))
            .await
            .expect("a read must not wait for the writer")
            .unwrap();

        drop(held);
        store
            .append_updates("doc", &updates, &DocEdit::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn the_read_pool_cannot_write() {
        let (store, _dir) = open().await;
        let refused = sqlx::query("DELETE FROM doc_updates")
            .execute(&store.reader)
            .await;
        assert!(refused.is_err());
    }
}
