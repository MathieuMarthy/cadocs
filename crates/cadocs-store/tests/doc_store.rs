use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use cadocs_store::{
    DocEdit, DocStore, MAX_TITLE_CHARS, PageStore, Snapshot, SqliteStore, StoreError, StoredDoc,
    StoredUpdate,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Row};
use tempfile::TempDir;

const DOC: &str = "doc";
const NO_EDIT: DocEdit = DocEdit {
    title: None,
    author: None,
};

/// A database with the pages the tests write to: a document needs its row in `pages`.
async fn open() -> (SqliteStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cadocs.db");
    let store = SqliteStore::open(&path).await.unwrap();
    let mut connection = raw_connection(&path).await;
    sqlx::raw_sql(
        "INSERT INTO users (id, email, name, created_at, updated_at)
             VALUES ('ada', 'ada@example.org', 'Ada', 1, 1), ('bob', 'bob@example.org', 'Bob', 1, 1);
         INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES ('ws', 'ws', 'Workspace', 1, 1);
         INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             VALUES ('ws', 'ada', 'owner', 1);
         INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
             VALUES ('space', 'ws', 'space', 'Space', 1, 1);
         INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
             VALUES ('doc', 'ws', 'space', 'a', 'doc', 1, 1),
                    ('a', 'ws', 'space', 'b', 'a', 1, 1),
                    ('b', 'ws', 'space', 'c', 'b', 1, 1),
                    ('other', 'ws', 'space', 'd', 'other', 1, 1);",
    )
    .execute(&mut connection)
    .await
    .unwrap();
    connection.close().await.unwrap();
    (store, dir)
}

/// The columns of `pages` that a flush writes.
async fn page_columns(dir: &TempDir) -> (String, Option<String>, i64) {
    let mut connection = raw_connection(&dir.path().join("cadocs.db")).await;
    let row = sqlx::query("SELECT title, updated_by, updated_at FROM pages WHERE id = 'doc'")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    (row.get(0), row.get(1), row.get(2))
}

fn edit(title: &str, author: &str) -> DocEdit {
    DocEdit {
        title: Some(title.to_owned()),
        author: Some(author.to_owned()),
    }
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
    store
        .append_updates(DOC, &updates([3, 1]), &NO_EDIT)
        .await
        .unwrap();
    store
        .append_updates(DOC, &updates([2]), &NO_EDIT)
        .await
        .unwrap();

    let doc = store.load(DOC).await.unwrap();

    assert_eq!(doc.snapshot, None);
    assert_eq!(doc.updates, updates([1, 2, 3]));
    assert_eq!(doc.last_seq(), 3);
}

#[tokio::test]
async fn empty_batch_is_a_no_op() {
    let (store, _dir) = open().await;

    store.append_updates(DOC, &[], &NO_EDIT).await.unwrap();

    assert_eq!(store.load(DOC).await.unwrap(), StoredDoc::default());
}

#[tokio::test]
async fn documents_are_isolated() {
    let (store, _dir) = open().await;
    store
        .append_updates("a", &updates([1]), &NO_EDIT)
        .await
        .unwrap();
    store
        .append_updates("b", &updates([1, 2]), &NO_EDIT)
        .await
        .unwrap();
    store.compact("b", &snapshot(2)).await.unwrap();

    let a = store.load("a").await.unwrap();

    assert_eq!(a.snapshot, None);
    assert_eq!(a.updates, updates([1]));
}

#[tokio::test]
async fn a_failing_batch_stores_nothing() {
    let (store, _dir) = open().await;
    store
        .append_updates(DOC, &updates([2]), &NO_EDIT)
        .await
        .unwrap();
    let other = StoredUpdate {
        seq: 2,
        data: vec![0xFF],
    };

    let result = store
        .append_updates(DOC, &[update(1), other, update(3)], &NO_EDIT)
        .await;

    assert!(matches!(
        result,
        Err(StoreError::SeqConflict { seq: 2, .. })
    ));
    assert_eq!(store.load(DOC).await.unwrap().updates, updates([2]));
}

#[tokio::test]
async fn a_flush_copies_the_title_and_who_wrote() {
    let (store, dir) = open().await;

    store
        .append_updates(DOC, &updates([1]), &edit("Plans", "ada"))
        .await
        .unwrap();

    let (title, updated_by, updated_at) = page_columns(&dir).await;
    assert_eq!(title, "Plans");
    assert_eq!(updated_by.as_deref(), Some("ada"));
    assert!(updated_at > 1);
}

