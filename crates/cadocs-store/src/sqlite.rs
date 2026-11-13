mod accounts;
mod attachments;
mod backup;
mod jobs;
mod pages;
mod revisions;
mod search;
mod spaces;
mod workspaces;

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use sqlx::Row;
use sqlx::migrate::Migrator;
use sqlx::sqlite::{
    SqliteAutoVacuum, SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions,
    SqliteSynchronous,
};
use tokio::sync::Notify;

use crate::{
    DocEdit, DocStore, PageSite, RevisionTiming, Snapshot, StoreError, StoredDoc, StoredUpdate,
    now_ms,
};

static MIGRATOR: Migrator = sqlx::migrate!("./migrations/sqlite");

/// Each connection has a thread and a page cache, so they are few and closed when idle.
const READER_CONNECTIONS: u32 = 2;

/// Takes the write lock up front. A deferred transaction that reads first would fail at once when
/// another process writes, because upgrading a read does not use the busy timeout.
const BEGIN_WRITE: &str = "BEGIN IMMEDIATE";

/// Bytes the WAL keeps on disk after a checkpoint. A large transaction, such as an import, grows
/// it, and SQLite would otherwise never shrink the file back.
const WAL_SIZE_LIMIT: i64 = 64 * 1024 * 1024;

/// `doc_snapshots.codec` of a state compressed with zstd. `0` is a raw one.
const ZSTD_CODEC: i64 = 1;
/// The level revisions use too: most of the gain, for little CPU in the room's task.
const ZSTD_LEVEL: i32 = 3;

/// How long after an edit its page is indexed: the edits of that time share one run.
const REINDEX_DELAY: Duration = Duration::from_secs(5);

/// SQLite allows one writer at a time: a single connection queues writes in the pool instead of
/// making them wait on `SQLITE_BUSY`, and reads never wait behind them (ADR 0002).
#[derive(Debug, Clone)]
pub struct SqliteStore {
    writer: SqlitePool,
    reader: SqlitePool,
    /// Told of each job a write adds, the reindex of a flush included.
    jobs_added: Arc<Notify>,
    reindex_delay: Duration,
    revision_timing: RevisionTiming,
}

impl SqliteStore {
    /// Opens the database, creating it if needed, and runs the migrations. A database newer than
    /// this binary is refused, as its applied migrations are unknown here.
    pub async fn open(path: &Path) -> Result<Self, StoreError> {
        let writer = writer(
            connect_options(path)
                .create_if_missing(true)
                // Takes effect only on a database with no table yet: see below for the others.
                .auto_vacuum(SqliteAutoVacuum::Incremental)
                .journal_mode(SqliteJournalMode::Wal),
        )
        .await?;
        MIGRATOR.run(&writer).await?;
        incremental_auto_vacuum(&writer).await?;
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
        Self {
            writer,
            reader,
            jobs_added: Arc::default(),
            reindex_delay: REINDEX_DELAY,
            revision_timing: RevisionTiming::default(),
        }
    }

    /// For tests, which cannot wait the default.
    pub fn with_reindex_delay(mut self, delay: Duration) -> Self {
        self.reindex_delay = delay;
        self
    }

    /// When a flush's revision is taken: `CADOCS_REVISION_PAUSE_SECS` and `_MAX_SECS`.
    pub fn with_revision_timing(mut self, timing: RevisionTiming) -> Self {
        self.revision_timing = timing;
        self
    }
}

async fn writer(options: SqliteConnectOptions) -> Result<SqlitePool, sqlx::Error> {
    SqlitePoolOptions::new()
        .max_connections(1)
        .test_before_acquire(false)
        .connect_with(options)
        .await
}

/// A database created before `auto_vacuum` was set keeps `NONE`, and only a `VACUUM` changes
/// that. It rewrites the whole file once, so it runs at startup, where `serve` holds the data
/// folder alone.
async fn incremental_auto_vacuum(writer: &SqlitePool) -> Result<(), StoreError> {
    let mode: i64 = sqlx::query_scalar("PRAGMA auto_vacuum")
        .fetch_one(writer)
        .await?;
    if mode != 2 {
        sqlx::raw_sql("PRAGMA auto_vacuum = INCREMENTAL; VACUUM;")
            .execute(writer)
            .await?;
    }
    Ok(())
}

