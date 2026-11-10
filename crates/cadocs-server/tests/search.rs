//! Search over real sockets: pages written through their socket, indexed by the job, found over
//! REST by those who can read them.

mod common;

use std::time::Duration;

use cadocs_content::ast::{Block, Document};
use cadocs_content::yjs::{FRAGMENT, TITLE, write};
use common::{Api, Login, Reply, logged_in};
use futures_util::SinkExt;
use serde_json::{Value, json};
use tokio::time::{Instant, sleep};
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::encoder::Encode;
use yrs::{Doc, Text, Transact};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const EVE: &str = "22222222-0000-7000-8000-000000000001";
/// Longer than a flush and a reindex, with the delay the tests set.
const PATIENCE: Duration = Duration::from_secs(10);

struct Wiki {
    api: Api,
    ada: Login,
    /// A member, with whatever the default role of a space gives.
    bob: Login,
    workspace: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

fn paragraph(text: &str) -> Block {
    Block::Paragraph(vec![cadocs_content::ast::Inline::Text {
        text: text.to_owned(),
        marks: Default::default(),
    }])
}

async fn wiki() -> Wiki {
    let (api, ada) = logged_in().await;
    let workspace = api.first_workspace().await;
    let bob = api
        .add_member(&workspace, BOB, "bob@example.org", "member")
        .await;
    Wiki {
        api,
        ada,
        bob,
        workspace,
    }
}

impl Wiki {
    async fn space(&self, name: &str, default_role: Value) -> String {
        let body = json!({ "name": name, "default_role": default_role });
        let path = format!("/workspaces/{}/spaces", self.workspace);
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), body)
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn page(&self, space: &str) -> String {
        let path = format!("/spaces/{space}/pages");
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), json!({}))
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    /// Writes a title and content to a page through its socket, as `ada`.
    async fn write(&self, page: &str, title: &str, blocks: Vec<Block>) {
        let doc = Doc::new();
        let title_text = doc.get_or_insert_text(TITLE);
        let fragment = doc.get_or_insert_xml_fragment(FRAGMENT);
        let update = {
            let mut txn = doc.transact_mut();
            title_text.push(&mut txn, title);
            write(&mut txn, &fragment, &Document { blocks });
            txn.encode_update_v1()
        };
        let url = format!("ws://{}/ws/pages/{page}", self.api.addr);
        let mut request = url.into_client_request().unwrap();
        let cookie = self.ada.cookie.parse().unwrap();
        request.headers_mut().insert(COOKIE, cookie);
        let (mut socket, _) = connect_async(request).await.unwrap();
        let frame = YMessage::Sync(SyncMessage::Update(update)).encode_v1();
        socket.send(Message::binary(frame)).await.unwrap();
        // Closing unloads the room, which flushes it.
        socket.close(None).await.unwrap();
    }

    async fn search(&self, by: &Login, query: &str) -> Reply {
        let path = format!("/workspaces/{}/search?{query}", self.workspace);
        self.api.get(by, &path).await
    }

    /// The ids of the results, once `expected` of them come back, or what came last.
    async fn found(&self, by: &Login, query: &str, expected: usize) -> Vec<String> {
        let deadline = Instant::now() + PATIENCE;
        loop {
            let reply = self.search(by, query).await;
            assert_eq!(reply.status, 200, "{reply:?}");
            let ids: Vec<String> = reply.body["items"]
                .as_array()
                .unwrap()
                .iter()
                .map(|item| item["id"].as_str().unwrap().to_owned())
                .collect();
            if ids.len() == expected || Instant::now() > deadline {
                return ids;
            }
            sleep(Duration::from_millis(50)).await;
        }
    }

    async fn language(&self, language: &str) {
        let path = format!("/workspaces/{}", self.workspace);
        let body = json!({ "language": language });
        let reply = self
            .api
            .send("PATCH", &path, &self.ada.headers(), body)
            .await;
        assert_eq!(reply.status, 200, "{reply:?}");
    }
}

#[tokio::test]
async fn a_page_is_found_by_its_title_and_body_with_an_excerpt() {
    let wiki = wiki().await;
    let space = wiki.space("Handbook", json!("viewer")).await;
    let page = wiki.page(&space).await;
    let body = vec![paragraph("How we deployed the servers last week.")];
    wiki.write(&page, "Release notes", body).await;

    assert_eq!(
        wiki.found(&wiki.bob, "q=deploying", 1).await,
        vec![page.clone()]
    );
    assert_eq!(
        wiki.found(&wiki.bob, "q=release+note", 1).await,
        vec![page.clone()]
    );
    assert_eq!(wiki.found(&wiki.bob, "q=serv", 1).await, vec![page.clone()]);

    let reply = wiki.search(&wiki.bob, "q=deploy").await;
    let item = &reply.body["items"][0];
    assert_eq!(item["title"], "Release notes");
    assert_eq!(item["space_id"], space);
    assert_eq!(
        item["excerpt"],
        json!([
            { "text": "How we ", "match": false },
            { "text": "deployed", "match": true },
            { "text": " the servers last week.", "match": false },
        ])
    );
}

