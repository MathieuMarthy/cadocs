//! The space actions of PERMISSIONS.md, cell by cell, over REST and the page socket: one
//! server per role, and each action of the matrix tried by a member holding that role.
//! Comments and public shares come in V1, so their rows wait for their routes.

mod common;

use std::time::Duration;

use common::{Api, Login, ORIGIN, Reply, logged_in, start_tuned};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use tokio::net::TcpStream;
use tokio::time::{Instant, sleep, timeout};
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::COOKIE;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::encoder::Encode;
use yrs::{Doc, Text, Transact};

type Socket = WebSocketStream<MaybeTlsStream<TcpStream>>;

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const PATIENCE: Duration = Duration::from_secs(10);

/// The roles a member can hold on a space, `None` for no access, lowest first.
const ROLES: [Option<&str>; 5] = [
    None,
    Some("viewer"),
    Some("commenter"),
    Some("editor"),
    Some("admin"),
];

/// The lowest role a row of the matrix needs.
#[derive(Clone, Copy, PartialEq, PartialOrd, Debug)]
enum Needs {
    Viewer = 1,
    Editor = 3,
    Admin = 4,
}

fn rank(role: Option<&str>) -> usize {
    ROLES.iter().position(|r| *r == role).unwrap()
}

/// What a member with `role` must get: the success status, `403` when they can read the
/// target, `404` when they cannot, so nothing reveals it (API.md, hidden resources).
fn expected(role: Option<&str>, needs: Needs, success: u16) -> u16 {
    expected_hidden(role, Needs::Viewer, needs, success)
}