/// No `mmap_size`, on purpose: mapped pages count in the process RSS, which the RAM targets
/// measure, while the OS page cache gives the same reads uncounted (ADR 0002).
fn connect_options(path: &Path) -> SqliteConnectOptions {
    SqliteConnectOptions::new()
        .filename(path)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true)
        // 512 KiB per connection: the OS page cache does the real caching.
        .pragma("cache_size", "-512")
        .pragma("journal_size_limit", WAL_SIZE_LIMIT.to_string())
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

        let row =
            sqlx::query("SELECT state, upto_seq, codec FROM doc_snapshots WHERE page_id = ?1")
                .bind(doc_id)
                .fetch_optional(&mut *tx)
                .await?;
        let snapshot = match row {
            None => None,
            Some(row) => {
                let stored: Vec<u8> = row.get("state");
                let state = if row.get::<i64, _>("codec") == ZSTD_CODEC {
                    zstd::stream::decode_all(stored.as_slice()).map_err(|_| {
                        StoreError::CorruptSnapshot {
                            doc_id: doc_id.to_owned(),
                        }
                    })?
                } else {
                    stored
                };
                Some(Snapshot {
                    state,
                    upto_seq: row.get("upto_seq"),
                })
            }
        };

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
        let run_at = created_at.saturating_add(self.reindex_delay.as_millis() as i64);
        search::queue_reindex(&mut tx, "p.id = ?1", doc_id, run_at).await?;
        let author = edit.author.as_deref();
        revisions::note_edit(&mut tx, doc_id, author, self.revision_timing, created_at).await?;
        tx.commit().await?;
        // The revision job is always queued or moved, so the worker always has news.
        self.jobs_added.notify_one();
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

        // A Yjs state repeats its structure and client ids, so it shrinks several times.
        let compressed = zstd::bulk::compress(&snapshot.state, ZSTD_LEVEL)
            .map_err(|error| StoreError::Database(sqlx::Error::Encode(Box::new(error))))?;
        sqlx::query(
            "INSERT INTO doc_snapshots (page_id, state, upto_seq, updated_at, codec)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT (page_id) DO UPDATE SET
                 state = excluded.state,
                 upto_seq = excluded.upto_seq,
                 updated_at = excluded.updated_at,
                 codec = excluded.codec",
        )
        .bind(doc_id)
        .bind(&compressed)
        .bind(snapshot.upto_seq)
        .bind(now_ms())
        .bind(ZSTD_CODEC)
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

    use crate::JobStore;

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

    async fn pragma(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar(sqlx::AssertSqlSafe(format!("PRAGMA {name}")))
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn a_new_database_frees_pages_incrementally() {
        let (store, _dir) = open().await;
        assert_eq!(pragma(&store.writer, "auto_vacuum").await, 2);
    }

    #[tokio::test]
    async fn an_older_database_turns_to_incremental_auto_vacuum() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cadocs.db");
        let old = SqlitePoolOptions::new()
            .connect_with(
                SqliteConnectOptions::new()
                    .filename(&path)
                    .create_if_missing(true),
            )
            .await
            .unwrap();
        sqlx::raw_sql("CREATE TABLE older (x)")
            .execute(&old)
            .await
            .unwrap();
        assert_eq!(pragma(&old, "auto_vacuum").await, 0);
        old.close().await;

        let store = SqliteStore::open(&path).await.unwrap();
        assert_eq!(pragma(&store.writer, "auto_vacuum").await, 2);
    }

    #[tokio::test]
    async fn the_wal_is_cut_back_after_a_checkpoint() {
        let (store, _dir) = open().await;
        assert_eq!(
            pragma(&store.writer, "journal_size_limit").await,
            WAL_SIZE_LIMIT
        );
    }

    #[tokio::test]
    async fn maintenance_gives_free_pages_back_to_the_disk() {
        let (store, _dir) = open().await;
        sqlx::raw_sql(
            "CREATE TABLE filler (data BLOB);
             WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 2000)
             INSERT INTO filler SELECT randomblob(4000) FROM n;
             DROP TABLE filler;",
        )
        .execute(&store.writer)
        .await
        .unwrap();
        assert!(pragma(&store.writer, "freelist_count").await > 2000);

        store.maintain_database().await.unwrap();
        assert_eq!(pragma(&store.writer, "freelist_count").await, 0);
    }

    /// A Yjs state that compresses, as real ones do: repeated structure.
    fn snapshot_state() -> Vec<u8> {
        b"<paragraph>cadocs</paragraph>".repeat(200)
    }

    #[tokio::test]
    async fn a_snapshot_is_stored_compressed_and_loads_as_it_was() {
        let (store, _dir) = open().await;
        let state = snapshot_state();
        let snapshot = Snapshot {
            state: state.clone(),
            upto_seq: 0,
        };
        store.compact("doc", &snapshot).await.unwrap();

        let (codec, stored): (i64, Vec<u8>) =
            sqlx::query_as("SELECT codec, state FROM doc_snapshots WHERE page_id = 'doc'")
                .fetch_one(&store.writer)
                .await
                .unwrap();
        assert_eq!(codec, 1);
        assert!(stored.len() * 5 < state.len(), "{} bytes", stored.len());
        assert_eq!(
            store.load("doc").await.unwrap().snapshot.unwrap().state,
            state
        );
    }

    #[tokio::test]
    async fn a_snapshot_stored_raw_before_compression_still_loads() {
        let (store, _dir) = open().await;
        sqlx::query(
            "INSERT INTO doc_snapshots (page_id, state, upto_seq, updated_at)
             VALUES ('doc', ?1, 0, 1)",
        )
        .bind(snapshot_state())
        .execute(&store.writer)
        .await
        .unwrap();
        let loaded = store.load("doc").await.unwrap().snapshot.unwrap();
        assert_eq!(loaded.state, snapshot_state());
    }

    #[tokio::test]
    async fn a_snapshot_that_does_not_decompress_fails_the_load() {
        let (store, _dir) = open().await;
        sqlx::raw_sql(
            "INSERT INTO doc_snapshots (page_id, state, upto_seq, updated_at, codec)
             VALUES ('doc', x'0102', 0, 1, 1);",
        )
        .execute(&store.writer)
        .await
        .unwrap();
        assert!(matches!(
            store.load("doc").await,
            Err(StoreError::CorruptSnapshot { .. })
        ));
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