#[tokio::test]
async fn a_title_match_ranks_first() {
    let wiki = wiki().await;
    let space = wiki.space("Handbook", json!("viewer")).await;
    let body_only = wiki.page(&space).await;
    let in_title = wiki.page(&space).await;
    let text = vec![paragraph("Something about backups.")];
    wiki.write(&body_only, "Notes", text).await;
    wiki.write(&in_title, "Backups", vec![paragraph("Nothing else.")])
        .await;

    assert_eq!(
        wiki.found(&wiki.ada, "q=backup", 2).await,
        [in_title, body_only]
    );
}

#[tokio::test]
async fn french_stemming_follows_the_language_of_the_workspace() {
    let wiki = wiki().await;
    let space = wiki.space("Handbook", json!("viewer")).await;
    let page = wiki.page(&space).await;
    wiki.write(
        &page,
        "Écuries",
        vec![paragraph("Les chevaux sont rentrés.")],
    )
    .await;
    assert_eq!(
        wiki.found(&wiki.ada, "q=chevaux", 1).await,
        vec![page.clone()]
    );
    assert!(
        wiki.found(&wiki.ada, "q=cheval+", 0).await.is_empty(),
        "English stems leave French plurals alone"
    );

    wiki.language("fr").await;
    assert_eq!(
        wiki.found(&wiki.ada, "q=cheval+", 1).await,
        vec![page.clone()]
    );
    assert_eq!(
        wiki.found(&wiki.ada, "q=ecurie+rentre", 1).await,
        vec![page.clone()],
        "accents do not matter"
    );
}

#[tokio::test]
async fn results_only_come_from_readable_spaces() {
    let wiki = wiki().await;
    let open = wiki.space("Open", json!("viewer")).await;
    let closed = wiki.space("Closed", Value::Null).await;
    let seen = wiki.page(&open).await;
    let hidden = wiki.page(&closed).await;
    wiki.write(&seen, "Seen", vec![paragraph("shared secret")])
        .await;
    wiki.write(&hidden, "Hidden", vec![paragraph("shared secret")])
        .await;

    let mut both = wiki.found(&wiki.ada, "q=secret", 2).await;
    both.sort();
    let mut expected = vec![seen.clone(), hidden.clone()];
    expected.sort();
    assert_eq!(both, expected);
    assert_eq!(
        wiki.found(&wiki.bob, "q=secret", 1).await,
        vec![seen.clone()]
    );

    let in_closed = format!("q=secret&space={closed}");
    assert_eq!(wiki.found(&wiki.ada, &in_closed, 1).await, [hidden]);
    assert!(wiki.found(&wiki.bob, &in_closed, 0).await.is_empty());
}