#[tokio::test]
async fn a_flush_without_a_title_or_author_keeps_them() {
    let (store, dir) = open().await;
    store
        .append_updates(DOC, &updates([1]), &edit("Plans", "ada"))
        .await
        .unwrap();

    store
        .append_updates(DOC, &updates([2]), &NO_EDIT)
        .await
        .unwrap();

    let (title, updated_by, _) = page_columns(&dir).await;
    assert_eq!(title, "Plans");
    assert_eq!(updated_by.as_deref(), Some("ada"));
}

#[tokio::test]
async fn an_empty_batch_writes_no_title() {
    let (store, dir) = open().await;

    store
        .append_updates(DOC, &[], &edit("Plans", "ada"))
        .await
        .unwrap();

    assert_eq!(page_columns(&dir).await.0, "");
}

#[tokio::test]
async fn a_refused_batch_writes_no_title() {
    let (store, dir) = open().await;

    let result = store
        .append_updates(DOC, &updates([0]), &edit("Plans", "ada"))
        .await;

    assert!(result.is_err());
    assert_eq!(page_columns(&dir).await.0, "");
}

/// The room may hold the id of an account deleted since: the edits must still reach the disk.
#[tokio::test]
async fn an_author_who_left_does_not_block_a_flush() {
    let (store, dir) = open().await;

    store
        .append_updates(DOC, &updates([1]), &edit("Plans", "gone"))
        .await
        .unwrap();

    let (title, updated_by, _) = page_columns(&dir).await;
    assert_eq!(title, "Plans");
    assert_eq!(updated_by, None);
    assert_eq!(store.load(DOC).await.unwrap().updates, updates([1]));
}

#[tokio::test]
async fn a_document_without_a_page_is_refused() {
    let (store, _dir) = open().await;

    let appended = store.append_updates("gone", &updates([1]), &NO_EDIT).await;
    let compacted = store.compact("gone", &snapshot(1)).await;

    assert!(matches!(appended, Err(StoreError::PageNotFound)));
    assert!(matches!(compacted, Err(StoreError::PageNotFound)));
}

#[tokio::test]
async fn purging_a_page_deletes_its_document() {
    let (store, dir) = open().await;
    store
        .append_updates(DOC, &updates(1..=3), &NO_EDIT)
        .await
        .unwrap();
    store.compact(DOC, &snapshot(2)).await.unwrap();

    store.trash_page(DOC, "ada", 2).await.unwrap();
    store.purge_page(DOC, "ada").await.unwrap();

    assert_eq!(store.load(DOC).await.unwrap(), StoredDoc::default());
    let mut connection = raw_connection(&dir.path().join("cadocs.db")).await;
    let rows: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM doc_updates) + (SELECT count(*) FROM doc_snapshots)",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(rows, 0);
}

#[tokio::test]
async fn only_an_editor_gets_a_page_to_edit() {
    let (store, _dir) = open().await;

    assert_eq!(store.page_to_edit(DOC, "ada").await.unwrap().id, DOC);
    assert!(matches!(
        store.page_to_edit(DOC, "bob").await,
        Err(StoreError::PageNotFound)
    ));
    store.trash_page(DOC, "ada", 2).await.unwrap();
    assert!(matches!(
        store.page_to_edit(DOC, "ada").await,
        Err(StoreError::PageNotFound)
    ));
}

#[test]
fn a_title_keeps_its_first_characters() {
    assert_eq!(DocEdit::clip_title("short".to_owned()), "short");
    let long = "é".repeat(MAX_TITLE_CHARS + 1);
    let clipped = DocEdit::clip_title(long);
    assert_eq!(clipped.chars().count(), MAX_TITLE_CHARS);
    assert!(clipped.chars().all(|letter| letter == 'é'));
}

