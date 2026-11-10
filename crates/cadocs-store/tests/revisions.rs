//! What the store does for revisions: the jobs and authors a flush records, numbering, reading
//! with the page's roles, and thinning. Restoring is tested over REST.

use std::path::Path;
use std::time::Duration;

use cadocs_store::{
    DocEdit, DocStore, NewRevision, RevisionAuthor, RevisionStore, RevisionTiming, SqliteStore,
    StoreError, StoredUpdate, now_ms,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteConnection};
use sqlx::{Connection, Executor};
use tempfile::TempDir;

const TIMING: RevisionTiming = RevisionTiming {
    pause: Duration::from_secs(60),
    max: Duration::from_secs(600),
};
const DAY: i64 = 86_400_000;

async fn open() -> (SqliteStore, TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap()
        .with_revision_timing(TIMING);
    sql(
        &dir,
        "INSERT INTO users (id, email, name, created_at, updated_at)
             VALUES ('ada', 'ada@example.org', 'Ada', 1, 1),
                    ('bob', 'bob@example.org', 'Bob', 1, 1),
                    ('vic', 'vic@example.org', 'Vic', 1, 1),
                    ('eve', 'eve@example.org', 'Eve', 1, 1);
         INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES ('ws', 'ws', 'Workspace', 1, 1);
         INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             VALUES ('ws', 'ada', 'owner', 1), ('ws', 'bob', 'member', 1),
                    ('ws', 'vic', 'member', 1);
         INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
             VALUES ('space', 'ws', 'space', 'Space', 1, 1),
                    ('hidden', 'ws', 'hidden', 'Hidden', 1, 1);
         INSERT INTO space_grants (id, space_id, user_id, role, created_at)
             VALUES ('g1', 'space', 'bob', 'editor', 1), ('g2', 'space', 'vic', 'viewer', 1);
         INSERT INTO pages (id, workspace_id, space_id, position, slug, title, created_at,
                            updated_at)
             VALUES ('doc', 'ws', 'space', 'a', 'doc', 'Doc', 1, 1),
                    ('next', 'ws', 'space', 'b', 'next', 'Next', 1, 1),
                    ('secret', 'ws', 'hidden', 'a', 'secret', 'Secret', 1, 1);",
    )
    .await;
    (store, dir)
}

async fn connection(dir: &TempDir) -> SqliteConnection {
    let path: &Path = &dir.path().join("cadocs.db");
    SqliteConnection::connect_with(&SqliteConnectOptions::new().filename(path))
        .await
        .unwrap()
}

async fn sql(dir: &TempDir, statement: &str) {
    let mut connection = connection(dir).await;
    let statement = sqlx::AssertSqlSafe(statement.to_owned());
    connection.execute(sqlx::raw_sql(statement)).await.unwrap();
    connection.close().await.unwrap();
}

async fn flush(store: &SqliteStore, page: &str, seq: i64, author: &str) {
    let update = StoredUpdate {
        seq,
        data: vec![1, 2, 3],
    };
    let edit = DocEdit {
        title: None,
        author: Some(author.to_owned()),
    };
    store.append_updates(page, &[update], &edit).await.unwrap();
}

/// `(dedup_key, payload, run_at, created_at)` of the pending revision jobs.
async fn revision_jobs(dir: &TempDir) -> Vec<(String, String, i64, i64)> {
    let mut connection = connection(dir).await;
    sqlx::query_as(
        "SELECT dedup_key, payload, run_at, created_at FROM jobs
         WHERE kind = 'create_revision' AND status = 'pending' ORDER BY dedup_key",
    )
    .fetch_all(&mut connection)
    .await
    .unwrap()
}

async fn add(store: &SqliteStore, id: &str, page: &str, read_at: i64) -> Option<i64> {
    store
        .add_revision(&NewRevision {
            id,
            page_id: page,
            title: id,
            state: id.as_bytes(),
            read_at,
        })
        .await
        .unwrap()
}

