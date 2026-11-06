//! Every trigger of "Reacting to changes" in `docs/PERMISSIONS.md`, on real page sockets.

mod common;

use std::time::Duration;

use common::{Api, Login, ORIGIN, PASSWORD, Reply, logged_in, start_tuned};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::timeout;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const BOB: &str = "22222222-0000-7000-8000-000000000001";
const BOB_EMAIL: &str = "bob@example.org";
const ACCESS_REVOKED: u16 = 4403;
const SESSION_ENDED: u16 = 4401;
const PAGE_GONE: u16 = 4404;
const MODE_CHANGED: u16 = 4409;

struct Wiki {
    api: Api,
    ada: Login,
    workspace: String,
    space: String,
    page: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

async fn wiki_on(api: Api, ada: Login, default_role: Value) -> Wiki {
    let workspace = api.first_workspace().await;
    let mut wiki = Wiki {
        api,
        ada,
        workspace,
        space: String::new(),
        page: String::new(),
    };
    wiki.space = wiki.new_space(default_role).await;
    wiki.page = wiki.new_page(json!({})).await;
    wiki
}

/// A workspace with one page, in a space with this default role.
async fn wiki(default_role: Value) -> Wiki {
    let (api, ada) = logged_in().await;
    wiki_on(api, ada, default_role).await
}

impl Wiki {
    async fn new_space(&self, default_role: Value) -> String {
        let body = json!({ "name": "Handbook", "default_role": default_role });
        let path = format!("/workspaces/{}/spaces", self.workspace);
        let reply = self.call("POST", &path, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn new_page(&self, body: Value) -> String {
        let path = format!("/spaces/{}/pages", self.space);
        let reply = self.call("POST", &path, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    /// A request by the owner.
    async fn call(&self, method: &str, path: &str, body: Value) -> Reply {
        self.api.send(method, path, &self.ada.headers(), body).await
    }

    async fn delete(&self, path: &str) -> Reply {
        self.api.raw("DELETE", path, &self.ada.headers(), b"").await
    }

    async fn member(&self, role: &str) -> Login {
        self.api
            .add_member(&self.workspace, BOB, BOB_EMAIL, role)
            .await
    }

    async fn socket_on(&self, page: &str, by: &Login) -> Socket {
        let url = format!("ws://{}/ws/pages/{page}", self.api.addr);
        let mut request = url.into_client_request().unwrap();
        let headers = request.headers_mut();
        headers.insert(COOKIE, by.cookie.parse().unwrap());
        headers.insert("origin", ORIGIN.parse().unwrap());
        connect_async(request).await.unwrap().0
    }

    async fn socket(&self, by: &Login) -> Socket {
        self.socket_on(&self.page, by).await
    }

    async fn set_default_role(&self, role: Value) {
        let path = format!("/spaces/{}", self.space);
        let reply = self
            .call("PATCH", &path, json!({ "default_role": role }))
            .await;
        assert_eq!(reply.status, 200, "{reply:?}");
    }

    async fn set_grants(&self, grants: Value) {
        let path = format!("/spaces/{}/grants", self.space);
        let reply = self.call("PUT", &path, json!({ "grants": grants })).await;
        assert_eq!(reply.status, 204, "{reply:?}");
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
    .expect("the socket must close")
}

/// The socket is still open once whatever was queued had the time to run.
async fn stays_open(socket: &mut Socket) {
    let quiet = timeout(Duration::from_millis(300), async {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Close(frame) = message {
                let code = frame.map(|frame| frame.code).unwrap_or(CloseCode::Status);
                panic!("closed with {code:?}");
            }
        }
    });
    assert!(quiet.await.is_err(), "the socket must stay open");
}

// Roles

#[tokio::test]
async fn revoking_a_grant_closes_with_4403() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("member").await;
    wiki.set_grants(json!([{ "user_id": BOB, "role": "editor" }]))
        .await;
    let mut socket = wiki.socket(&bob).await;
    wiki.set_grants(json!([])).await;
    assert_eq!(close_code(&mut socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn a_lower_grant_that_keeps_the_page_readable_closes_with_4409() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("member").await;
    wiki.set_grants(json!([{ "user_id": BOB, "role": "editor" }]))
        .await;
    let mut socket = wiki.socket(&bob).await;
    wiki.set_grants(json!([{ "user_id": BOB, "role": "viewer" }]))
        .await;
    assert_eq!(close_code(&mut socket).await, Some(MODE_CHANGED));
}

#[tokio::test]
async fn a_higher_grant_closes_with_4409_too() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("member").await;
    wiki.set_grants(json!([{ "user_id": BOB, "role": "viewer" }]))
        .await;
    let mut socket = wiki.socket(&bob).await;
    wiki.set_grants(json!([{ "user_id": BOB, "role": "editor" }]))
        .await;
    assert_eq!(close_code(&mut socket).await, Some(MODE_CHANGED));
}

#[tokio::test]
async fn a_grant_that_keeps_the_mode_leaves_the_socket_open() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("member").await;
    wiki.set_grants(json!([{ "user_id": BOB, "role": "editor" }]))
        .await;
    let mut socket = wiki.socket(&bob).await;
    wiki.set_grants(json!([{ "user_id": BOB, "role": "admin" }]))
        .await;
    stays_open(&mut socket).await;
}

#[tokio::test]
async fn changing_the_default_role_recomputes_sockets() {
    let wiki = wiki(json!("editor")).await;
    let bob = wiki.member("member").await;
    let mut socket = wiki.socket(&bob).await;
    wiki.set_default_role(json!("viewer")).await;
    assert_eq!(close_code(&mut socket).await, Some(MODE_CHANGED));

    let mut socket = wiki.socket(&bob).await;
    wiki.set_default_role(Value::Null).await;
    assert_eq!(close_code(&mut socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn a_lower_workspace_role_recomputes_sockets() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let mut socket = wiki.socket(&bob).await;
    let path = format!("/workspaces/{}/members/{BOB}", wiki.workspace);
    let reply = wiki.call("PATCH", &path, json!({ "role": "member" })).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn a_recompute_closes_only_the_sockets_it_concerns() {
    let wiki = wiki(json!("editor")).await;
    let bob = wiki.member("member").await;
    let mut owner = wiki.socket(&wiki.ada).await;
    let mut member = wiki.socket(&bob).await;
    wiki.set_default_role(json!("viewer")).await;
    assert_eq!(close_code(&mut member).await, Some(MODE_CHANGED));
    stays_open(&mut owner).await;
}

// Moves

#[tokio::test]
async fn moving_to_another_space_recomputes_the_page_and_its_descendants() {
    let wiki = wiki(json!("editor")).await;
    let bob = wiki.member("member").await;
    let child = wiki.new_page(json!({ "parent_id": wiki.page })).await;
    let private = wiki.new_space(Value::Null).await;
    let mut parent_socket = wiki.socket(&bob).await;
    let mut child_socket = wiki.socket_on(&child, &bob).await;

    let path = format!("/pages/{}/move", wiki.page);
    let reply = wiki
        .call("POST", &path, json!({ "space_id": private }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(close_code(&mut parent_socket).await, Some(ACCESS_REVOKED));
    assert_eq!(close_code(&mut child_socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn moving_to_a_read_only_space_closes_with_4409() {
    let wiki = wiki(json!("editor")).await;
    let bob = wiki.member("member").await;
    let readable = wiki.new_space(json!("viewer")).await;
    let mut socket = wiki.socket(&bob).await;
    let path = format!("/pages/{}/move", wiki.page);
    let body = json!({ "space_id": readable });
    assert_eq!(wiki.call("POST", &path, body).await.status, 200);
    assert_eq!(close_code(&mut socket).await, Some(MODE_CHANGED));
}

// Accounts

#[tokio::test]
async fn removing_a_member_closes_with_4403() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let mut socket = wiki.socket(&bob).await;
    let path = format!("/workspaces/{}/members/{BOB}", wiki.workspace);
    assert_eq!(wiki.delete(&path).await.status, 204);
    assert_eq!(close_code(&mut socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn disabling_a_user_closes_with_4403() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let mut socket = wiki.socket(&bob).await;
    let path = format!("/admin/users/{BOB}");
    let reply = wiki.call("PATCH", &path, json!({ "disabled": true })).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(ACCESS_REVOKED));
}

#[tokio::test]
async fn deleting_a_user_closes_with_4401() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let mut socket = wiki.socket(&bob).await;
    let reply = wiki.delete(&format!("/admin/users/{BOB}")).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(SESSION_ENDED));
}

#[tokio::test]
async fn deleting_your_own_account_closes_with_4401() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let mut socket = wiki.socket(&bob).await;
    let reply = wiki.api.raw("DELETE", "/me", &bob.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(SESSION_ENDED));
}

// Sessions

#[tokio::test]
async fn logging_out_closes_the_sockets_of_that_session_only() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let reply = wiki.api.login(BOB_EMAIL, PASSWORD).await;
    let phone = Login::from(&reply);
    let mut laptop_socket = wiki.socket(&bob).await;
    let mut phone_socket = wiki.socket(&phone).await;

    let reply = wiki
        .api
        .raw("POST", "/auth/logout", &bob.headers(), b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut laptop_socket).await, Some(SESSION_ENDED));
    stays_open(&mut phone_socket).await;
}

#[tokio::test]
async fn changing_a_password_closes_the_other_sessions() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let reply = wiki.api.login(BOB_EMAIL, PASSWORD).await;
    let phone = Login::from(&reply);
    let mut laptop_socket = wiki.socket(&bob).await;
    let mut phone_socket = wiki.socket(&phone).await;

    let body = json!({ "password": "another long password", "current_password": PASSWORD });
    let reply = wiki.api.send("PATCH", "/me", &bob.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(close_code(&mut phone_socket).await, Some(SESSION_ENDED));
    stays_open(&mut laptop_socket).await;
}

#[tokio::test]
async fn using_a_reset_link_closes_every_session() {
    let wiki = wiki(Value::Null).await;
    let bob = wiki.member("admin").await;
    let mut socket = wiki.socket(&bob).await;

    let path = format!("/admin/users/{BOB}/password-reset");
    let reply = wiki.api.raw("POST", &path, &wiki.ada.headers(), b"").await;
    let link = reply.body["link"].as_str().unwrap();
    let token = link.rsplit('/').next().unwrap();
    let path = format!("/auth/password-reset/{token}");
    let body = json!({ "password": "another long password" });
    let reply = wiki.api.send("POST", &path, &[], body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(SESSION_ENDED));
}

#[tokio::test]
async fn an_expired_session_closes_without_any_request() {
    let api = start_tuned(ORIGIN, "", &[("CADOCS_SOCKET_RECHECK_SECS", "1")]).await;
    let reply = api.setup().await;
    let ada = Login::from(&reply);
    let wiki = wiki_on(api, ada, Value::Null).await;
    let mut socket = wiki.socket(&wiki.ada).await;
    wiki.api.sql("UPDATE sessions SET expires_at = 1").await;
    assert_eq!(close_code(&mut socket).await, Some(SESSION_ENDED));
}

#[tokio::test]
async fn a_change_made_beside_the_server_is_found_by_the_timer() {
    let api = start_tuned(ORIGIN, "", &[("CADOCS_SOCKET_RECHECK_SECS", "1")]).await;
    let reply = api.setup().await;
    let ada = Login::from(&reply);
    let wiki = wiki_on(api, ada, Value::Null).await;
    let mut socket = wiki.socket(&wiki.ada).await;
    wiki.api.sql("DELETE FROM sessions").await;
    assert_eq!(close_code(&mut socket).await, Some(SESSION_ENDED));
}

// Trash

#[tokio::test]
async fn trashing_a_page_closes_it_and_its_descendants_with_4404() {
    let wiki = wiki(Value::Null).await;
    let child = wiki.new_page(json!({ "parent_id": wiki.page })).await;
    let mut parent_socket = wiki.socket(&wiki.ada).await;
    let mut child_socket = wiki.socket_on(&child, &wiki.ada).await;
    let reply = wiki.delete(&format!("/pages/{}", wiki.page)).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut parent_socket).await, Some(PAGE_GONE));
    assert_eq!(close_code(&mut child_socket).await, Some(PAGE_GONE));
}

#[tokio::test]
async fn trashing_a_space_closes_its_pages_with_4404() {
    let wiki = wiki(Value::Null).await;
    let mut socket = wiki.socket(&wiki.ada).await;
    let reply = wiki.delete(&format!("/spaces/{}", wiki.space)).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(PAGE_GONE));
}

#[tokio::test]
async fn deleting_a_workspace_closes_its_pages_with_4404() {
    let (api, ada) = logged_in().await;
    let body = json!({ "name": "Second" });
    let reply = api.send("POST", "/workspaces", &ada.headers(), body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let second = id(&reply);
    let mut wiki = wiki_on(api, ada, Value::Null).await;
    wiki.workspace = second.clone();
    wiki.space = wiki.new_space(Value::Null).await;
    wiki.page = wiki.new_page(json!({})).await;
    let mut socket = wiki.socket(&wiki.ada).await;

    let path = format!("/workspaces/{second}");
    let reply = wiki
        .call("DELETE", &path, json!({ "name": "Second" }))
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(close_code(&mut socket).await, Some(PAGE_GONE));
}

#[tokio::test]
async fn a_page_that_was_purged_closes_with_4404_for_a_socket_that_outlived_the_trash() {
    let wiki = wiki(Value::Null).await;
    let page = format!("/pages/{}", wiki.page);
    let mut socket = wiki.socket(&wiki.ada).await;
    wiki.delete(&page).await;
    assert_eq!(close_code(&mut socket).await, Some(PAGE_GONE));
    wiki.delete(&format!("{page}/purge")).await;
    assert_eq!(wiki.api.get(&wiki.ada, &page).await.status, 404);
}
