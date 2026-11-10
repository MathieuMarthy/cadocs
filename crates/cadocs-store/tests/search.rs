//! What the store does for search: reindex jobs queued by edits and by a language change, the
//! index itself, and the attachment references it adds. Who may read a result is tested over REST.

use std::path::Path;
use std::time::Duration;

use cadocs_store::{
    DocEdit, DocStore, JobStore, PageIndex, SearchStore, SqliteStore, StoredUpdate,
    WorkspaceChanges, WorkspaceStore, now_ms,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Row};
use tempfile::TempDir;
use tokio::time::timeout;

const DELAY: Duration = Duration::from_secs(60);

async fn open() -> (SqliteStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap()
        .with_reindex_delay(DELAY);
    sql(
        &dir,
        "INSERT INTO users (id, email, name, created_at, updated_at)
             VALUES ('ada', 'ada@example.org', 'Ada', 1, 1);
         INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES ('ws', 'ws', 'Workspace', 1, 1), ('other', 'other', 'Other', 1, 1);
         INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             VALUES ('ws', 'ada', 'owner', 1);
         INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
             VALUES ('space', 'ws', 'space', 'Space', 1, 1),
                    ('far', 'other', 'far', 'Far', 1, 1);
         INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
             VALUES ('doc', 'ws', 'space', 'a', 'doc', 1, 1),
                    ('next', 'ws', 'space', 'b', 'next', 1, 1),
                    ('away', 'other', 'far', 'a', 'away', 1, 1);
         INSERT INTO attachments (id, workspace_id, file_name, content_type, size, sha256,
                                  created_at)
             VALUES ('mine', 'ws', 'a.png', 'image/png', 1, '', 1),
                    ('theirs', 'other', 'b.png', 'image/png', 1, '', 1);",
    )
    .await;
    (store, dir)
}

async fn connection(path: &Path) -> SqliteConnection {
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap()
}

async fn sql(dir: &TempDir, statement: &'static str) {
    let mut connection = connection(&dir.path().join("cadocs.db")).await;
    sqlx::raw_sql(statement)
        .execute(&mut connection)
        .await
        .unwrap();
    connection.close().await.unwrap();
}

/// `(dedup_key, payload, run_at)` of the pending reindex jobs, by key.
async fn reindex_jobs(dir: &TempDir) -> Vec<(String, String, i64)> {
    let mut connection = connection(&dir.path().join("cadocs.db")).await;
    sqlx::query_as(
        "SELECT dedup_key, payload, run_at FROM jobs
         WHERE kind = 'reindex_page' AND status = 'pending' ORDER BY dedup_key",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap()
}

async fn flush(store: &SqliteStore, page: &str, seq: i64) {
    let update = StoredUpdate {
        seq,
        data: vec![1, 2, 3],
    };
    store
        .append_updates(page, &[update], &DocEdit::default())
        .await
        .unwrap();
}

fn index<'a>(page: &'a str, body: &'a str, attachments: &'a [String]) -> PageIndex<'a> {
    PageIndex {
        page_id: page,
        title: "titl",
        body,
        plain_text: body,
        attachments,
    }
}

#[tokio::test]
async fn a_flush_queues_one_delayed_reindex_of_its_page() {
    let (store, dir) = open().await;
    let before = now_ms();
    flush(&store, "doc", 1).await;
    flush(&store, "doc", 2).await;

    let jobs = reindex_jobs(&dir).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let (key, payload, run_at) = &jobs[0];
    assert_eq!(key, "reindex:doc");
    assert_eq!(payload, r#"{"page":"doc"}"#);
    let delay = DELAY.as_millis() as i64;
    assert!(*run_at >= before + delay && *run_at <= now_ms() + delay);
}

#[tokio::test]
async fn a_flush_that_queues_a_reindex_wakes_the_worker() {
    let (store, _dir) = open().await;
    let added = store.jobs_added();
    flush(&store, "doc", 1).await;
    timeout(Duration::from_secs(1), added.notified())
        .await
        .expect("the queued reindex must wake the worker");
}

#[tokio::test]
async fn a_refused_flush_queues_nothing() {
    let (store, dir) = open().await;
    let update = StoredUpdate {
        seq: 0,
        data: vec![1],
    };
    let refused = store
        .append_updates("doc", &[update], &DocEdit::default())
        .await;
    assert!(refused.is_err());
    assert!(reindex_jobs(&dir).await.is_empty());
}

#[tokio::test]
async fn a_new_language_reindexes_the_whole_workspace() {
    let (store, dir) = open().await;
    let changes = |language: &str| WorkspaceChanges {
        name: None,
        language: Some(language.to_owned()),
    };
    store
        .update_workspace("ws", "ada", &changes("en"), now_ms())
        .await
        .unwrap();
    assert!(
        reindex_jobs(&dir).await.is_empty(),
        "the language is the same"
    );

    let added = store.jobs_added();
    store
        .update_workspace("ws", "ada", &changes("fr"), now_ms())
        .await
        .unwrap();
    let keys: Vec<String> = reindex_jobs(&dir).await.into_iter().map(|j| j.0).collect();
    assert_eq!(keys, ["reindex:doc", "reindex:next"]);
    timeout(Duration::from_secs(1), added.notified())
        .await
        .expect("the reindex must wake the worker");
}

#[tokio::test]
async fn the_target_of_an_index_is_the_language_of_the_workspace() {
    let (store, dir) = open().await;
    sql(
        &dir,
        "UPDATE workspaces SET language = 'fr' WHERE id = 'ws'",
    )
    .await;
    let target = store.index_target("doc").await.unwrap().unwrap();
    assert_eq!(target.language, "fr");
    assert!(store.index_target("gone").await.unwrap().is_none());
}

#[tokio::test]
async fn indexing_twice_replaces_the_text() {
    let (store, dir) = open().await;
    assert!(store.index_page(&index("doc", "first", &[])).await.unwrap());
    assert!(
        store
            .index_page(&index("doc", "second", &[]))
            .await
            .unwrap()
    );

    let mut connection = connection(&dir.path().join("cadocs.db")).await;
    let rows = sqlx::query(
        "SELECT t.plain_text, f.body FROM page_text t JOIN page_fts f ON f.rowid = t.id",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].get::<String, _>(0), "second");
    let fts: i64 = sqlx::query_scalar("SELECT count(*) FROM page_fts")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(fts, 1);
}