fn names(authors: &[RevisionAuthor]) -> Vec<&str> {
    authors.iter().map(|author| author.name.as_str()).collect()
}

#[tokio::test]
async fn a_flush_queues_one_revision_after_the_pause() {
    let (store, dir) = open().await;
    let before = now_ms();
    flush(&store, "doc", 1, "ada").await;

    let jobs = revision_jobs(&dir).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    let (key, payload, run_at, _) = &jobs[0];
    assert_eq!(key, "revision:doc");
    assert_eq!(payload, r#"{"page":"doc"}"#);
    let pause = TIMING.pause.as_millis() as i64;
    assert!(*run_at >= before + pause && *run_at <= now_ms() + pause);
}

#[tokio::test]
async fn each_flush_pushes_the_revision_back_up_to_the_longest_wait() {
    let (store, dir) = open().await;
    flush(&store, "doc", 1, "ada").await;
    let first = revision_jobs(&dir).await[0].2;
    tokio::time::sleep(Duration::from_millis(20)).await;
    flush(&store, "doc", 2, "ada").await;
    let jobs = revision_jobs(&dir).await;
    assert_eq!(jobs.len(), 1, "{jobs:?}");
    assert!(jobs[0].2 > first, "a later edit pushes the revision back");

    // An editing session that started long ago is due at its longest wait.
    sql(&dir, "UPDATE jobs SET created_at = created_at - 590000").await;
    flush(&store, "doc", 3, "ada").await;
    let (_, _, run_at, created_at) = revision_jobs(&dir).await[0];
    assert_eq!(run_at, created_at + TIMING.max.as_millis() as i64);
}

#[tokio::test]
async fn revisions_are_numbered_and_name_their_authors() {
    let (store, dir) = open().await;
    flush(&store, "doc", 1, "ada").await;
    flush(&store, "doc", 2, "bob").await;
    flush(&store, "doc", 3, "gone").await;
    assert_eq!(add(&store, "r1", "doc", now_ms()).await, Some(1));

    // Bob writes while the second revision reads: he is in it, and stays for the next.
    let read_at = now_ms();
    tokio::time::sleep(Duration::from_millis(5)).await;
    flush(&store, "doc", 4, "bob").await;
    assert_eq!(add(&store, "r2", "doc", read_at).await, Some(2));
    assert_eq!(add(&store, "r3", "doc", now_ms()).await, Some(3));
    assert_eq!(add(&store, "r4", "doc", now_ms()).await, Some(4));

    let listed = store.revisions("doc", "vic", None, 10).await.unwrap();
    let numbers: Vec<i64> = listed.iter().map(|r| r.number).collect();
    assert_eq!(numbers, [4, 3, 2, 1]);
    assert_eq!(names(&listed[3].authors), ["Ada", "Bob"]);
    assert_eq!(names(&listed[2].authors), ["Bob"]);
    assert_eq!(names(&listed[1].authors), ["Bob"]);
    assert!(listed[0].authors.is_empty());
    assert_eq!(listed[3].title, "r1");

    let older = store.revisions("doc", "vic", Some(3), 1).await.unwrap();
    assert_eq!(older.iter().map(|r| r.number).collect::<Vec<_>>(), [2]);

    // A deleted account leaves its revisions without its name.
    sql(&dir, "DELETE FROM users WHERE id = 'ada'").await;
    let listed = store.revisions("doc", "vic", None, 10).await.unwrap();
    assert_eq!(names(&listed[3].authors), ["Bob"]);

    // Numbers are per page.
    assert_eq!(add(&store, "n1", "next", now_ms()).await, Some(1));
    let latest = store.latest_revision("doc").await.unwrap().unwrap();
    assert_eq!((latest.summary.number, latest.state), (4, b"r4".to_vec()));
    assert_eq!(store.latest_revision("secret").await.unwrap(), None);
}

#[tokio::test]
async fn a_revision_of_a_page_that_is_gone_is_not_stored() {
    let (store, dir) = open().await;
    sql(&dir, "DELETE FROM pages WHERE id = 'doc'").await;
    assert_eq!(add(&store, "r1", "doc", now_ms()).await, None);
    let mut connection = connection(&dir).await;
    let count: i64 = sqlx::query_scalar("SELECT count(*) FROM page_revisions")
        .fetch_one(&mut connection)
        .await
        .unwrap();
    assert_eq!(count, 0);
}

#[tokio::test]
async fn readers_read_revisions_and_editors_restore_them() {
    let (store, dir) = open().await;
    add(&store, "r1", "doc", now_ms()).await;

    let read = store.revision("doc", 1, "vic").await.unwrap();
    assert_eq!(read.state, b"r1");
    assert!(matches!(
        store.revision("doc", 2, "vic").await,
        Err(StoreError::RevisionNotFound)
    ));
    assert!(matches!(
        store.revision_to_restore("doc", 1, "vic").await,
        Err(StoreError::NotAllowed)
    ));
    assert_eq!(
        store.revision_to_restore("doc", 1, "bob").await.unwrap(),
        read
    );
    for refused in [
        store.revision("doc", 1, "eve").await.map(|_| ()),
        store.revisions("doc", "eve", None, 10).await.map(|_| ()),
        store.revision_to_restore("doc", 1, "eve").await.map(|_| ()),
    ] {
        assert!(
            matches!(refused, Err(StoreError::PageNotFound)),
            "{refused:?}"
        );
    }

    // A trashed page shows no revision.
    sql(&dir, "UPDATE pages SET deleted_at = 1 WHERE id = 'doc'").await;
    assert!(matches!(
        store.revisions("doc", "bob", None, 10).await,
        Err(StoreError::PageNotFound)
    ));
    assert!(matches!(
        store.revision_to_restore("doc", 1, "bob").await,
        Err(StoreError::PageNotFound)
    ));
}

#[tokio::test]
async fn thinning_keeps_one_revision_per_day_and_the_latest() {
    let (store, dir) = open().await;
    for id in ["a1", "a2", "b1", "b2", "c1"] {
        add(&store, id, "doc", now_ms()).await;
    }
    add(&store, "old", "next", now_ms()).await;
    // Days 10 and 11 are past the retention, day 30 is not.
    sql(
        &dir,
        &format!(
            "UPDATE page_revisions SET created_at = CASE title
                 WHEN 'a1' THEN {d10} + 1 WHEN 'a2' THEN {d10} + 2
                 WHEN 'b1' THEN {d11} + 1 WHEN 'b2' THEN {d11} + 2
                 ELSE {d11} + 3 END",
            d10 = 10 * DAY,
            d11 = 11 * DAY,
        ),
    )
    .await;
    sql(
        &dir,
        &format!(
            "UPDATE page_revisions SET created_at = {} WHERE title = 'c1'",
            30 * DAY
        ),
    )
    .await;

    assert_eq!(store.thin_revisions(20 * DAY).await.unwrap(), 2);
    let kept = store.revisions("doc", "ada", None, 10).await.unwrap();
    let titles: Vec<&str> = kept.iter().map(|r| r.title.as_str()).collect();
    assert_eq!(titles, ["c1", "b2", "a2"]);
    // The only revision of a page stays, however old.
    assert_eq!(
        store
            .revisions("next", "ada", None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn titles_are_given_for_readable_live_pages_only() {
    let (store, dir) = open().await;
    sql(
        &dir,
        "INSERT INTO pages (id, workspace_id, space_id, position, slug, title, deleted_at,
                            created_at, updated_at)
             VALUES ('binned', 'ws', 'space', 'c', 'binned', 'Binned', 1, 1, 1)",
    )
    .await;
    let ids = ["doc", "secret", "binned", "nowhere", "next"].map(str::to_owned);
    let mut titles = store.readable_titles("vic", &ids).await.unwrap();
    titles.sort();
    assert_eq!(
        titles,
        [
            ("doc".to_owned(), "Doc".to_owned()),
            ("next".to_owned(), "Next".to_owned())
        ]
    );
}
