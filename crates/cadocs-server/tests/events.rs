//! The events socket of `docs/API.md`, over real sockets.

mod common;

use std::time::Duration;

use cadocs_collab::{Event, PageEvent};
use common::{Api, Login, ORIGIN, Reply, logged_in, start_tuned};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::tungstenite::{Bytes, Error, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const BOB: &str = "22222222-0000-7000-8000-000000000001";
const BOB_EMAIL: &str = "bob@example.org";
const PROTOCOL_ERROR: u16 = 1002;
const SESSION_ENDED: u16 = 4401;
const ACCESS_REVOKED: u16 = 4403;
const WORKSPACE_GONE: u16 = 4404;
const TOO_SLOW: u16 = 4408;

struct Wiki {
    api: Api,
    ada: Login,
    workspace: String,
    space: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

async fn wiki_on(api: Api, ada: Login) -> Wiki {
    let workspace = api.first_workspace().await;
    let mut wiki = Wiki {
        api,
        ada,
        workspace,
        space: String::new(),
    };
    wiki.space = wiki.new_space("Handbook", json!("editor")).await;
    wiki
}

/// A workspace with one space, where every member is an editor.
async fn wiki() -> Wiki {
    let (api, ada) = logged_in().await;
    wiki_on(api, ada).await
}

impl Wiki {
    async fn new_space(&self, name: &str, default_role: Value) -> String {
        let body = json!({ "name": name, "default_role": default_role });
        let path = format!("/workspaces/{}/spaces", self.workspace);
        let reply = self.call("POST", &path, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn new_page_in(&self, space: &str, by: &Login) -> String {
        let path = format!("/spaces/{space}/pages");
        let reply = self.api.send("POST", &path, &by.headers(), json!({})).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn call(&self, method: &str, path: &str, body: Value) -> Reply {
        self.api.send(method, path, &self.ada.headers(), body).await
    }

    async fn delete(&self, path: &str) -> Reply {
        self.api.raw("DELETE", path, &self.ada.headers(), b"").await
    }

    async fn bob(&self) -> Login {
        self.api
            .add_member(&self.workspace, BOB, BOB_EMAIL, "member")
            .await
    }

    fn request(&self, query: &str, by: &Login) -> impl IntoClientRequest + Unpin + use<> {
        let url = format!("ws://{}/ws/events{query}", self.api.addr);
        let mut request = url.into_client_request().unwrap();
        let headers = request.headers_mut();
        headers.insert(COOKIE, by.cookie.parse().unwrap());
        headers.insert("origin", ORIGIN.parse().unwrap());
        request
    }

    async fn socket(&self, by: &Login) -> Socket {
        let query = format!("?workspace={}", self.workspace);
        connect_async(self.request(&query, by)).await.unwrap().0
    }

    async fn refusal(&self, query: &str, by: &Login) -> u16 {
        match connect_async(self.request(query, by)).await {
            Err(Error::Http(response)) => response.status().as_u16(),
            other => panic!("expected a refusal, got {other:?}"),
        }
    }
}

fn page_event(kind: &str, space: &str, page: &str) -> Value {
    json!({ "type": kind, "space": space, "page": page })
}

fn space_event(kind: &str, space: &str) -> Value {
    json!({ "type": kind, "space": space })
}

/// The next event, past the pings.
async fn event(socket: &mut Socket) -> Value {
    timeout(Duration::from_secs(5), async {
        loop {
            match socket.next().await {
                Some(Ok(Message::Text(text))) => return serde_json::from_str(&text).unwrap(),
                Some(Ok(Message::Ping(_) | Message::Pong(_))) => {}
                other => panic!("expected an event, got {other:?}"),
            }
        }
    })
    .await
    .expect("an event")
}

/// Skips what a change of access adds around an event.
async fn event_past_access(socket: &mut Socket) -> Value {
    loop {
        let event = event(socket).await;
        if event != json!({ "type": "access.changed" }) {
            return event;
        }
    }
}

async fn close_code(socket: &mut Socket) -> Option<u16> {
    timeout(Duration::from_secs(5), async {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Close(frame) = message {
                return frame.map(|frame| u16::from(frame.code));
            }
        }
        None
    })
    .await
    .expect("the socket closes")
}

// Tree changes

#[tokio::test]
async fn two_clients_see_each_others_tree_changes() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut ada_socket = wiki.socket(&wiki.ada).await;
    let mut bob_socket = wiki.socket(&bob).await;
    let space = &wiki.space;

    let page = wiki.new_page_in(space, &wiki.ada).await;
    let created = page_event("page.created", space, &page);
    assert_eq!(event(&mut bob_socket).await, created);
    assert_eq!(event(&mut ada_socket).await, created);

    let page = wiki.new_page_in(space, &bob).await;
    let created = page_event("page.created", space, &page);
    assert_eq!(event(&mut ada_socket).await, created);
    assert_eq!(event(&mut bob_socket).await, created);
}

#[tokio::test]
async fn each_change_to_a_page_has_its_event() {
    let wiki = wiki().await;
    let mut socket = wiki.socket(&wiki.ada).await;
    let space = &wiki.space;
    let page = wiki.new_page_in(space, &wiki.ada).await;
    assert_eq!(
        event(&mut socket).await,
        page_event("page.created", space, &page)
    );

    let reply = wiki
        .call("POST", &format!("/pages/{page}/move"), json!({}))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        event(&mut socket).await,
        page_event("page.moved", space, &page)
    );

    let reply = wiki
        .call(
            "PATCH",
            &format!("/pages/{page}"),
            json!({ "title": "Plans" }),
        )
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        event(&mut socket).await,
        page_event("page.renamed", space, &page)
    );

    let reply = wiki.delete(&format!("/pages/{page}")).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(
        event(&mut socket).await,
        page_event("page.trashed", space, &page)
    );

    let reply = wiki
        .call("POST", &format!("/pages/{page}/restore"), json!({}))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        event(&mut socket).await,
        page_event("page.restored", space, &page)
    );

    let reply = wiki.delete(&format!("/pages/{page}")).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(
        event(&mut socket).await,
        page_event("page.trashed", space, &page)
    );
    let reply = wiki.delete(&format!("/pages/{page}/purge")).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(
        event(&mut socket).await,
        page_event("page.purged", space, &page)
    );
}

