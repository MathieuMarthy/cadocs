//! The page socket behind sessions, and titles: real roles from the database, over real sockets.

mod common;

use std::time::Duration;

use cadocs_store::MAX_TITLE_CHARS;
use common::{Api, Login, ORIGIN, Reply, logged_in};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::header::{COOKIE, ORIGIN as ORIGIN_HEADER};
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, StateVector, Text, Transact, Update};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const BOB: &str = "11111111-0000-7000-8000-000000000001";
/// Past the default flush interval.
const FLUSHED: Duration = Duration::from_millis(900);

struct Wiki {
    api: Api,
    ada: Login,
    workspace: String,
    page: String,
}

/// A workspace with one page, in a space with this default role.
async fn wiki(default_role: Value) -> Wiki {
    let (api, ada) = logged_in().await;
    let workspace = api.first_workspace().await;
    let body = json!({ "name": "Handbook", "default_role": default_role });
    let path = format!("/workspaces/{workspace}/spaces");
    let reply = api.send("POST", &path, &ada.headers(), body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let space = id(&reply);
    let path = format!("/spaces/{space}/pages");
    let reply = api.send("POST", &path, &ada.headers(), json!({})).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let page = id(&reply);
    Wiki {
        api,
        ada,
        workspace,
        page,
    }
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

impl Wiki {
    fn request(&self, page: &str, by: Option<&Login>) -> Request {
        let url = format!("ws://{}/ws/pages/{page}", self.api.addr);
        let mut request = url.into_client_request().unwrap();
        if let Some(login) = by {
            let cookie = login.cookie.parse().unwrap();
            request.headers_mut().insert(COOKIE, cookie);
        }
        request
    }

    async fn socket(&self, by: &Login) -> Socket {
        connect_async(self.request(&self.page, Some(by)))
            .await
            .unwrap()
            .0
    }

    async fn member(&self, role: &str) -> Login {
        let email = "bob@example.org";
        self.api.add_member(&self.workspace, BOB, email, role).await
    }

    async fn patch(&self, by: &Login, body: Value) -> Reply {
        let path = format!("/pages/{}", self.page);
        self.api.send("PATCH", &path, &by.headers(), body).await
    }

    async fn row(&self) -> Reply {
        self.api
            .get(&self.ada, &format!("/pages/{}", self.page))
            .await
    }

    async fn trash(&self) {
        let path = format!("/pages/{}", self.page);
        let reply = self
            .api
            .raw("DELETE", &path, &self.ada.headers(), b"")
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }

    async fn purge(&self) {
        self.trash().await;
        let path = format!("/pages/{}/purge", self.page);
        let reply = self
            .api
            .raw("DELETE", &path, &self.ada.headers(), b"")
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }

    async fn stored_rows(&self) -> i64 {
        let page = &self.page;
        self.api
            .number(&format!(
                "SELECT (SELECT count(*) FROM doc_updates WHERE page_id = '{page}')
                      + (SELECT count(*) FROM doc_snapshots WHERE page_id = '{page}')"
            ))
            .await
    }
}

async fn refused(request: Request) -> u16 {
    match connect_async(request).await {
        Err(WsError::Http(response)) => response.status().as_u16(),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

fn title_frame(text: &str) -> Message {
    let doc = Doc::new();
    let title = doc.get_or_insert_text("title");
    let mut txn = doc.transact_mut();
    title.push(&mut txn, text);
    let update = txn.encode_update_v1();
    Message::binary(YMessage::Sync(SyncMessage::Update(update)).encode_v1())
}

fn ask_for_everything() -> Message {
    let step = SyncMessage::SyncStep1(StateVector::default());
    Message::binary(YMessage::Sync(step).encode_v1())
}

/// Reads frames, applying what they carry, until the title is `expected`.
async fn wait_for_title(socket: &mut Socket, expected: &str) {
    let doc = Doc::new();
    let title = doc.get_or_insert_text("title");
    timeout(Duration::from_secs(5), async {
        loop {
            let Message::Binary(frame) = socket.next().await.unwrap().unwrap() else {
                continue;
            };
            if let YMessage::Sync(SyncMessage::SyncStep2(update) | SyncMessage::Update(update)) =
                YMessage::decode_v1(&frame).unwrap()
            {
                let update = Update::decode_v1(&update).unwrap();
                doc.transact_mut().apply_update(update).unwrap();
            }
            if title.get_string(&doc.transact()) == expected {
                return;
            }
        }
    })
    .await
    .expect("the title must arrive");
}

async fn close_code(socket: &mut Socket) -> Option<CloseCode> {
    timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Close(frame) = message {
                return frame.map(|frame| frame.code);
            }
        }
        None
    })
    .await
    .expect("the socket must close")
}

async fn got_auth_frame(socket: &mut Socket) -> bool {
    timeout(Duration::from_secs(5), async {
        loop {
            if let Message::Binary(frame) = socket.next().await.unwrap().unwrap()
                && frame[0] == 2
            {
                return;
            }
        }
    })
    .await
    .is_ok()
}

// Before the upgrade

#[tokio::test]
async fn a_socket_needs_a_session() {
    let wiki = wiki(Value::Null).await;

    assert_eq!(refused(wiki.request(&wiki.page, None)).await, 401);

    let stale = Login {
        cookie: "session=stale".to_owned(),
        csrf: String::new(),
    };
    assert_eq!(refused(wiki.request(&wiki.page, Some(&stale))).await, 401);
}

#[tokio::test]
async fn a_logged_out_session_cannot_open_a_socket() {
    let wiki = wiki(Value::Null).await;
    let path = "/auth/logout";
    let reply = wiki.api.raw("POST", path, &wiki.ada.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");

    assert_eq!(
        refused(wiki.request(&wiki.page, Some(&wiki.ada))).await,
        401
    );
}

#[tokio::test]
async fn the_origin_comes_before_the_session() {
    let wiki = wiki(Value::Null).await;

    let mut foreign = wiki.request(&wiki.page, Some(&wiki.ada));
    let evil = "https://evil.example".parse().unwrap();
    foreign.headers_mut().insert(ORIGIN_HEADER, evil);
    assert_eq!(refused(foreign).await, 403);

    let mut anonymous = wiki.request(&wiki.page, None);
    let evil = "https://evil.example".parse().unwrap();
    anonymous.headers_mut().insert(ORIGIN_HEADER, evil);
    assert_eq!(refused(anonymous).await, 403);

    let mut public = wiki.request(&wiki.page, Some(&wiki.ada));
    public
        .headers_mut()
        .insert(ORIGIN_HEADER, ORIGIN.parse().unwrap());
    connect_async(public).await.expect("our own origin");
}

#[tokio::test]
async fn an_unknown_page_is_not_found() {
    let wiki = wiki(Value::Null).await;
    assert_eq!(refused(wiki.request("nope", Some(&wiki.ada))).await, 404);
}

#[tokio::test]
async fn a_page_the_user_cannot_read_is_not_found() {
    let wiki = wiki(Value::Null).await;
    let member = wiki.member("member").await;
    assert_eq!(refused(wiki.request(&wiki.page, Some(&member))).await, 404);

    let stranger = wiki.api.add_and_login("stranger", "s@example.org").await;
    assert_eq!(
        refused(wiki.request(&wiki.page, Some(&stranger))).await,
        404
    );
}

#[tokio::test]
async fn a_trashed_page_is_not_found() {
    let wiki = wiki(Value::Null).await;
    wiki.trash().await;
    assert_eq!(
        refused(wiki.request(&wiki.page, Some(&wiki.ada))).await,
        404
    );
}

#[tokio::test]
async fn the_old_route_is_gone() {
    let wiki = wiki(Value::Null).await;
    let url = format!("ws://{}/ws/{}", wiki.api.addr, wiki.page);
    let mut request = url.into_client_request().unwrap();
    let cookie = wiki.ada.cookie.parse().unwrap();
    request.headers_mut().insert(COOKIE, cookie);
    assert!(connect_async(request).await.is_err());
}

// Mode

#[tokio::test]
async fn an_editor_writes() {
    let wiki = wiki(json!("editor")).await;
    let member = wiki.member("member").await;
    let mut socket = wiki.socket(&member).await;
    socket.send(title_frame("By an editor")).await.unwrap();

    let mut witness = wiki.socket(&wiki.ada).await;
    witness.send(ask_for_everything()).await.unwrap();
    wait_for_title(&mut witness, "By an editor").await;
}

#[tokio::test]
async fn a_viewer_and_a_commenter_are_read_only() {
    for role in ["viewer", "commenter"] {
        let wiki = wiki(json!(role)).await;
        let member = wiki.member("member").await;
        let mut socket = wiki.socket(&member).await;

        socket.send(title_frame("Refused")).await.unwrap();

        assert!(got_auth_frame(&mut socket).await, "{role}");
        sleep(FLUSHED).await;
        assert_eq!(wiki.stored_rows().await, 0, "{role}");
    }
}

// Title

#[tokio::test]
async fn a_flush_copies_the_title_to_the_page() {
    let wiki = wiki(json!("editor")).await;
    let member = wiki.member("member").await;
    let mut socket = wiki.socket(&member).await;
    let before = wiki.row().await.body["updated_at"].as_i64().unwrap();

    socket.send(title_frame("From the socket")).await.unwrap();
    sleep(FLUSHED).await;

    let row = wiki.row().await;
    assert_eq!(row.body["title"], "From the socket");
    assert_eq!(row.body["updated_by"], BOB);
    assert!(row.body["updated_at"].as_i64().unwrap() > before);
}

#[tokio::test]
async fn patch_reaches_open_clients_and_the_page() {
    let wiki = wiki(json!("editor")).await;
    let mut socket = wiki.socket(&wiki.ada).await;
    let member = wiki.member("member").await;

    let reply = wiki.patch(&member, json!({ "title": "  Renamed  " })).await;

    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["title"], "Renamed");
    assert_eq!(reply.body["id"], wiki.page.as_str());
    wait_for_title(&mut socket, "Renamed").await;
    sleep(FLUSHED).await;
    let row = wiki.row().await;
    assert_eq!(row.body["title"], "Renamed");
    assert_eq!(row.body["updated_by"], BOB);
}

#[tokio::test]
async fn patch_works_with_nobody_on_the_page() {
    let wiki = wiki(Value::Null).await;

    let reply = wiki
        .patch(&wiki.ada, json!({ "title": "Quiet rename" }))
        .await;

    assert_eq!(reply.status, 200, "{reply:?}");
    sleep(FLUSHED).await;
    assert_eq!(wiki.row().await.body["title"], "Quiet rename");
    let mut socket = wiki.socket(&wiki.ada).await;
    socket.send(ask_for_everything()).await.unwrap();
    wait_for_title(&mut socket, "Quiet rename").await;
}

#[tokio::test]
async fn patch_can_empty_a_title() {
    let wiki = wiki(Value::Null).await;
    let reply = wiki.patch(&wiki.ada, json!({ "title": "Gone soon" })).await;
    assert_eq!(reply.status, 200, "{reply:?}");

    let reply = wiki.patch(&wiki.ada, json!({ "title": "" })).await;

    assert_eq!(reply.status, 200, "{reply:?}");
    sleep(FLUSHED).await;
    assert_eq!(wiki.row().await.body["title"], "");
}

#[tokio::test]
async fn patch_needs_an_editor() {
    let wiki = wiki(json!("commenter")).await;
    let member = wiki.member("member").await;

    let reply = wiki.patch(&member, json!({ "title": "No" })).await;

    assert_eq!(reply.status, 403, "{reply:?}");
    assert_eq!(reply.code(), "forbidden");
}

#[tokio::test]
async fn patch_hides_a_page_from_who_cannot_read_it() {
    let wiki = wiki(Value::Null).await;
    let member = wiki.member("member").await;

    let reply = wiki.patch(&member, json!({ "title": "No" })).await;

    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn patch_refuses_a_trashed_page() {
    let wiki = wiki(Value::Null).await;
    wiki.trash().await;

    let reply = wiki.patch(&wiki.ada, json!({ "title": "No" })).await;

    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn patch_refuses_a_long_title() {
    let wiki = wiki(Value::Null).await;
    let long = "x".repeat(MAX_TITLE_CHARS + 1);

    let reply = wiki.patch(&wiki.ada, json!({ "title": long })).await;

    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["title"], "too_long");
}

#[tokio::test]
async fn patch_needs_the_csrf_token() {
    let wiki = wiki(Value::Null).await;
    let path = format!("/pages/{}", wiki.page);
    let headers = [("cookie", wiki.ada.cookie.as_str())];

    let reply = wiki
        .api
        .send("PATCH", &path, &headers, json!({ "title": "No" }))
        .await;

    assert_eq!(reply.status, 403, "{reply:?}");
}

// Foreign key

#[tokio::test]
async fn purging_a_page_deletes_its_document() {
    let wiki = wiki(Value::Null).await;
    let reply = wiki.patch(&wiki.ada, json!({ "title": "Stored" })).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    sleep(FLUSHED).await;
    assert!(wiki.stored_rows().await > 0);

    wiki.purge().await;

    assert_eq!(wiki.stored_rows().await, 0);
}

/// Nothing closes these sockets yet (step 8), so the next flush finds out.
#[tokio::test]
async fn an_edit_on_a_purged_page_closes_the_socket() {
    let wiki = wiki(Value::Null).await;
    let mut socket = wiki.socket(&wiki.ada).await;
    wiki.purge().await;

    socket.send(title_frame("Too late")).await.unwrap();

    assert_eq!(close_code(&mut socket).await, Some(CloseCode::from(4404)));
}