#[tokio::test]
async fn indexing_a_page_that_is_gone_does_nothing() {
    let (store, _dir) = open().await;
    assert!(!store.index_page(&index("gone", "text", &[])).await.unwrap());
}

#[tokio::test]
async fn purging_a_page_drops_its_index() {
    let (store, dir) = open().await;
    store.index_page(&index("doc", "text", &[])).await.unwrap();
    sql(&dir, "DELETE FROM pages WHERE id = 'doc'").await;

    let mut connection = connection(&dir.path().join("cadocs.db")).await;
    let left: i64 = sqlx::query_scalar(
        "SELECT (SELECT count(*) FROM page_text) + (SELECT count(*) FROM page_fts)",
    )
    .fetch_one(&mut connection)
    .await
    .unwrap();
    assert_eq!(left, 0);
}

#[tokio::test]
async fn indexing_references_the_attachments_of_the_same_workspace_only() {
    let (store, dir) = open().await;
    let found = ["mine".to_owned(), "theirs".to_owned(), "unknown".to_owned()];
    store
        .index_page(&index("doc", "text", &found))
        .await
        .unwrap();
    store
        .index_page(&index("doc", "text", &found))
        .await
        .unwrap();

    let mut connection = connection(&dir.path().join("cadocs.db")).await;
    let refs: Vec<(String, String)> =
        sqlx::query_as("SELECT attachment_id, page_id FROM attachment_refs ORDER BY 1")
            .fetch_all(&mut connection)
            .await
            .unwrap();
    assert_eq!(refs, [("mine".to_owned(), "doc".to_owned())]);
}

#[tokio::test]
async fn the_migration_queues_a_first_index_of_every_page() {
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
        include_str!("../migrations/sqlite/0007_doc_pages.sql"),
        include_str!("../migrations/sqlite/0008_jobs.sql"),
        include_str!("../migrations/sqlite/0009_attachments.sql"),
        "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES ('ws', 'ws', 'Workspace', 1, 1);
         INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
             VALUES ('space', 'ws', 'space', 'Space', 1, 1);
         INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
             VALUES ('a', 'ws', 'space', 'a', 'a', 1, 1), ('b', 'ws', 'space', 'b', 'b', 1, 1);",
        include_str!("../migrations/sqlite/0010_search.sql"),
    ] {
        sqlx::raw_sql(script)
            .execute(&mut connection)
            .await
            .unwrap();
    }
    let jobs: Vec<(String, String)> = sqlx::query_as(
        "SELECT dedup_key, payload FROM jobs WHERE kind = 'reindex_page' AND run_at > 0
         ORDER BY dedup_key",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap();
    assert_eq!(
        jobs,
        [
            ("reindex:a".to_owned(), r#"{"page":"a"}"#.to_owned()),
            ("reindex:b".to_owned(), r#"{"page":"b"}"#.to_owned()),
        ]
    );
}

#[tokio::test]
async fn jobs_added_is_shared_by_every_clone() {
    let (store, _dir) = open().await;
    let added = store.clone().jobs_added();
    flush(&store, "doc", 1).await;
    timeout(Duration::from_secs(1), added.notified())
        .await
        .expect("a clone must hear it too");
}

#[tokio::test]
async fn enqueue_wakes_through_jobs_added() {
    let (store, _dir) = open().await;
    let added = store.jobs_added();
    let job = cadocs_store::NewJob {
        id: "j".into(),
        kind: "k".into(),
        dedup_key: None,
        payload: "{}".into(),
        run_at: 0,
        now: 0,
    };
    store.enqueue(&job).await.unwrap();
    timeout(Duration::from_secs(1), added.notified())
        .await
        .expect("enqueue must wake the worker");
}
