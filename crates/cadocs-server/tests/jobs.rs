//! The cleanup jobs, on a running server.

mod common;

use std::time::Duration;

use cadocs_jobs::{
    Cleanup, Handler, MAINTAIN_DATABASE, PURGE_EXPIRED, PURGE_ORPHAN_FILES, PURGE_TRASH, ServerJobs,
};
use cadocs_store::{JobStore, SqliteStore, THIN_REVISIONS, now_ms};
use common::{Api, Login, ORIGIN, Reply, logged_in};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const DAY_MS: i64 = 24 * 3600 * 1000;
/// Past the default of `CADOCS_TRASH_DAYS`.
const TOO_OLD: i64 = 31;
const STILL_KEPT: i64 = 29;
const FAR_AHEAD: i64 = i64::MAX / 2;

struct Wiki {
    api: Api,
    ada: Login,
    workspace: String,
    space: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

async fn wiki() -> Wiki {
    let (api, ada) = logged_in().await;
    let workspace = api.first_workspace().await;
    let mut wiki = Wiki {
        api,
        ada,
        workspace,
        space: String::new(),
    };
    wiki.space = wiki.new_space("Handbook").await;
    wiki
}

impl Wiki {
    async fn new_space(&self, name: &str) -> String {
        let body = json!({ "name": name, "default_role": "editor" });
        let path = format!("/workspaces/{}/spaces", self.workspace);
        let reply = self.call("POST", &path, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn new_page(&self, parent: Option<&str>) -> String {
        let path = format!("/spaces/{}/pages", self.space);
        let reply = self
            .call("POST", &path, json!({ "parent_id": parent }))
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn call(&self, method: &str, path: &str, body: Value) -> Reply {
        self.api.send(method, path, &self.ada.headers(), body).await
    }

    async fn trash_page(&self, page: &str, days_ago: i64) {
        let path = format!("/pages/{page}");
        let reply = self
            .api
            .raw("DELETE", &path, &self.ada.headers(), b"")
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");
        self.age_page(page, days_ago).await;
    }

    /// Moves the trashing of a page and of what went with it to the past.
    async fn age_page(&self, page: &str, days_ago: i64) {
        let at = now_ms() - days_ago * DAY_MS;
        self.api
            .sql(&format!(
                "UPDATE pages SET deleted_at = {at}
                 WHERE deleted_at = (SELECT deleted_at FROM pages WHERE id = '{page}')"
            ))
            .await;
    }

    async fn trash_space(&self, space: &str, days_ago: i64) {
        let path = format!("/spaces/{space}");
        let reply = self
            .api
            .raw("DELETE", &path, &self.ada.headers(), b"")
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");
        let at = now_ms() - days_ago * DAY_MS;
        let update = format!("UPDATE spaces SET deleted_at = {at} WHERE id = '{space}'");
        self.api.sql(&update).await;
    }

    async fn page_exists(&self, page: &str) -> bool {
        let query = format!("SELECT count(*) FROM pages WHERE id = '{page}'");
        self.api.number(&query).await == 1
    }

    async fn socket(&self) -> Socket {
        let addr = self.api.addr;
        let url = format!("ws://{addr}/ws/events?workspace={}", self.workspace);
        let mut request = url.into_client_request().unwrap();
        let headers = request.headers_mut();
        headers.insert(COOKIE, self.ada.cookie.parse().unwrap());
        headers.insert("origin", ORIGIN.parse().unwrap());
        connect_async(request).await.unwrap().0
    }

    /// Runs a job and waits for it to end, with the reindexes it may cause. A revision waits for a
    /// pause in editing far longer than a test, so it is not waited for.
    async fn run(&self, kind: &str) {
        let queue = &self.api.queue;
        queue
            .enqueue(kind, Some(kind), "{}", now_ms())
            .await
            .unwrap();
        let ended = async {
            let query = "SELECT count(*) FROM jobs WHERE kind <> 'create_revision'";
            while self.api.number(query).await != 0 {
                sleep(Duration::from_millis(5)).await;
            }
        };
        timeout(Duration::from_secs(5), ended)
            .await
            .expect("the job never ended");
    }
}

/// The next event, past the pings and what a change of access adds.
async fn event(socket: &mut Socket) -> Value {
    timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => {
                    let event: Value = serde_json::from_str(&text).unwrap();
                    if event != json!({ "type": "access.changed" }) {
                        return event;
                    }
                }
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("expected an event, got {other:?}"),
            }
        }
    })
    .await
    .expect("an event")
}

async fn no_event(socket: &mut Socket) {
    let quiet = timeout(Duration::from_millis(150), event(socket)).await;
    assert!(quiet.is_err(), "{quiet:?}");
}

