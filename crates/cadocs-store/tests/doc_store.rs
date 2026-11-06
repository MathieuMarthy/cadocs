use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cadocs_store::{DocStore, Snapshot, SqliteStore, StoreError, StoredDoc, StoredUpdate};
use sqlx::Connection;
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use tempfile::TempDir;

const DOC: &str = "doc";

async fn open() -> (SqliteStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap();
    (store, dir)
}

async fn raw_connection(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap()
}

fn update(seq: i64) -> StoredUpdate {
    StoredUpdate {
        seq,
        data: vec![seq as u8; 3],
    }
}

fn updates(seqs: impl IntoIterator<Item = i64>) -> Vec<StoredUpdate> {
    seqs.into_iter().map(update).collect()
}

fn snapshot(upto_seq: i64) -> Snapshot {
    Snapshot {
        state: vec![0xAB, upto_seq as u8],
        upto_seq,
    }
}

#[tokio::test]
async fn unknown_document_is_empty() {
    let (store, _dir) = open().await;

    let doc = store.load(DOC).await.unwrap();

    assert_eq!(doc, StoredDoc::default());
    assert_eq!(doc.last_seq(), 0);
}

#[tokio::test]
async fn updates_come_back_in_seq_order() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates([3, 1])).await.unwrap();
    store.append_updates(DOC, &updates([2])).await.unwrap();

    let doc = store.load(DOC).await.unwrap();

    assert_eq!(doc.snapshot, None);
    assert_eq!(doc.updates, updates([1, 2, 3]));
    assert_eq!(doc.last_seq(), 3);
}

#[tokio::test]
async fn empty_batch_is_a_no_op() {
    let (store, _dir) = open().await;

    store.append_updates(DOC, &[]).await.unwrap();

    assert_eq!(store.load(DOC).await.unwrap(), StoredDoc::default());
}

#[tokio::test]
async fn documents_are_isolated() {
    let (store, _dir) = open().await;
    store.append_updates("a", &updates([1])).await.unwrap();
    store.append_updates("b", &updates([1, 2])).await.unwrap();
    store.compact("b", &snapshot(2)).await.unwrap();

    let a = store.load("a").await.unwrap();

    assert_eq!(a.snapshot, None);
    assert_eq!(a.updates, updates([1]));
}

#[tokio::test]
async fn a_failing_batch_stores_nothing() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates([2])).await.unwrap();
    let other = StoredUpdate {
        seq: 2,
        data: vec![0xFF],
    };

    let result = store
        .append_updates(DOC, &[update(1), other, update(3)])
        .await;

    assert!(matches!(
        result,
        Err(StoreError::SeqConflict { seq: 2, .. })
    ));
    assert_eq!(store.load(DOC).await.unwrap().updates, updates([2]));
}

#[tokio::test]
async fn replaying_a_stored_batch_succeeds() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates(1..=3)).await.unwrap();

    store.append_updates(DOC, &updates(1..=4)).await.unwrap();

    assert_eq!(store.load(DOC).await.unwrap().updates, updates(1..=4));
}

#[tokio::test]
async fn a_seq_covered_by_the_snapshot_is_refused() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates(1..=3)).await.unwrap();
    store.compact(DOC, &snapshot(3)).await.unwrap();

    let result = store.append_updates(DOC, &updates([2, 4])).await;

    assert!(matches!(
        result,
        Err(StoreError::SeqConflict { seq: 2, .. })
    ));
    assert!(store.load(DOC).await.unwrap().updates.is_empty());
}

#[tokio::test]
async fn a_seq_below_one_is_refused() {
    let (store, _dir) = open().await;

    let result = store.append_updates(DOC, &updates([0])).await;

    assert!(matches!(
        result,
        Err(StoreError::SeqConflict { seq: 0, .. })
    ));
}

/// The CLI writes beside a live server (OPERATIONS.md). A transaction that reads before it writes
/// fails at once in that case, without using the busy timeout.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn writes_wait_for_another_process() {
    let (store, dir) = open().await;
    store.append_updates(DOC, &updates(1..=3)).await.unwrap();
    let mut other = raw_connection(&dir.path().join("cadocs.db")).await;
    sqlx::query("BEGIN IMMEDIATE")
        .execute(&mut other)
        .await
        .unwrap();
    sqlx::query("INSERT INTO doc_updates VALUES ('other', 1, x'00', 0)")
        .execute(&mut other)
        .await
        .unwrap();

    let writes = tokio::spawn({
        let store = store.clone();
        async move {
            store.compact(DOC, &snapshot(2)).await?;
            store.append_updates(DOC, &updates([4])).await
        }
    });
    tokio::time::sleep(Duration::from_millis(200)).await;
    sqlx::query("COMMIT").execute(&mut other).await.unwrap();

    writes.await.unwrap().unwrap();
    assert_eq!(store.load(DOC).await.unwrap().updates, updates([3, 4]));
}

