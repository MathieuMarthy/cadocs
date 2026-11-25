//! In-app notifications over REST (`docs/API.md#notifications`): who is told of a comment, and
//! what reading them does. The `notification` event is in `events.rs`.

mod common;

use common::{Api, Login, Reply, logged_in};
use serde_json::{Value, json};

const BOB: &str = "33333333-0000-7000-8000-000000000001";
const CAROL: &str = "33333333-0000-7000-8000-000000000002";

/// Ada owns the workspace, the space and the page. Bob and Carol comment on it.
struct Wiki {
    api: Api,
    ada: Login,
    bob: Login,
    carol: Login,
    space: String,
    page: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

async fn wiki() -> Wiki {
    let (api, ada) = logged_in().await;
    let workspace = api.first_workspace().await;
    let bob = api
        .add_member(&workspace, BOB, "bob@example.org", "member")
        .await;
    let carol = api
        .add_member(&workspace, CAROL, "carol@example.org", "member")
        .await;
    let path = format!("/workspaces/{workspace}/spaces");
    let body = json!({ "name": "Handbook", "default_role": null });
    let space = id(&api.send("POST", &path, &ada.headers(), body).await);
    let wiki = Wiki {
        space: space.clone(),
        page: String::new(),
        api,
        ada,
        bob,
        carol,
    };
    wiki.grant(true).await;
    let path = format!("/spaces/{space}/pages");
    let reply = wiki
        .api
        .send("POST", &path, &wiki.ada.headers(), json!({}))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    Wiki {
        page: id(&reply),
        ..wiki
    }
}

impl Wiki {
    /// Bob and Carol comment, or only Carol does.
    async fn grant(&self, bob: bool) {
        let mut grants = vec![json!({ "user_id": CAROL, "role": "commenter" })];
        if bob {
            grants.push(json!({ "user_id": BOB, "role": "commenter" }));
        }
        let path = format!("/spaces/{}/grants", self.space);
        let body = json!({ "grants": grants });
        let reply = self.api.send("PUT", &path, &self.ada.headers(), body).await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }

