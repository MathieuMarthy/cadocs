//! The database side of `cadocs backup` and `cadocs restore` (`docs/OPERATIONS.md`).

use std::path::Path;

use super::{MIGRATOR, SqliteStore, connect_options, writer};
use crate::StoreError;

impl SqliteStore {
    /// Writes a consistent copy of the database at `path` to `target`, which must not exist, with
    /// `VACUUM INTO`. It opens the database as an ordinary client, so a server may be running: the
    /// copy reads one snapshot, the WAL included, and never blocks the server's writes.
    pub async fn write_backup(path: &Path, target: &Path) -> Result<(), StoreError> {
        let target = target.to_str().ok_or(StoreError::NotUtf8Path)?;
        let db = writer(connect_options(path)).await?;
        let copied = sqlx::query("VACUUM INTO ?").bind(target).execute(&db).await;
        db.close().await;
        copied?;
        Ok(())
    }

    /// Checks a database before it is restored: intact, and not newer than this binary. An older
    /// one is fine, as `serve` migrates it at its next start.
    pub async fn check_backup(path: &Path) -> Result<(), StoreError> {
        let db = writer(connect_options(path)).await?;
        let checked = check(&db).await;
        // Closing the last connection checkpoints and removes any WAL the check left.
        db.close().await;
        checked
    }
}

async fn check(db: &sqlx::SqlitePool) -> Result<(), StoreError> {
    let verdict: String = sqlx::query_scalar("PRAGMA quick_check(1)")
        .fetch_one(db)
        .await?;
    if verdict != "ok" {
        return Err(StoreError::Damaged(verdict));
    }
    let applied: Option<i64> =
        sqlx::query_scalar("SELECT max(version) FROM _sqlx_migrations WHERE success = 1")
            .fetch_one(db)
            .await?;
    let known = MIGRATOR.iter().map(|migration| migration.version).max();
    if applied > known {
        return Err(StoreError::NewerVersion);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use sqlx::SqlitePool;
    use tempfile::TempDir;

    use super::*;

    /// A database with one workspace in it, its writer left open as a running server's is.
    async fn live() -> (TempDir, SqliteStore) {
        let dir = tempfile::tempdir().unwrap();
        let store = SqliteStore::open(&dir.path().join("cadocs.db"))
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES ('ws', 'ws', 'Workspace', 1, 1)",
        )
        .execute(&store.writer)
        .await
        .unwrap();
        (dir, store)
    }

    async fn plain(path: &Path) -> SqlitePool {
        writer(connect_options(path)).await.unwrap()
    }

    #[tokio::test]
    async fn a_backup_of_a_live_database_holds_what_its_wal_holds() {
        let (dir, _store) = live().await;
        assert!(
            dir.path().join("cadocs.db-wal").exists(),
            "the row is in the WAL"
        );
        let target = dir.path().join("copy.db");

        SqliteStore::write_backup(&dir.path().join("cadocs.db"), &target)
            .await
            .unwrap();

        let copy = plain(&target).await;
        let name: String = sqlx::query_scalar("SELECT name FROM workspaces")
            .fetch_one(&copy)
            .await
            .unwrap();
        assert_eq!(name, "Workspace");
        SqliteStore::check_backup(&target).await.unwrap();
    }

    #[tokio::test]
    async fn a_backup_neither_makes_a_database_nor_overwrites_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("cadocs.db");
        let target = dir.path().join("copy.db");
        SqliteStore::write_backup(&missing, &target)
            .await
            .unwrap_err();
        assert!(!missing.exists());

        let (dir, _store) = live().await;
        std::fs::write(&target, b"keep me").unwrap();
        SqliteStore::write_backup(&dir.path().join("cadocs.db"), &target)
            .await
            .unwrap_err();
        assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
    }

    #[tokio::test]
    async fn a_newer_database_is_refused_and_an_older_one_is_not() {
        let (dir, store) = live().await;
        let path = dir.path().join("cadocs.db");
        sqlx::query("DELETE FROM _sqlx_migrations WHERE version = (SELECT max(version) FROM _sqlx_migrations)")
            .execute(&store.writer)
            .await
            .unwrap();
        SqliteStore::check_backup(&path).await.unwrap();

        sqlx::query(
            "INSERT INTO _sqlx_migrations
                 (version, description, success, checksum, execution_time)
             VALUES (99999999999999, 'from the future', 1, x'00', 0)",
        )
        .execute(&store.writer)
        .await
        .unwrap();
        let error = SqliteStore::check_backup(&path).await.unwrap_err();
        assert!(matches!(error, StoreError::NewerVersion), "{error}");
    }

    #[tokio::test]
    async fn a_damaged_file_is_refused() {
        let (dir, store) = live().await;
        let path = dir.path().join("cadocs.db");
        let target = dir.path().join("copy.db");
        SqliteStore::write_backup(&path, &target).await.unwrap();
        drop(store);

        // Zeroes every page past the first: the schema still reads, the tables do not.
        let mut bytes = std::fs::read(&target).unwrap();
        let page_size = u16::from_be_bytes([bytes[16], bytes[17]]) as usize;
        bytes[page_size..].fill(0);
        std::fs::write(&target, bytes).unwrap();

        SqliteStore::check_backup(&target).await.unwrap_err();
    }

    #[tokio::test]
    async fn a_file_that_is_no_database_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cadocs.db");
        std::fs::write(&path, b"not a database at all, just some text").unwrap();
        SqliteStore::check_backup(&path).await.unwrap_err();
    }
}