/// The migration runs on databases that have a prototype's documents, which have no page.
#[tokio::test]
async fn the_migration_keeps_the_documents_of_pages_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut connection = SqliteConnection::connect_with(
        &SqliteConnectOptions::new()
            .filename(dir.path().join("old.db"))
            .create_if_missing(true),
    )
    .await
    .unwrap();
    for script in [
        include_str!("../migrations/sqlite/0001_doc_log.sql"),
        include_str!("../migrations/sqlite/0002_accounts.sql"),
        include_str!("../migrations/sqlite/0003_password_resets.sql"),
        include_str!("../migrations/sqlite/0004_workspace_invitations.sql"),
        include_str!("../migrations/sqlite/0005_spaces.sql"),
        include_str!("../migrations/sqlite/0006_pages.sql"),
        "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES ('ws', 'ws', 'Workspace', 1, 1);
         INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
             VALUES ('space', 'ws', 'space', 'Space', 1, 1);
         INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
             VALUES ('page', 'ws', 'space', 'a', 'page', 1, 1);
         INSERT INTO doc_updates VALUES ('page', 2, x'01', 1), ('prototype', 1, x'02', 1);
         INSERT INTO doc_snapshots VALUES ('page', x'03', 1, 1), ('prototype', x'04', 0, 1);",
        include_str!("../migrations/sqlite/0007_doc_pages.sql"),
    ] {
        sqlx::raw_sql(script)
            .execute(&mut connection)
            .await
            .unwrap();
    }

    let pages: Vec<(String, String)> = sqlx::query_as(
        "SELECT page_id, 'update' FROM doc_updates UNION ALL
         SELECT page_id, 'snapshot' FROM doc_snapshots ORDER BY 2",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        pages,
        [
            ("page".to_owned(), "snapshot".to_owned()),
            ("page".to_owned(), "update".to_owned())
        ]
    );
}

#[tokio::test]
async fn replaying_a_stored_batch_succeeds() {
    let (store, _dir) = open().await;
    store
        .append_updates(DOC, &updates(1..=3), &NO_EDIT)
        .await
        .unwrap();

    store
        .append_updates(DOC, &updates(1..=4), &NO_EDIT)
        .await
        .unwrap();

    assert_eq!(store.load(DOC).await.unwrap().updates, updates(1..=4));
}

#[tokio::test]
async fn a_seq_covered_by_the_snapshot_is_refused() {
    let (store, _dir) = open().await;
    store
        .append_updates(DOC, &updates(1..=3), &NO_EDIT)
        .await
        .unwrap();
    store.compact(DOC, &snapshot(3)).await.unwrap();

    let result = store.append_updates(DOC, &updates([2, 4]), &NO_EDIT).await;

    assert!(matches!(
        result,
        Err(StoreError::SeqConflict { seq: 2, .. })
    ));
    assert!(store.load(DOC).await.unwrap().updates.is_empty());
}

#[tokio::test]
async fn a_seq_below_one_is_refused() {
    let (store, _dir) = open().await;

    let result = store.append_updates(DOC, &updates([0]), &NO_EDIT).await;

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
    store
        .append_updates(DOC, &updates(1..=3), &NO_EDIT)
        .await
        .unwrap();
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
            store.append_updates(DOC, &updates([4]), &NO_EDIT).await
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
    store
        .append_updates(DOC, &updates(1..=5), &NO_EDIT)
        .await
        .unwrap();

    store.compact(DOC, &snapshot(3)).await.unwrap();

    let doc = store.load(DOC).await.unwrap();
    assert_eq!(doc.snapshot, Some(snapshot(3)));
    assert_eq!(doc.updates, updates([4, 5]));
    assert_eq!(doc.last_seq(), 5);
}

#[tokio::test]
async fn seq_keeps_growing_after_a_full_compaction() {
    let (store, _dir) = open().await;
    store
        .append_updates(DOC, &updates(1..=5), &NO_EDIT)
        .await
        .unwrap();

    store.compact(DOC, &snapshot(5)).await.unwrap();

    let doc = store.load(DOC).await.unwrap();
    assert!(doc.updates.is_empty());
    assert_eq!(doc.last_seq(), 5);
}

#[tokio::test]
async fn a_newer_snapshot_replaces_the_stored_one() {
    let (store, _dir) = open().await;
    store
        .append_updates(DOC, &updates(1..=4), &NO_EDIT)
        .await
        .unwrap();
    store.compact(DOC, &snapshot(2)).await.unwrap();

    store.compact(DOC, &snapshot(4)).await.unwrap();

    let doc = store.load(DOC).await.unwrap();
    assert_eq!(doc.snapshot, Some(snapshot(4)));
    assert!(doc.updates.is_empty());
}

#[tokio::test]
async fn an_older_snapshot_is_refused_and_changes_nothing() {
    let (store, _dir) = open().await;
    store
        .append_updates(DOC, &updates(1..=4), &NO_EDIT)
        .await
        .unwrap();
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
    store
        .append_updates(DOC, &updates(1..=3), &NO_EDIT)
        .await
        .unwrap();
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
    store
        .append_updates(DOC, &updates([1]), &NO_EDIT)
        .await
        .unwrap();

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
                store
                    .append_updates(DOC, &updates([seq]), &NO_EDIT)
                    .await
                    .unwrap();
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