#[tokio::test]
async fn trashed_pages_and_spaces_are_left_out() {
    let wiki = wiki().await;
    let space = wiki.space("Handbook", json!("viewer")).await;
    let other = wiki.space("Other", json!("viewer")).await;
    let page = wiki.page(&space).await;
    let elsewhere = wiki.page(&other).await;
    wiki.write(&page, "Draft", vec![paragraph("obsolete")])
        .await;
    wiki.write(&elsewhere, "Draft", vec![paragraph("obsolete")])
        .await;
    assert_eq!(wiki.found(&wiki.ada, "q=obsolete", 2).await.len(), 2);

    let headers = wiki.ada.headers();
    let reply = wiki
        .api
        .raw("DELETE", &format!("/pages/{page}"), &headers, b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = wiki
        .api
        .raw("DELETE", &format!("/spaces/{other}"), &headers, b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(wiki.found(&wiki.ada, "q=obsolete", 0).await.is_empty());

    let path = format!("/pages/{page}/restore");
    let reply = wiki.api.send("POST", &path, &headers, json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(wiki.found(&wiki.ada, "q=obsolete", 1).await, [page]);
}

#[tokio::test]
async fn a_stranger_gets_not_found() {
    let wiki = wiki().await;
    let eve = wiki.api.add_and_login(EVE, "eve@example.org").await;
    let reply = wiki.search(&eve, "q=anything").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = wiki.api.get(&wiki.ada, "/workspaces/nope/search?q=a").await;
    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn the_query_is_checked() {
    let wiki = wiki().await;
    for query in ["", "q=", "q=+++", "space=x"] {
        let reply = wiki.search(&wiki.ada, query).await;
        assert_eq!(reply.status, 400, "{query}: {reply:?}");
        assert_eq!(reply.body["error"]["fields"]["q"], "required", "{query}");
    }
    let long = format!("q={}", "a".repeat(201));
    let reply = wiki.search(&wiki.ada, &long).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["q"], "too_long");
    let reply = wiki.search(&wiki.ada, "q=%FF").await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["q"], "invalid");
}

#[tokio::test]
async fn fts_syntax_is_only_text() {
    let wiki = wiki().await;
    let space = wiki.space("Handbook", json!("viewer")).await;
    let page = wiki.page(&space).await;
    wiki.write(&page, "Plain", vec![paragraph("alpha beta")])
        .await;
    assert_eq!(wiki.found(&wiki.ada, "q=alpha", 1).await, [page]);
    for query in [
        "q=%22alpha",
        "q=alpha+OR+%2A",
        "q=title%3Aalpha",
        "q=%3F%21",
        "q=alpha%0A",
        "q=alpha%09",
    ] {
        let reply = wiki.search(&wiki.ada, query).await;
        assert_eq!(reply.status, 200, "{query}: {reply:?}");
    }
}

#[tokio::test]
async fn a_search_needs_a_session() {
    let wiki = wiki().await;
    let path = format!("/workspaces/{}/search?q=a", wiki.workspace);
    let reply = wiki.api.raw("GET", &path, &[], b"").await;
    assert_eq!(reply.status, 401, "{reply:?}");
}

#[tokio::test]
async fn an_image_pasted_into_another_page_is_readable_there() {
    let wiki = wiki().await;
    let closed = wiki.space("Closed", Value::Null).await;
    let open = wiki.space("Open", json!("viewer")).await;
    let source = wiki.page(&closed).await;
    let target = wiki.page(&open).await;
    let path = format!("/pages/{source}/attachments?name=diagram.png");
    let headers = [
        wiki.ada.headers()[0],
        wiki.ada.headers()[1],
        ("content-type", "image/png"),
    ];
    let reply = wiki.api.raw("POST", &path, &headers, b"png").await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let image = id(&reply);
    let download = format!("/attachments/{image}");
    assert_eq!(wiki.api.get(&wiki.bob, &download).await.status, 404);

    let pasted = Block::Image {
        id: image.clone(),
        alt: Some("architecture".into()),
    };
    wiki.write(&target, "Overview", vec![pasted]).await;
    assert_eq!(
        wiki.found(&wiki.bob, "q=architecture", 1).await,
        [target],
        "the alternative text is searchable"
    );
    let reply = wiki.api.get(&wiki.bob, &download).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.bytes, b"png");
}

#[tokio::test]
async fn an_attachment_of_another_workspace_stays_out_of_reach() {
    let wiki = wiki().await;
    let space = wiki.space("Open", json!("viewer")).await;
    let page = wiki.page(&space).await;
    wiki.api
        .sql(
            "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
                 VALUES ('far', 'far', 'Far', 1, 1);
             INSERT INTO attachments (id, workspace_id, file_name, content_type, size, sha256,
                                      created_at)
                 VALUES ('elsewhere', 'far', 'x.png', 'image/png', 1, '', 1);",
        )
        .await;
    let pasted = Block::Image {
        id: "elsewhere".into(),
        alt: Some("borrowed".into()),
    };
    wiki.write(&page, "Borrow", vec![pasted]).await;
    assert_eq!(wiki.found(&wiki.ada, "q=borrowed", 1).await, [page]);
    let refs = wiki
        .api
        .number("SELECT count(*) FROM attachment_refs WHERE attachment_id = 'elsewhere'")
        .await;
    assert_eq!(refs, 0);
}

#[tokio::test]
async fn a_space_after_the_last_word_ends_it() {
    let wiki = wiki().await;
    let space = wiki.space("Handbook", json!("viewer")).await;
    let page = wiki.page(&space).await;
    wiki.write(&page, "Categories", vec![paragraph("listed")])
        .await;
    assert_eq!(wiki.found(&wiki.ada, "q=cat", 1).await, vec![page.clone()]);
    assert!(wiki.found(&wiki.ada, "q=cat+", 0).await.is_empty());
}