#[tokio::test]
async fn a_move_across_spaces_is_sent_to_both() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let other = wiki.new_space("Private", Value::Null).await;
    let page = wiki.new_page_in(&wiki.space, &wiki.ada).await;
    let mut ada_socket = wiki.socket(&wiki.ada).await;
    let mut bob_socket = wiki.socket(&bob).await;

    let body = json!({ "space_id": other });
    let reply = wiki
        .call("POST", &format!("/pages/{page}/move"), body)
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");

    let left = page_event("page.moved", &wiki.space, &page);
    let arrived = page_event("page.moved", &other, &page);
    assert_eq!(event(&mut ada_socket).await, left);
    assert_eq!(event(&mut ada_socket).await, arrived);
    // Bob only reads the space it left.
    assert_eq!(event(&mut bob_socket).await, left);
    wiki.new_page_in(&wiki.space, &wiki.ada).await;
    assert_eq!(event(&mut bob_socket).await["type"], "page.created");
}

#[tokio::test]
async fn a_socket_hears_nothing_of_a_space_its_user_cannot_read() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let private = wiki.new_space("Private", Value::Null).await;
    let mut socket = wiki.socket(&bob).await;

    wiki.new_page_in(&private, &wiki.ada).await;
    let visible = wiki.new_page_in(&wiki.space, &wiki.ada).await;

    assert_eq!(
        event(&mut socket).await,
        page_event("page.created", &wiki.space, &visible)
    );
}

// Spaces

#[tokio::test]
async fn each_change_to_a_space_has_its_event() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut socket = wiki.socket(&bob).await;

    let space = wiki.new_space("New", json!("viewer")).await;
    assert_eq!(
        event_past_access(&mut socket).await,
        space_event("space.created", &space)
    );

    let body = json!({ "description": "Plans" });
    let path = format!("/spaces/{space}");
    let reply = wiki.call("PATCH", &path, body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        event_past_access(&mut socket).await,
        space_event("space.updated", &space)
    );

    assert_eq!(wiki.delete(&path).await.status, 204);
    assert_eq!(
        event_past_access(&mut socket).await,
        space_event("space.trashed", &space)
    );

    let path = format!("/spaces/{space}/restore");
    let reply = wiki.call("POST", &path, json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        event_past_access(&mut socket).await,
        space_event("space.restored", &space)
    );
}

// Access