#[tokio::test]
async fn compaction_keeps_later_updates() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates(1..=5)).await.unwrap();

    store.compact(DOC, &snapshot(3)).await.unwrap();

    let doc = store.load(DOC).await.unwrap();
    assert_eq!(doc.snapshot, Some(snapshot(3)));
    assert_eq!(doc.updates, updates([4, 5]));
    assert_eq!(doc.last_seq(), 5);
}

#[tokio::test]
async fn seq_keeps_growing_after_a_full_compaction() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates(1..=5)).await.unwrap();

    store.compact(DOC, &snapshot(5)).await.unwrap();

    let doc = store.load(DOC).await.unwrap();
    assert!(doc.updates.is_empty());
    assert_eq!(doc.last_seq(), 5);
}

#[tokio::test]
async fn a_newer_snapshot_replaces_the_stored_one() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates(1..=4)).await.unwrap();
    store.compact(DOC, &snapshot(2)).await.unwrap();

    store.compact(DOC, &snapshot(4)).await.unwrap();

    let doc = store.load(DOC).await.unwrap();
    assert_eq!(doc.snapshot, Some(snapshot(4)));
    assert!(doc.updates.is_empty());
}

#[tokio::test]
async fn an_older_snapshot_is_refused_and_changes_nothing() {
    let (store, _dir) = open().await;
    store.append_updates(DOC, &updates(1..=4)).await.unwrap();
    store.compact(DOC, &snapshot(3)).await.unwrap();

    let result = store.compact(DOC, &snapshot(2)).await;

    assert!(matches!(
        result,
        Err(StoreError::StaleSnapshot {
            stored: 3,
            offered: 2,
            ..
        })
    ));
    let doc = store.load(DOC).await.unwrap();
    assert_eq!(doc.snapshot, Some(snapshot(3)));
    assert_eq!(doc.updates, updates([4]));
}

#[tokio::test]
async fn data_survives_reopening() {
    let (store, dir) = open().await;
    store.append_updates(DOC, &updates(1..=3)).await.unwrap();
    store.compact(DOC, &snapshot(2)).await.unwrap();
    drop(store);

    let store = SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap();

    let doc = store.load(DOC).await.unwrap();
    assert_eq!(doc.snapshot, Some(snapshot(2)));
    assert_eq!(doc.updates, updates([3]));
}

#[tokio::test]
async fn database_uses_the_configured_journal() {
    let (store, dir) = open().await;
    store.append_updates(DOC, &updates([1])).await.unwrap();

    let mut connection = raw_connection(&dir.path().join("cadocs.db")).await;
    let mode: String = sqlx::query_scalar("PRAGMA journal_mode")
        .fetch_one(&mut connection)
        .await
        .unwrap();

    assert_eq!(mode, "wal");
}

#[tokio::test]
async fn database_newer_than_the_binary_is_refused() {
    let (store, dir) = open().await;
    drop(store);
    let path = dir.path().join("cadocs.db");
    let mut connection = raw_connection(&path).await;
    sqlx::query(
        "INSERT INTO _sqlx_migrations (version, description, success, checksum, execution_time)
         VALUES (99999, 'from the future', 1, x'00', 0)",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();

    let result = SqliteStore::open(&path).await;

    assert!(matches!(result, Err(StoreError::Migration(_))));
}

/// The state rebuilt by a load must never miss a `seq`, whatever compaction does meanwhile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn load_sees_a_complete_document_during_compaction() {
    let (store, _dir) = open().await;
    let store = Arc::new(store);
    let done = Arc::new(AtomicBool::new(false));

    let writer = tokio::spawn({
        let (store, done) = (store.clone(), done.clone());
        async move {
            for seq in 1..=2000 {
                store.append_updates(DOC, &updates([seq])).await.unwrap();
                // Updates stay after the snapshot: a load that mixes two states shows a gap.
                if seq % 5 == 0 {
                    store.compact(DOC, &snapshot(seq - 2)).await.unwrap();
                }
            }
            done.store(true, Ordering::Release);
        }
    });

    let mut loads = 0;
    while !done.load(Ordering::Acquire) {
        let doc = store.load(DOC).await.unwrap();
        let first = doc.snapshot.as_ref().map_or(0, |s| s.upto_seq) + 1;
        let seqs: Vec<i64> = doc.updates.iter().map(|u| u.seq).collect();
        let expected: Vec<i64> = (first..first + seqs.len() as i64).collect();
        assert_eq!(seqs, expected);
        loads += 1;
    }

    writer.await.unwrap();
    assert!(loads > 0);
}
