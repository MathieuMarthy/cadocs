//! Revisions over real sockets: taken by the job after edits, listed and shown over REST to the
//! page's readers, and restored by its editors as an edit open clients receive.

mod common;

use std::time::Duration;

use cadocs_content::ast::{Block, Document, Inline};
use cadocs_content::yjs::{read_page, replace_page};
use common::{Api, Login, ORIGIN, Reply, start_tuned};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, Transact, Update};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const VIC: &str = "33333333-0000-7000-8000-000000000001";
/// Longer than a flush and a revision job.
const PATIENCE: Duration = Duration::from_secs(10);
/// Past a flush, with the interval the tests set.
const FLUSHED: Duration = Duration::from_millis(600);

struct Wiki {
    api: Api,
    ada: Login,
    /// An editor of the space.
    bob: Login,
    /// A viewer of the space.
    vic: Login,
    space: String,
    page: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

fn paragraph(inlines: Vec<Inline>) -> Document {
    Document {
        blocks: vec![Block::Paragraph(inlines)],
    }
}

fn text(value: &str) -> Inline {
    Inline::Text {
        text: value.to_owned(),
        marks: Default::default(),
    }
}

/// A revision after each flush, and flushes soon after an edit.
async fn wiki() -> Wiki {
    let settings = [
        ("CADOCS_REVISION_PAUSE_SECS", "0"),
        ("CADOCS_REVISION_MAX_SECS", "0"),
        ("CADOCS_FLUSH_INTERVAL_MS", "100"),
    ];
    let api = start_tuned(ORIGIN, "", &settings).await;
    let reply = api.setup().await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let ada = Login::from(&reply);
    let workspace = api.first_workspace().await;
    let bob = api
        .add_member(&workspace, BOB, "bob@example.org", "member")
        .await;
    let vic = api
        .add_member(&workspace, VIC, "vic@example.org", "member")
        .await;
    let body = json!({ "name": "Handbook", "default_role": "viewer" });
    let path = format!("/workspaces/{workspace}/spaces");
    let reply = api.send("POST", &path, &ada.headers(), body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let space = id(&reply);
    let grants = json!({ "grants": [{ "user_id": BOB, "role": "editor" }] });
    let reply = api
        .send(
            "PUT",
            &format!("/spaces/{space}/grants"),
            &ada.headers(),
            grants,
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api
        .send(
            "POST",
            &format!("/spaces/{space}/pages"),
            &ada.headers(),
            json!({}),
        )
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let page = id(&reply);
    Wiki {
        api,
        ada,
        bob,
        vic,
        space,
        page,
    }
}

/// A client of the page socket that keeps its document, as the editor does.
struct Editor {
    socket: Socket,
    doc: Doc,
}

impl Editor {
    async fn open(wiki: &Wiki, by: &Login) -> Self {
        let url = format!("ws://{}/ws/pages/{}", wiki.api.addr, wiki.page);
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert(COOKIE, by.cookie.parse().unwrap());
        let (socket, _) = connect_async(request).await.unwrap();
        Self {
            socket,
            doc: Doc::new(),
        }
    }

    async fn write(&mut self, title: &str, document: &Document) {
        let update = {
            let mut txn = self.doc.transact_mut();
            replace_page(&mut txn, title, document);
            txn.encode_update_v1()
        };
        let frame = YMessage::Sync(SyncMessage::Update(update)).encode_v1();
        self.socket.send(Message::binary(frame)).await.unwrap();
    }

    /// Applies what arrives until the page reads as `title` and `document`.
    async fn wait_for(&mut self, title: &str, document: &Document) {
        let expected = (title.to_owned(), document.clone());
        timeout(PATIENCE, async {
            while read_page(&self.doc) != expected {
                let Message::Binary(frame) = self.socket.next().await.unwrap().unwrap() else {
                    continue;
                };
                if let YMessage::Sync(
                    SyncMessage::SyncStep2(update) | SyncMessage::Update(update),
                ) = YMessage::decode_v1(&frame).unwrap()
                {
                    let update = Update::decode_v1(&update).unwrap();
                    self.doc.transact_mut().apply_update(update).unwrap();
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expected:?} never arrived, {:?}", read_page(&self.doc)));
    }
}

impl Wiki {
    async fn list(&self, by: &Login) -> Reply {
        let path = format!("/pages/{}/revisions", self.page);
        self.api.get(by, &path).await
    }

    /// The titles of the revisions, the most recent first, once `done` holds or time is up.
    async fn titles_until(&self, done: impl Fn(&[String]) -> bool) -> Vec<String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let reply = self.list(&self.ada).await;
            assert_eq!(reply.status, 200, "{reply:?}");
            let titles: Vec<String> = reply.body["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["title"].as_str().unwrap().to_owned())
                .collect();
            if done(&titles) || Instant::now() > deadline {
                return titles;
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    async fn restore(&self, by: &Login, number: &str) -> Reply {
        let path = format!("/pages/{}/revisions/{number}/restore", self.page);
        self.api.send("POST", &path, &by.headers(), json!({})).await
    }
}

#[tokio::test]
async fn edits_make_revisions_that_readers_list_and_view() {
    let wiki = wiki().await;
    let mut editor = Editor::open(&wiki, &wiki.ada).await;
    editor.write("First", &paragraph(vec![text("one")])).await;
    assert_eq!(wiki.titles_until(|t| t.len() == 1).await, ["First"]);
    editor.write("Second", &paragraph(vec![text("two")])).await;
    assert_eq!(
        wiki.titles_until(|t| t.len() == 2).await,
        ["Second", "First"]
    );

    let reply = wiki.list(&wiki.vic).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items[0]["number"], 2);
    assert_eq!(items[1]["number"], 1);
    let ada = wiki.api.session(&wiki.ada).await.body["user"]["id"].clone();
    assert_eq!(items[1]["authors"][0]["id"], ada);
    assert!(items[1]["authors"][0]["name"].is_string());
    assert!(items[1]["created_at"].is_i64());
    assert_eq!(reply.body["next_cursor"], Value::Null);

    let reply = wiki
        .api
        .get(
            &wiki.vic,
            &format!("/pages/{}/revisions?limit=1", wiki.page),
        )
        .await;
    assert_eq!(reply.body["items"][0]["number"], 2);
    let cursor = reply.body["next_cursor"].as_str().unwrap().to_owned();
    let path = format!("/pages/{}/revisions?limit=1&cursor={cursor}", wiki.page);
    let reply = wiki.api.get(&wiki.vic, &path).await;
    assert_eq!(reply.body["items"][0]["number"], 1, "{reply:?}");

    let reply = wiki
        .api
        .get(&wiki.vic, &format!("/pages/{}/revisions/1", wiki.page))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["number"], 1);
    assert_eq!(reply.body["title"], "First");
    assert_eq!(reply.body["html"], "<p>one</p>");
}

#[tokio::test]
async fn content_that_did_not_change_makes_no_revision() {
    let wiki = wiki().await;
    let mut editor = Editor::open(&wiki, &wiki.ada).await;
    let document = paragraph(vec![text("same")]);
    editor.write("Same", &document).await;
    assert_eq!(wiki.titles_until(|t| t.len() == 1).await, ["Same"]);
    // Written again: new Yjs items, the same content.
    editor.write("Same", &document).await;
    sleep(FLUSHED * 3).await;
    assert_eq!(wiki.titles_until(|_| true).await, ["Same"]);
}

#[tokio::test]
async fn a_restore_reaches_open_clients_and_keeps_the_previous_content() {
    let wiki = wiki().await;
    let mut editor = Editor::open(&wiki, &wiki.ada).await;
    let first = paragraph(vec![text("one")]);
    editor.write("First", &first).await;
    wiki.titles_until(|t| t.len() == 1).await;
    // Restored before its flush, maybe: then only the restore can keep this content.
    editor.write("Second", &paragraph(vec![text("two")])).await;
    sleep(Duration::from_millis(50)).await;

    let reply = wiki.restore(&wiki.bob, "1").await;
    assert_eq!(reply.status, 204, "{reply:?}");

    editor.wait_for("First", &first).await;
    let titles = wiki.titles_until(|t| t.len() >= 3 && t[0] == "First").await;
    assert!(titles.contains(&"Second".to_owned()), "{titles:?}");
    assert_eq!(titles[0], "First", "{titles:?}");
    let reply = wiki.list(&wiki.ada).await;
    assert_eq!(reply.body["items"][0]["authors"][0]["id"], BOB);
}

#[tokio::test]
async fn restoring_needs_an_editor_and_a_revision() {
    let wiki = wiki().await;
    let mut editor = Editor::open(&wiki, &wiki.ada).await;
    editor.write("First", &paragraph(vec![text("one")])).await;
    wiki.titles_until(|t| t.len() == 1).await;
    let eve = wiki
        .api
        .add_and_login("44444444-0000-7000-8000-000000000001", "eve@example.org")
        .await;

    assert_eq!(wiki.restore(&wiki.vic, "1").await.status, 403);
    assert_eq!(wiki.restore(&eve, "1").await.status, 404);
    assert_eq!(wiki.restore(&wiki.bob, "9").await.status, 404);
    assert_eq!(wiki.restore(&wiki.bob, "one").await.status, 400);
    let path = format!("/pages/{}/revisions/1", wiki.page);
    assert_eq!(wiki.api.get(&eve, &path).await.status, 404);
    assert_eq!(wiki.list(&eve).await.status, 404);
    let path = format!("/pages/{}/revisions/1/restore", wiki.page);
    let no_csrf = [("Origin", ORIGIN), ("Cookie", wiki.bob.cookie.as_str())];
    let reply = wiki.api.send("POST", &path, &no_csrf, json!({})).await;
    assert_eq!(reply.status, 403, "{reply:?}");
}

#[tokio::test]
async fn a_view_names_only_the_pages_its_reader_can_open() {
    let wiki = wiki().await;
    let reply = wiki
        .api
        .send(
            "POST",
            &format!("/spaces/{}/pages", wiki.space),
            &wiki.ada.headers(),
            json!({}),
        )
        .await;
    let open = id(&reply);
    wiki.api
        .send(
            "PATCH",
            &format!("/pages/{open}"),
            &wiki.ada.headers(),
            json!({ "title": "Open" }),
        )
        .await;
    let workspace = wiki.api.first_workspace().await;
    let body = json!({ "name": "Private", "default_role": null });
    let path = format!("/workspaces/{workspace}/spaces");
    let private = id(&wiki
        .api
        .send("POST", &path, &wiki.ada.headers(), body)
        .await);
    let reply = wiki
        .api
        .send(
            "POST",
            &format!("/spaces/{private}/pages"),
            &wiki.ada.headers(),
            json!({}),
        )
        .await;
    let hidden = id(&reply);
    wiki.api
        .send(
            "PATCH",
            &format!("/pages/{hidden}"),
            &wiki.ada.headers(),
            json!({ "title": "Hidden" }),
        )
        .await;
    sleep(FLUSHED).await;

    let mut editor = Editor::open(&wiki, &wiki.ada).await;
    let links = paragraph(vec![
        Inline::PageLink { id: open.clone() },
        Inline::PageLink { id: hidden },
    ]);
    editor.write("Links", &links).await;
    wiki.titles_until(|t| t.first().is_some_and(|t| t == "Links"))
        .await;

    let path = format!("/pages/{}/revisions/1", wiki.page);
    let html = wiki.api.get(&wiki.vic, &path).await.body["html"].clone();
    let html = html.as_str().unwrap();
    assert!(html.contains(">Open</a>"), "{html}");
    assert!(!html.contains("Hidden"), "{html}");
    assert!(html.contains("page not found"), "{html}");
}