#[tokio::test]
async fn a_new_grant_is_announced_and_opens_the_space() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let private = wiki.new_space("Private", Value::Null).await;
    let mut socket = wiki.socket(&bob).await;

    let body = json!({ "grants": [{ "user_id": BOB, "role": "viewer" }] });
    let path = format!("/spaces/{private}/grants");
    assert_eq!(wiki.call("PUT", &path, body).await.status, 204);
    assert_eq!(
        event(&mut socket).await,
        json!({ "type": "access.changed" })
    );

    let page = wiki.new_page_in(&private, &wiki.ada).await;
    assert_eq!(
        event(&mut socket).await,
        page_event("page.created", &private, &page)
    );
}

#[tokio::test]
async fn a_lost_grant_is_announced_and_closes_the_space() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut socket = wiki.socket(&bob).await;

    let path = format!("/spaces/{}", wiki.space);
    let body = json!({ "default_role": null });
    assert_eq!(wiki.call("PATCH", &path, body).await.status, 200);
    // `space.updated` may not come: the recheck can already have taken the space away.
    while event(&mut socket).await != json!({ "type": "access.changed" }) {}

    let other = wiki.new_space("Open", json!("viewer")).await;
    wiki.new_page_in(&wiki.space, &wiki.ada).await;
    let page = wiki.new_page_in(&other, &wiki.ada).await;
    let expected = page_event("page.created", &other, &page);
    loop {
        let event = event_past_access(&mut socket).await;
        assert_ne!(event["space"], json!(wiki.space), "{event}");
        if event == expected {
            break;
        }
    }
}

#[tokio::test]
async fn a_change_that_leaves_a_socket_as_it_is_says_nothing() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut socket = wiki.socket(&bob).await;

    // A pass over every socket, which concerns nobody here.
    let private = wiki.new_space("Private", Value::Null).await;
    let path = format!("/spaces/{private}/grants");
    let body = json!({ "grants": [] });
    assert_eq!(wiki.call("PUT", &path, body).await.status, 204);

    let page = wiki.new_page_in(&wiki.space, &wiki.ada).await;
    assert_eq!(
        event(&mut socket).await,
        page_event("page.created", &wiki.space, &page)
    );
}

// Closes

#[tokio::test]
async fn removing_a_member_closes_with_4403() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut socket = wiki.socket(&bob).await;
    let path = format!("/workspaces/{}/members/{BOB}", wiki.workspace);
    assert_eq!(wiki.delete(&path).await.status, 204);
    assert_eq!(close_code(&mut socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn disabling_a_user_closes_with_4403() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut socket = wiki.socket(&bob).await;
    let path = format!("/admin/users/{BOB}");
    let reply = wiki.call("PATCH", &path, json!({ "disabled": true })).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn deleting_a_user_closes_with_4401() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut socket = wiki.socket(&bob).await;
    let reply = wiki.delete(&format!("/admin/users/{BOB}")).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(SESSION_ENDED));
}

#[tokio::test]
async fn logging_out_closes_with_4401() {
    let wiki = wiki().await;
    let bob = wiki.bob().await;
    let mut socket = wiki.socket(&bob).await;
    let reply = wiki
        .api
        .raw("POST", "/auth/logout", &bob.headers(), b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(SESSION_ENDED));
}