#[test]
fn every_cleanup_recurs() {
    assert_eq!(
        Cleanup::<SqliteStore>::RECURRING,
        [
            PURGE_EXPIRED,
            PURGE_TRASH,
            PURGE_ORPHAN_FILES,
            MAINTAIN_DATABASE
        ]
    );
    assert_eq!(
        ServerJobs::<SqliteStore>::RECURRING,
        [
            PURGE_EXPIRED,
            PURGE_TRASH,
            PURGE_ORPHAN_FILES,
            MAINTAIN_DATABASE,
            THIN_REVISIONS
        ]
    );
}

#[tokio::test]
async fn database_maintenance_gives_free_pages_back() {
    let wiki = wiki().await;
    let api = &wiki.api;
    api.sql("CREATE TABLE filler (data BLOB)").await;
    api.sql(
        "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 500)
         INSERT INTO filler SELECT randomblob(4000) FROM n",
    )
    .await;
    api.sql("DROP TABLE filler").await;
    assert!(api.number("PRAGMA freelist_count").await > 500);

    wiki.run(MAINTAIN_DATABASE).await;
    assert_eq!(api.number("PRAGMA freelist_count").await, 0);
}

#[tokio::test]
async fn a_page_past_its_time_goes_as_a_purge_by_hand_would() {
    let wiki = wiki().await;
    let page = wiki.new_page(None).await;
    let child = wiki.new_page(Some(&page)).await;
    wiki.call(
        "PATCH",
        &format!("/pages/{page}"),
        json!({ "title": "Old" }),
    )
    .await;
    wiki.trash_page(&page, TOO_OLD).await;
    sleep(Duration::from_millis(600)).await;
    let stored = "SELECT count(*) FROM doc_updates";
    assert!(wiki.api.number(stored).await > 0, "the page has a document");
    let mut socket = wiki.socket().await;
    let before = now_ms() - 30 * DAY_MS;
    let listed = wiki.api.store.expired_pages(before, 10).await.unwrap();
    assert_eq!(listed.len(), 1, "the child goes with its parent");

    wiki.run(PURGE_TRASH).await;

    assert!(!wiki.page_exists(&page).await);
    assert!(!wiki.page_exists(&child).await);
    assert_eq!(wiki.api.number(stored).await, 0);
    let purged = json!({ "type": "page.purged", "space": wiki.space, "page": page });
    assert_eq!(event(&mut socket).await, purged);
    no_event(&mut socket).await;
}

#[tokio::test]
async fn a_page_within_its_time_stays() {
    let wiki = wiki().await;
    let page = wiki.new_page(None).await;
    wiki.trash_page(&page, STILL_KEPT).await;
    let live = wiki.new_page(None).await;

    wiki.run(PURGE_TRASH).await;

    assert!(wiki.page_exists(&page).await);
    assert!(wiki.page_exists(&live).await);
}

#[tokio::test]
async fn an_old_child_goes_from_under_a_recent_parent() {
    let wiki = wiki().await;
    let parent = wiki.new_page(None).await;
    let child = wiki.new_page(Some(&parent)).await;
    wiki.trash_page(&child, TOO_OLD).await;
    wiki.trash_page(&parent, 1).await;
    let mut socket = wiki.socket().await;

    wiki.run(PURGE_TRASH).await;

    assert!(wiki.page_exists(&parent).await);
    assert!(!wiki.page_exists(&child).await);
    let purged = json!({ "type": "page.purged", "space": wiki.space, "page": child });
    assert_eq!(event(&mut socket).await, purged);
}

#[tokio::test]
async fn more_pages_than_a_batch_all_go() {
    let wiki = wiki().await;
    let page = wiki.new_page(None).await;
    wiki.trash_page(&page, TOO_OLD).await;
    wiki.api
        .sql(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < 250)
             INSERT INTO pages (id, workspace_id, space_id, position, slug, title,
                                deleted_at, created_at, updated_at)
             SELECT 'page-' || i, p.workspace_id, p.space_id, 'a' || i, 'slug-' || i, '',
                    p.deleted_at, 0, 0
             FROM n, pages p",
        )
        .await;

    wiki.run(PURGE_TRASH).await;

    assert_eq!(wiki.api.number("SELECT count(*) FROM pages").await, 0);
}

