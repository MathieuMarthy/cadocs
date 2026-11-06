use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::Row;
use sqlx::sqlite::{
    SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions, SqliteSynchronous,
};

use crate::{DocStore, Snapshot, StoreError, StoredDoc, StoredUpdate};

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
        let writer = SqlitePoolOptions::new()
            .max_connections(1)
            .test_before_acquire(false)
            .connect_with(
                connect_options(path)
                    .create_if_missing(true)
                    .journal_mode(SqliteJournalMode::Wal),
            )
            .await?;
        sqlx::migrate!("./migrations/sqlite").run(&writer).await?;

        let reader = SqlitePoolOptions::new()
            .max_connections(READER_CONNECTIONS)
            .test_before_acquire(false)
            .connect_lazy_with(connect_options(path).read_only(true));

        Ok(Self { writer, reader })
    }
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

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as i64)
}

impl DocStore for SqliteStore {
    async fn load(&self, doc_id: &str) -> Result<StoredDoc, StoreError> {
        let mut tx = self.reader.begin().await?;

        let snapshot = sqlx::query("SELECT state, upto_seq FROM doc_snapshots WHERE doc_id = ?1")
            .bind(doc_id)
            .fetch_optional(&mut *tx)
            .await?
            .map(|row| Snapshot {
                state: row.get("state"),
                upto_seq: row.get("upto_seq"),
            });

        let after = snapshot.as_ref().map_or(0, |s| s.upto_seq);
        let updates = sqlx::query(
            "SELECT seq, data FROM doc_updates WHERE doc_id = ?1 AND seq > ?2 ORDER BY seq",
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
    ) -> Result<(), StoreError> {
        if updates.is_empty() {
            return Ok(());
        }

        let created_at = now_ms();
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;
        let covered: i64 =
            sqlx::query_scalar("SELECT upto_seq FROM doc_snapshots WHERE doc_id = ?1")
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
                "INSERT INTO doc_updates (doc_id, seq, data, created_at)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (doc_id, seq) DO NOTHING",
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
                    "SELECT data FROM doc_updates WHERE doc_id = ?1 AND seq = ?2",
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
        Ok(())
    }

    async fn compact(&self, doc_id: &str, snapshot: &Snapshot) -> Result<(), StoreError> {
        let mut tx = self.writer.begin_with(BEGIN_WRITE).await?;

        let stored: Option<i64> =
            sqlx::query_scalar("SELECT upto_seq FROM doc_snapshots WHERE doc_id = ?1")
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
            "INSERT INTO doc_snapshots (doc_id, state, upto_seq, updated_at)
             VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (doc_id) DO UPDATE SET
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

        sqlx::query("DELETE FROM doc_updates WHERE doc_id = ?1 AND seq <= ?2")
            .bind(doc_id)
            .bind(snapshot.upto_seq)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }
}