#[tokio::test]
async fn deleting_a_workspace_closes_with_4404() {
    let (api, ada) = logged_in().await;
    let body = json!({ "name": "Second" });
    let reply = api.send("POST", "/workspaces", &ada.headers(), body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let second = id(&reply);
    let mut wiki = wiki_on(api, ada).await;
    wiki.workspace = second.clone();
    let mut socket = wiki.socket(&wiki.ada).await;

    let path = format!("/workspaces/{second}");
    let body = json!({ "name": "Second" });
    let reply = wiki.call("DELETE", &path, body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(WORKSPACE_GONE));
}

#[tokio::test]
async fn trashing_a_page_or_a_space_leaves_the_socket_open() {
    let wiki = wiki().await;
    let page = wiki.new_page_in(&wiki.space, &wiki.ada).await;
    let mut socket = wiki.socket(&wiki.ada).await;

    assert_eq!(wiki.delete(&format!("/pages/{page}")).await.status, 204);
    assert_eq!(event(&mut socket).await["type"], "page.trashed");
    let path = format!("/spaces/{}", wiki.space);
    assert_eq!(wiki.delete(&path).await.status, 204);
    assert_eq!(event(&mut socket).await["type"], "space.trashed");
}

// The socket itself

#[tokio::test]
async fn a_client_frame_closes_the_socket() {
    let wiki = wiki().await;
    for frame in [
        Message::text("hello"),
        Message::binary(Bytes::from_static(b"hello")),
    ] {
        let mut socket = wiki.socket(&wiki.ada).await;
        socket.send(frame).await.unwrap();
        assert_eq!(close_code(&mut socket).await, Some(PROTOCOL_ERROR));
    }
}

#[tokio::test]
async fn a_pong_is_the_one_thing_a_client_may_send() {
    let wiki = wiki().await;
    let mut socket = wiki.socket(&wiki.ada).await;
    socket.send(Message::Pong(Bytes::new())).await.unwrap();

    let page = wiki.new_page_in(&wiki.space, &wiki.ada).await;
    assert_eq!(
        event(&mut socket).await,
        page_event("page.created", &wiki.space, &page)
    );
}

#[tokio::test]
async fn an_upgrade_is_refused_for_who_has_no_place_there() {
    let wiki = wiki().await;
    let stranger = wiki
        .api
        .add_and_login("33333333-0000-7000-8000-000000000001", "eve@example.org")
        .await;
    let query = format!("?workspace={}", wiki.workspace);

    assert_eq!(wiki.refusal(&query, &stranger).await, 404);
    assert_eq!(wiki.refusal("?workspace=nothing", &wiki.ada).await, 404);
    assert_eq!(wiki.refusal("", &wiki.ada).await, 404);

    let nobody = Login {
        cookie: "session=nothing".to_owned(),
        ..wiki.ada.clone()
    };
    assert_eq!(wiki.refusal(&query, &nobody).await, 401);
}

#[tokio::test]
async fn an_upgrade_from_another_site_is_refused() {
    let wiki = wiki().await;
    let url = format!(
        "ws://{}/ws/events?workspace={}",
        wiki.api.addr, wiki.workspace
    );
    let mut request = url.into_client_request().unwrap();
    let headers = request.headers_mut();
    headers.insert(COOKIE, wiki.ada.cookie.parse().unwrap());
    headers.insert("origin", "https://evil.example.org".parse().unwrap());
    match connect_async(request).await {
        Err(Error::Http(response)) => assert_eq!(response.status().as_u16(), 403),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

#[tokio::test]
async fn events_sockets_count_among_the_open_sockets() {
    let api = start_tuned(ORIGIN, "", &[("CADOCS_MAX_SOCKETS", "1")]).await;
    let reply = api.setup().await;
    let ada = Login::from(&reply);
    let wiki = wiki_on(api, ada).await;

    let socket = wiki.socket(&wiki.ada).await;
    let query = format!("?workspace={}", wiki.workspace);
    assert_eq!(wiki.refusal(&query, &wiki.ada).await, 503);
    drop(socket);
}

/// On one thread, nothing runs between the events: the socket task can only find itself behind.
#[tokio::test]
async fn a_lagging_client_gets_resync() {
    let wiki = wiki().await;
    let mut socket = wiki.socket(&wiki.ada).await;
    // The upgrade is answered before the socket task first reads from its queue.
    tokio::task::yield_now().await;

    for _ in 0..1000 {
        let event = Event::page(PageEvent::Moved, &wiki.space, "page");
        wiki.api.registry.publish(&wiki.workspace, event);
    }

    assert_eq!(event(&mut socket).await, json!({ "type": "resync" }));
    let page = wiki.new_page_in(&wiki.space, &wiki.ada).await;
    assert_eq!(
        event(&mut socket).await,
        page_event("page.created", &wiki.space, &page)
    );
}

#[tokio::test]
async fn the_server_pings_and_a_silent_client_is_closed() {
    let settings = [("CADOCS_PING_SECS", "1"), ("CADOCS_PING_TIMEOUT_SECS", "2")];
    let api = start_tuned(ORIGIN, "", &settings).await;
    let reply = api.setup().await;
    let ada = Login::from(&reply);
    let wiki = wiki_on(api, ada).await;
    let mut socket = wiki.socket(&wiki.ada).await;

    // Not polling the client means no pong goes out, so the server hears silence.
    sleep(Duration::from_secs(3)).await;

    assert_eq!(close_code(&mut socket).await, Some(TOO_SLOW));
}