#[tokio::test]
async fn a_restore_just_before_the_purge_wins() {
    let wiki = wiki().await;
    let page = wiki.new_page(None).await;
    wiki.trash_page(&page, TOO_OLD).await;
    let before = now_ms() - 30 * DAY_MS;
    let listed = wiki.api.store.expired_pages(before, 10).await.unwrap();
    assert_eq!(listed.len(), 1);

    let path = format!("/pages/{page}/restore");
    assert_eq!(wiki.call("POST", &path, json!({})).await.status, 200);

    assert!(
        !wiki
            .api
            .store
            .purge_expired_page(&page, before)
            .await
            .unwrap()
    );
    assert!(wiki.page_exists(&page).await);
}

#[tokio::test]
async fn a_space_past_its_time_goes_with_its_pages() {
    let wiki = wiki().await;
    let page = wiki.new_page(None).await;
    let kept = wiki.new_space("Kept").await;
    wiki.trash_space(&wiki.space, TOO_OLD).await;
    wiki.trash_space(&kept, STILL_KEPT).await;
    let mut socket = wiki.socket().await;

    wiki.run(PURGE_TRASH).await;

    let there = |space: &str| format!("SELECT count(*) FROM spaces WHERE id = '{space}'");
    assert_eq!(wiki.api.number(&there(&wiki.space)).await, 0);
    assert_eq!(wiki.api.number(&there(&kept)).await, 1);
    assert!(!wiki.page_exists(&page).await);
    let purged = json!({ "type": "space.purged", "space": wiki.space });
    assert_eq!(event(&mut socket).await, purged);
    no_event(&mut socket).await;
}

#[tokio::test]
async fn a_space_restored_just_before_the_purge_stays() {
    let wiki = wiki().await;
    wiki.trash_space(&wiki.space, TOO_OLD).await;
    let before = now_ms() - 30 * DAY_MS;
    let listed = wiki.api.store.expired_spaces(before, 10).await.unwrap();
    assert_eq!(listed.len(), 1);

    let path = format!("/spaces/{}/restore", wiki.space);
    assert_eq!(wiki.call("POST", &path, json!({})).await.status, 200);

    let store = &wiki.api.store;
    assert!(
        !store
            .purge_expired_space(&wiki.space, before)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn expired_sessions_and_links_go_and_live_ones_stay() {
    let wiki = wiki().await;
    let api = &wiki.api;
    let workspace = &wiki.workspace;
    for (name, expires_at) in [("old", 1), ("live", FAR_AHEAD)] {
        api.sql(&format!(
            "INSERT INTO sessions (token_hash, user_id, created_at, last_seen_at, expires_at)
             SELECT '{name}', id, 0, 0, {expires_at} FROM users"
        ))
        .await;
        api.sql(&format!(
            "INSERT INTO password_resets (token_hash, user_id, expires_at, created_at)
             SELECT '{name}', id, {expires_at}, 0 FROM users"
        ))
        .await;
        api.sql(&format!(
            "INSERT INTO workspace_invitations
                 (id, workspace_id, email, role, token_hash, invited_by, expires_at, created_at)
             SELECT '{name}', '{workspace}', '{name}@example.org', 'member', '{name}', id,
                    {expires_at}, 0 FROM users"
        ))
        .await;
    }
    let sessions = api.number("SELECT count(*) FROM sessions").await;

    wiki.run(PURGE_EXPIRED).await;

    assert_eq!(
        api.number("SELECT count(*) FROM sessions").await,
        sessions - 1
    );
    for table in ["password_resets", "workspace_invitations"] {
        let left = format!("SELECT count(*) FROM {table} WHERE token_hash = 'live'");
        assert_eq!(api.number(&left).await, 1, "{table}");
        let all = format!("SELECT count(*) FROM {table}");
        assert_eq!(api.number(&all).await, 1, "{table}");
    }
    assert_eq!(api.session(&wiki.ada).await.status, 200);
}

#[tokio::test]
async fn a_failed_job_stays_for_a_month() {
    let wiki = wiki().await;
    let api = &wiki.api;
    let recent = now_ms() - DAY_MS;
    for (name, at) in [("old", now_ms() - TOO_OLD * DAY_MS), ("recent", recent)] {
        api.sql(&format!(
            "INSERT INTO jobs (id, kind, status, run_at, locked_at, created_at)
             VALUES ('{name}', 'test', 'failed', 0, {at}, 0)"
        ))
        .await;
    }
    let queue = &api.queue;
    queue
        .enqueue(PURGE_EXPIRED, None, "{}", now_ms())
        .await
        .unwrap();

    let gone = async {
        while api.number("SELECT count(*) FROM jobs").await != 1 {
            sleep(Duration::from_millis(5)).await;
        }
    };
    timeout(Duration::from_secs(5), gone)
        .await
        .expect("it never went");

    let left = "SELECT count(*) FROM jobs WHERE id = 'recent'";
    assert_eq!(api.number(left).await, 1);
}