    async fn comment(&self, by: &Login, body: Value) -> String {
        let path = format!("/pages/{}/comments", self.page);
        let reply = self.api.send("POST", &path, &by.headers(), body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn inbox(&self, by: &Login) -> Value {
        let reply = self.api.get(by, "/me/notifications").await;
        assert_eq!(reply.status, 200, "{reply:?}");
        reply.body
    }

    async fn read(&self, by: &Login, body: Value) -> Reply {
        let headers = by.headers();
        self.api
            .send("POST", "/me/notifications/read", &headers, body)
            .await
    }
}

fn kinds(inbox: &Value) -> Vec<&str> {
    let items = inbox["items"].as_array().unwrap();
    items.iter().map(|n| n["kind"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn a_comment_tells_the_owner_of_the_page_and_never_its_author() {
    let wiki = wiki().await;
    assert_eq!(wiki.inbox(&wiki.ada).await["unread"], 0);
    let thread = wiki.comment(&wiki.bob, json!({ "body": "Question" })).await;

    let inbox = wiki.inbox(&wiki.ada).await;
    assert_eq!(kinds(&inbox), ["comment"]);
    assert_eq!(inbox["unread"], 1);
    let item = &inbox["items"][0];
    assert_eq!(item["comment_id"], thread);
    assert_eq!(item["page"]["id"], wiki.page);
    assert_eq!(item["page"]["space_id"], wiki.space);
    assert_eq!(item["actor"]["id"], BOB);
    assert_eq!(item["read"], false);
    assert_eq!(kinds(&wiki.inbox(&wiki.bob).await), Vec::<&str>::new());

    // Ada answers: Bob is told of a reply, and Ada of nothing, her own comment.
    wiki.comment(&wiki.ada, json!({ "body": "Answer", "parent_id": thread }))
        .await;
    assert_eq!(kinds(&wiki.inbox(&wiki.bob).await), ["reply"]);
    assert_eq!(wiki.inbox(&wiki.ada).await["unread"], 1);
}

#[tokio::test]
async fn a_mention_is_one_notification_whatever_else_applies() {
    let wiki = wiki().await;
    let body = format!("Ada, see <user:{BOB}> and <user:{CAROL}>");
    let thread = wiki.comment(&wiki.bob, json!({ "body": body })).await;
    // Bob named himself: no one tells him. Carol is mentioned, Ada owns the page.
    assert_eq!(kinds(&wiki.inbox(&wiki.bob).await), Vec::<&str>::new());
    assert_eq!(kinds(&wiki.inbox(&wiki.carol).await), ["mention"]);
    assert_eq!(kinds(&wiki.inbox(&wiki.ada).await), ["comment"]);

    // Ada replies and mentions Bob, who owns the thread: a mention, not also a reply.
    let body = format!("<user:{BOB}>");
    wiki.comment(&wiki.ada, json!({ "body": body, "parent_id": thread }))
        .await;
    assert_eq!(kinds(&wiki.inbox(&wiki.bob).await), ["mention"]);

    // An edit tells those it names anew, and keeps the notifications of those it still names.
    let path = format!("/comments/{thread}");
    let body = json!({ "body": format!("<user:{CAROL}> <user:{BOB}>") });
    let reply = wiki
        .api
        .send("PATCH", &path, &wiki.bob.headers(), body)
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(kinds(&wiki.inbox(&wiki.carol).await), ["mention"]);
}

#[tokio::test]
async fn reading_changes_the_state_of_the_caller_alone() {
    let wiki = wiki().await;
    let first = wiki.comment(&wiki.bob, json!({ "body": "One" })).await;
    wiki.comment(&wiki.carol, json!({ "body": "Two" })).await;
    let inbox = wiki.inbox(&wiki.ada).await;
    assert_eq!(inbox["unread"], 2);
    // Newest first.
    assert_eq!(inbox["items"][1]["comment_id"], first);
    let one = inbox["items"][1]["id"].clone();

    // Another user's ids change nothing.
    let reply = wiki.read(&wiki.bob, json!({ "ids": [one] })).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(wiki.inbox(&wiki.ada).await["unread"], 2);

    let reply = wiki.read(&wiki.ada, json!({ "ids": [one] })).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let inbox = wiki.inbox(&wiki.ada).await;
    assert_eq!(inbox["unread"], 1);
    assert_eq!(inbox["items"][1]["read"], true);

    let reply = wiki.read(&wiki.ada, json!({})).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let inbox = wiki.inbox(&wiki.ada).await;
    assert_eq!(inbox["unread"], 0);
    assert_eq!(inbox["items"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn a_notification_follows_the_access_to_its_page_and_comment() {
    let wiki = wiki().await;
    let body = format!("<user:{BOB}>");
    let thread = wiki.comment(&wiki.carol, json!({ "body": body })).await;
    assert_eq!(wiki.inbox(&wiki.bob).await["unread"], 1);

    // Bob loses the space: the notification would name a page he cannot read.
    wiki.grant(false).await;
    let inbox = wiki.inbox(&wiki.bob).await;
    assert_eq!(kinds(&inbox), Vec::<&str>::new());
    assert_eq!(inbox["unread"], 0);
    wiki.grant(true).await;
    assert_eq!(wiki.inbox(&wiki.bob).await["unread"], 1);

    // The comment gone, its notifications go with it.
    let path = format!("/comments/{thread}");
    let reply = wiki
        .api
        .raw("DELETE", &path, &wiki.carol.headers(), b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(wiki.inbox(&wiki.bob).await["unread"], 0);
    assert_eq!(wiki.inbox(&wiki.ada).await["unread"], 0);
}

#[tokio::test]
async fn the_routes_need_a_session() {
    let wiki = wiki().await;
    let reply = wiki
        .api
        .send("GET", "/me/notifications", &[], Value::Null)
        .await;
    assert_eq!(reply.status, 401, "{reply:?}");
}

#[tokio::test]
async fn an_edit_naming_someone_told_already_turns_it_into_a_mention() {
    let wiki = wiki().await;
    let thread = wiki.comment(&wiki.ada, json!({ "body": "Question" })).await;
    let reply = wiki
        .comment(&wiki.bob, json!({ "body": "Answer", "parent_id": thread }))
        .await;
    assert_eq!(kinds(&wiki.inbox(&wiki.ada).await), ["reply"]);

    let ada = wiki.api.session(&wiki.ada).await.body["user"]["id"].clone();
    let body = json!({ "body": format!("Answer <user:{}>", ada.as_str().unwrap()) });
    let path = format!("/comments/{reply}");
    let reply = wiki
        .api
        .send("PATCH", &path, &wiki.bob.headers(), body)
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(kinds(&wiki.inbox(&wiki.ada).await), ["mention"]);
}

/// Only who may read the page is told of it: rows they could never see would push their real
/// notifications out of the inbox.
#[tokio::test]
async fn no_one_is_told_of_a_page_they_cannot_read() {
    let wiki = wiki().await;
    let thread = wiki.comment(&wiki.bob, json!({ "body": "Question" })).await;
    wiki.grant(false).await;

    let body = format!("<user:{BOB}> look");
    wiki.comment(&wiki.carol, json!({ "body": body })).await;
    wiki.comment(
        &wiki.carol,
        json!({ "body": "Answer", "parent_id": thread }),
    )
    .await;
    // Ada owns the page and reads it: she is told.
    assert_eq!(wiki.inbox(&wiki.ada).await["unread"], 3);

    // Nothing was kept for Bob to see once he reads the space again.
    wiki.grant(true).await;
    assert_eq!(kinds(&wiki.inbox(&wiki.bob).await), Vec::<&str>::new());
}