/// As `expected`, for a target only `sees` and above can read: the trash is hidden from
/// who cannot list it, and a trashed space from all but its admins.
fn expected_hidden(role: Option<&str>, sees: Needs, needs: Needs, success: u16) -> u16 {
    match rank(role) {
        r if r >= needs as usize => success,
        r if r >= sees as usize => 403,
        _ => 404,
    }
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

/// A space only `ada` reads, where `bob` holds `role`, with what each action needs.
struct Wiki {
    api: Api,
    ada: Login,
    bob: Login,
    space: String,
    page: String,
    attachment: String,
}

impl Wiki {
    async fn new(role: Option<&str>) -> Self {
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
        let path = format!("/workspaces/{workspace}/spaces");
        let body = json!({ "name": "Handbook", "default_role": null });
        let reply = api.send("POST", &path, &ada.headers(), body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        let space = id(&reply);
        let mut wiki = Self {
            api,
            ada,
            bob,
            space,
            page: String::new(),
            attachment: String::new(),
        };
        if let Some(role) = role {
            let reply = wiki.grants(&wiki.ada, role).await;
            assert_eq!(reply.status, 204, "{reply:?}");
        }
        wiki.page = wiki.new_page().await;
        let reply = wiki.upload(&wiki.ada).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        wiki.attachment = id(&reply);
        wiki.first_revision().await;
        wiki
    }

    async fn grants(&self, by: &Login, role: &str) -> Reply {
        let body = json!({ "grants": [{ "user_id": BOB, "role": role }] });
        let path = format!("/spaces/{}/grants", self.space);
        self.api.send("PUT", &path, &by.headers(), body).await
    }

    async fn new_page(&self) -> String {
        let path = format!("/spaces/{}/pages", self.space);
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), json!({}))
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn trashed_page(&self) -> String {
        let page = self.new_page().await;
        let reply = self
            .call(&self.ada, "DELETE", &format!("/pages/{page}"))
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");
        page
    }

    async fn upload(&self, by: &Login) -> Reply {
        let path = format!("/pages/{}/attachments?name=notes.txt", self.page);
        let mut headers = by.headers().to_vec();
        headers.push(("content-type", "text/plain"));
        self.api.raw("POST", &path, &headers, b"some notes").await
    }

    /// Renames the page and waits for the revision job to record it.
    async fn first_revision(&self) {
        let path = format!("/pages/{}", self.page);
        let body = json!({ "title": "First" });
        let reply = self
            .api
            .send("PATCH", &path, &self.ada.headers(), body)
            .await;
        assert_eq!(reply.status, 200, "{reply:?}");
        let deadline = Instant::now() + PATIENCE;
        let path = format!("/pages/{}/revisions", self.page);
        while self.api.get(&self.ada, &path).await.body["items"]
            .as_array()
            .is_none_or(Vec::is_empty)
        {
            assert!(Instant::now() < deadline, "no revision was taken");
            sleep(Duration::from_millis(50)).await;
        }
    }

    async fn call(&self, by: &Login, method: &str, path: &str) -> Reply {
        self.api.raw(method, path, &by.headers(), b"").await
    }

    async fn send(&self, by: &Login, method: &str, path: &str, body: Value) -> Reply {
        self.api.send(method, path, &by.headers(), body).await
    }

    fn socket_request(&self, page: &str, by: &Login) -> impl IntoClientRequest + Unpin + use<> {
        let url = format!("ws://{}/ws/pages/{page}", self.api.addr);
        let mut request = url.into_client_request().unwrap();
        request
            .headers_mut()
            .insert(COOKIE, by.cookie.parse().unwrap());
        request
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

/// Whether the server refuses a write with its permission frame.
async fn refuses_writes(socket: &mut Socket) -> bool {
    timeout(Duration::from_secs(2), async {
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

/// One outcome per cell, gathered so a failure lists every wrong cell at once.
#[derive(Default)]
struct Cells(Vec<String>);

impl Cells {
    fn check(&mut self, role: Option<&str>, action: &str, got: &Reply, want: u16) {
        if got.status != want {
            self.0.push(format!(
                "{role:?} {action}: got {} {}, want {want}",
                got.status,
                got.code()
            ));
        }
    }

    fn assert_empty(self) {
        assert!(self.0.is_empty(), "\n{}", self.0.join("\n"));
    }
}

#[tokio::test]
async fn each_space_action_follows_the_matrix_over_rest() {
    let mut cells = Cells::default();
    for role in ROLES {
        let wiki = Wiki::new(role).await;
        let (bob, space, page) = (&wiki.bob, wiki.space.clone(), wiki.page.clone());
        let check = |cells: &mut Cells, action: &str, got: &Reply, needs, success| {
            cells.check(role, action, got, expected(role, needs, success));
        };

        // Read the space, tree and grants.
        let got = wiki.call(bob, "GET", &format!("/spaces/{space}")).await;
        check(&mut cells, "read the space", &got, Needs::Viewer, 200);
        let got = wiki
            .call(bob, "GET", &format!("/spaces/{space}/tree"))
            .await;
        check(&mut cells, "read the tree", &got, Needs::Viewer, 200);
        let got = wiki
            .call(bob, "GET", &format!("/spaces/{space}/grants"))
            .await;
        check(&mut cells, "read the grants", &got, Needs::Viewer, 200);

        // Read a page, export it, read its revisions, download attachments.
        let got = wiki.call(bob, "GET", &format!("/pages/{page}")).await;
        check(&mut cells, "read a page", &got, Needs::Viewer, 200);
        let got = wiki
            .call(bob, "GET", &format!("/pages/{page}/export"))
            .await;
        check(&mut cells, "export", &got, Needs::Viewer, 200);
        let got = wiki
            .call(bob, "GET", &format!("/pages/{page}/revisions"))
            .await;
        check(&mut cells, "list revisions", &got, Needs::Viewer, 200);
        let got = wiki
            .call(bob, "GET", &format!("/pages/{page}/revisions/1"))
            .await;
        check(&mut cells, "view a revision", &got, Needs::Viewer, 200);
        let path = format!("/attachments/{}", wiki.attachment);
        let got = wiki.call(bob, "GET", &path).await;
        check(&mut cells, "download", &got, Needs::Viewer, 200);

        // Create, update, move, trash or restore a page.
        let path = format!("/spaces/{space}/pages");
        let got = wiki.send(bob, "POST", &path, json!({})).await;
        check(&mut cells, "create a page", &got, Needs::Editor, 201);
        let body = json!({ "title": "Renamed" });
        let got = wiki
            .send(bob, "PATCH", &format!("/pages/{page}"), body)
            .await;
        check(&mut cells, "update a page", &got, Needs::Editor, 200);
        let path = format!("/pages/{page}/move");
        let got = wiki
            .send(bob, "POST", &path, json!({ "after": null }))
            .await;
        check(&mut cells, "move a page", &got, Needs::Editor, 200);
        let doomed = wiki.new_page().await;
        let got = wiki.call(bob, "DELETE", &format!("/pages/{doomed}")).await;
        check(&mut cells, "trash a page", &got, Needs::Editor, 204);
        let trashed = wiki.trashed_page().await;
        let path = format!("/pages/{trashed}/restore");
        let got = wiki.send(bob, "POST", &path, json!({})).await;
        let want = expected_hidden(role, Needs::Editor, Needs::Editor, 200);
        cells.check(role, "restore a page", &got, want);

        // List the trash.
        let got = wiki
            .call(bob, "GET", &format!("/spaces/{space}/trash"))
            .await;
        check(&mut cells, "list the trash", &got, Needs::Editor, 200);

        // Restore a revision, import.
        let path = format!("/pages/{page}/revisions/1/restore");
        let got = wiki.send(bob, "POST", &path, json!({})).await;
        check(&mut cells, "restore a revision", &got, Needs::Editor, 204);
        let path = format!("/spaces/{space}/import?name=notes.md");
        let mut headers = bob.headers().to_vec();
        headers.push(("content-type", "text/markdown"));
        let got = wiki.api.raw("POST", &path, &headers, b"# Notes\n").await;
        check(&mut cells, "import", &got, Needs::Editor, 201);

        // Upload or delete an attachment.
        let got = wiki.upload(bob).await;
        check(&mut cells, "upload", &got, Needs::Editor, 201);
        let path = format!("/attachments/{}", wiki.attachment);
        let got = wiki.call(bob, "DELETE", &path).await;
        check(&mut cells, "delete an attachment", &got, Needs::Editor, 204);

        // Purge a page from the trash.
        let trashed = wiki.trashed_page().await;
        let got = wiki
            .call(bob, "DELETE", &format!("/pages/{trashed}/purge"))
            .await;
        let want = expected_hidden(role, Needs::Editor, Needs::Admin, 204);
        cells.check(role, "purge a page", &got, want);

        // Update the space, edit grants.
        let body = json!({ "name": "Renamed" });
        let got = wiki
            .send(bob, "PATCH", &format!("/spaces/{space}"), body)
            .await;
        check(&mut cells, "update the space", &got, Needs::Admin, 200);
        // Bob keeps their own role, so the rows after this one still test it.
        let got = match role {
            Some(role) => wiki.grants(bob, role).await,
            None => {
                let path = format!("/spaces/{space}/grants");
                wiki.send(bob, "PUT", &path, json!({ "grants": [] })).await
            }
        };
        check(&mut cells, "edit grants", &got, Needs::Admin, 204);

        // Trash or restore the space.
        let got = wiki.call(bob, "DELETE", &format!("/spaces/{space}")).await;
        check(&mut cells, "trash the space", &got, Needs::Admin, 204);
        if got.status != 204 {
            let reply = wiki
                .call(&wiki.ada, "DELETE", &format!("/spaces/{space}"))
                .await;
            assert_eq!(reply.status, 204, "{reply:?}");
        }
        let path = format!("/spaces/{space}/restore");
        let got = wiki.send(bob, "POST", &path, json!({})).await;
        let want = expected_hidden(role, Needs::Admin, Needs::Admin, 200);
        cells.check(role, "restore the space", &got, want);
    }
    cells.assert_empty();
}

/// A trashed space denies every action but its restoration, which needs a space admin.
#[tokio::test]
async fn a_trashed_space_denies_everything_but_its_restoration() {
    let mut cells = Cells::default();
    for role in ROLES {
        let wiki = Wiki::new(role).await;
        let (bob, space, page) = (&wiki.bob, wiki.space.clone(), wiki.page.clone());
        let reply = wiki
            .call(&wiki.ada, "DELETE", &format!("/spaces/{space}"))
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");

        let reads = [
            format!("/spaces/{space}"),
            format!("/spaces/{space}/tree"),
            format!("/spaces/{space}/grants"),
            format!("/spaces/{space}/trash"),
            format!("/pages/{page}"),
            format!("/pages/{page}/export"),
            format!("/pages/{page}/revisions"),
            format!("/attachments/{}", wiki.attachment),
        ];
        for path in reads {
            let got = wiki.call(bob, "GET", &path).await;
            cells.check(role, &format!("GET {path}"), &got, 404);
        }
        let path = format!("/spaces/{space}/pages");
        let got = wiki.send(bob, "POST", &path, json!({})).await;
        cells.check(role, "create a page", &got, 404);
        let body = json!({ "name": "Renamed" });
        let got = wiki
            .send(bob, "PATCH", &format!("/spaces/{space}"), body)
            .await;
        cells.check(role, "update the space", &got, 404);
        let got = wiki.upload(bob).await;
        cells.check(role, "upload", &got, 404);
        let body = json!({ "title": "Renamed" });
        let got = wiki
            .send(bob, "PATCH", &format!("/pages/{page}"), body)
            .await;
        cells.check(role, "update a page", &got, 404);
        let got = wiki.call(bob, "DELETE", &format!("/pages/{page}")).await;
        cells.check(role, "trash a page", &got, 404);

        // Only its admins find it in the workspace's list of trashed spaces.
        let workspace = wiki.api.first_workspace().await;
        let got = wiki
            .call(bob, "GET", &format!("/workspaces/{workspace}/spaces/trash"))
            .await;
        let listed = got.body["items"]
            .as_array()
            .is_some_and(|items| items.iter().any(|item| item["id"] == space.as_str()));
        if listed != (role == Some("admin")) {
            cells
                .0
                .push(format!("{role:?} trashed spaces list: listed is {listed}"));
        }
        let request = wiki.socket_request(&page, bob);
        let refused = match connect_async(request).await {
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                response.status().as_u16()
            }
            other => panic!("expected a refusal, got {other:?}"),
        };
        if refused != 404 {
            cells
                .0
                .push(format!("{role:?} page socket: got {refused}, want 404"));
        }

        let path = format!("/spaces/{space}/restore");
        let got = wiki.send(bob, "POST", &path, json!({})).await;
        let want = expected_hidden(role, Needs::Admin, Needs::Admin, 200);
        cells.check(role, "restore", &got, want);
    }
    cells.assert_empty();
}

#[tokio::test]
async fn the_page_socket_mode_follows_the_matrix() {
    for role in ROLES {
        let wiki = Wiki::new(role).await;
        let request = wiki.socket_request(&wiki.page, &wiki.bob);
        let Some(role_name) = role else {
            match connect_async(request).await {
                Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                    assert_eq!(response.status().as_u16(), 404);
                }
                other => panic!("expected a refusal, got {other:?}"),
            }
            continue;
        };
        let mut socket = connect_async(request).await.unwrap().0;
        let title = format!("By the {role_name}");
        socket.send(title_frame(&title)).await.unwrap();
        let writes = rank(role) >= Needs::Editor as usize;
        assert_eq!(refuses_writes(&mut socket).await, !writes, "{role_name}");

        // A write reaches the page row at the next flush, a refused one never does.
        let path = format!("/pages/{}", wiki.page);
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut stored = false;
        while Instant::now() < deadline {
            let reply = wiki.api.get(&wiki.ada, &path).await;
            // Both clients wrote to the title: the merge keeps both texts.
            if reply.body["title"].as_str().unwrap().contains(&title) {
                stored = true;
                break;
            }
            sleep(Duration::from_millis(50)).await;
        }
        assert_eq!(stored, writes, "{role_name}");
    }
}

/// Every member reads the workspace, its members, searches and opens the events socket; a
/// stranger finds nothing.
#[tokio::test]
async fn workspace_reads_follow_the_matrix_over_rest_and_the_events_socket() {
    let (api, ada) = logged_in().await;
    let ws = api.first_workspace().await;
    let mut logins = vec![("owner", ada)];
    for (n, role) in ["member", "admin"].into_iter().enumerate() {
        let id = format!("11111111-0000-7000-8000-00000000000{n}");
        let login = api
            .add_member(&ws, &id, &format!("{role}@example.org"), role)
            .await;
        logins.push((role, login));
    }
    let stranger = api
        .add_and_login("22222222-0000-7000-8000-000000000001", "eve@example.org")
        .await;
    logins.push(("stranger", stranger));

    for (role, login) in &logins {
        let want = if *role == "stranger" { 404 } else { 200 };
        for path in [
            format!("/workspaces/{ws}"),
            format!("/workspaces/{ws}/members"),
            format!("/workspaces/{ws}/spaces"),
            format!("/workspaces/{ws}/recent"),
            format!("/workspaces/{ws}/search?q=notes"),
        ] {
            let got = api.get(login, &path).await;
            assert_eq!(got.status, want, "{role} GET {path}: {got:?}");
        }

        // Create a space: admins and owners only.
        let path = format!("/workspaces/{ws}/spaces");
        let body = json!({ "name": format!("By the {role}") });
        let got = api.send("POST", &path, &login.headers(), body).await;
        let want_create = match *role {
            "stranger" => 404,
            "member" => 403,
            _ => 201,
        };
        assert_eq!(got.status, want_create, "{role} creates a space: {got:?}");

        let url = format!("ws://{}/ws/events?workspace={ws}", api.addr);
        let mut request = url.into_client_request().unwrap();
        let headers = request.headers_mut();
        headers.insert(COOKIE, login.cookie.parse().unwrap());
        headers.insert("origin", ORIGIN.parse().unwrap());
        match connect_async(request).await {
            Ok(_) => assert_eq!(want, 200, "{role} opened the events socket"),
            Err(tokio_tungstenite::tungstenite::Error::Http(response)) => {
                assert_eq!(response.status().as_u16(), want, "{role} events socket");
            }
            Err(other) => panic!("{role}: {other:?}"),
        }
    }
}

/// Instance actions of PERMISSIONS.md that depend on `CADOCS_WORKSPACE_CREATION` (ADR 0024): who
/// creates a workspace, and what `GET /session` tells the front end about it.
#[tokio::test]
async fn workspace_creation_follows_the_setting_over_rest() {
    for (setting, user_creates) in [(None, true), (Some("all"), true), (Some("admins"), false)] {
        let settings: Vec<(&str, &str)> = setting
            .map(|value| ("CADOCS_WORKSPACE_CREATION", value))
            .into_iter()
            .collect();
        let api = start_tuned(ORIGIN, "", &settings).await;
        let reply = api.setup().await;
        assert_eq!(reply.status, 201, "{reply:?}");
        let admin = Login::from(&reply);
        let ws = api.first_workspace().await;
        let member = api.add_member(&ws, BOB, "bob@example.org", "member").await;
        let loner = api
            .add_and_login("22222222-0000-7000-8000-000000000001", "eve@example.org")
            .await;

        for (who, login, allowed) in [
            ("instance admin", &admin, true),
            ("member", &member, user_creates),
            ("user with no workspace", &loner, user_creates),
        ] {
            let session = api.session(login).await;
            assert_eq!(
                session.body["can_create_workspace"], allowed,
                "{setting:?} {who}: {session:?}"
            );

            let body = json!({ "name": format!("Of the {who}") });
            let got = api
                .send("POST", "/workspaces", &login.headers(), body)
                .await;
            if allowed {
                assert_eq!(got.status, 201, "{setting:?} {who}: {got:?}");
                assert_eq!(got.body["role"], "owner", "{setting:?} {who}");
            } else {
                assert_eq!(
                    (got.status, got.code()),
                    (403, "forbidden"),
                    "{setting:?} {who}"
                );
            }
        }
    }
}
