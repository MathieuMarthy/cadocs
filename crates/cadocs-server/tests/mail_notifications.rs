//! Notifications by mail (`docs/API.md#mail-of-notifications`): one message for a burst, only for
//! those who asked, and nothing without a relay.

mod common;

use std::time::Duration;

use common::mail::{body, messages, settle, with_mail};
use common::{Api, Login, PASSWORD, logged_in};
use serde_json::{Value, json};

const BOB: &str = "44444444-0000-7000-8000-000000000001";

fn id(body: &Value) -> String {
    body["id"].as_str().unwrap().to_owned()
}

/// Ada owns a workspace with one page, which Bob may comment on.
async fn wiki(api: &Api, ada: &Login) -> (Login, String) {
    let workspace = api.first_workspace().await;
    let bob = api
        .add_member(&workspace, BOB, "bob@example.org", "member")
        .await;
    let path = format!("/workspaces/{workspace}/spaces");
    let body = json!({ "name": "Handbook", "default_role": null });
    let space = id(&api.send("POST", &path, &ada.headers(), body).await.body);
    let grants = json!({ "grants": [{ "user_id": BOB, "role": "commenter" }] });
    let path = format!("/spaces/{space}/grants");
    assert_eq!(
        api.send("PUT", &path, &ada.headers(), grants).await.status,
        204
    );
    let path = format!("/spaces/{space}/pages");
    let page = api.send("POST", &path, &ada.headers(), json!({})).await;
    (bob, id(&page.body))
}

async fn comment(api: &Api, by: &Login, page: &str, text: &str) {
    let path = format!("/pages/{page}/comments");
    let reply = api
        .send("POST", &path, &by.headers(), json!({ "body": text }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
}

async fn choose(api: &Api, by: &Login, enabled: bool) -> u16 {
    let body = json!({ "enabled": enabled });
    api.send("PUT", "/me/email-notifications", &by.headers(), body)
        .await
        .status
}

async fn ada(api: &Api) -> Login {
    Login::from(&api.login("ada@example.org", PASSWORD).await)
}

#[tokio::test]
async fn a_burst_of_notifications_sends_one_message_to_who_asked() {
    let (api, relay) = with_mail().await;
    let ada = ada(&api).await;
    let (bob, page) = wiki(&api, &ada).await;
    assert_eq!(choose(&api, &ada, true).await, 204);
    assert_eq!(
        api.get(&ada, "/session").await.body["user"]["email_notifications"],
        true
    );

    comment(&api, &bob, &page, "First").await;
    comment(&api, &bob, &page, "Second").await;

    let sent = messages(&relay, 1).await;
    assert!(sent[0].to.contains("ada@example.org"), "{:?}", sent[0].to);
    let text = body(&sent[0]);
    assert!(text.matches("commented on").count() == 1, "{text}");
    assert!(text.contains(&format!("/?page={page}")), "{text}");
    // Bob never asked: he is told of nothing, and Ada's two notifications went in one message.
    settle().await;
    assert_eq!(relay.messages().len(), 1);

    // What a message told is not told again.
    comment(&api, &bob, &page, "Third").await;
    assert_eq!(messages(&relay, 2).await.len(), 2);
}

#[tokio::test]
async fn the_message_is_in_the_language_of_who_gets_it() {
    let (api, relay) = with_mail().await;
    let ada = ada(&api).await;
    let (bob, page) = wiki(&api, &ada).await;
    assert_eq!(choose(&api, &ada, true).await, 204);
    api.sql("UPDATE workspaces SET language = 'fr'").await;

    comment(&api, &bob, &page, "Premier").await;

    let text = body(&messages(&relay, 1).await[0]);
    assert!(text.contains("Bonjour Ada,"), "{text}");
    assert!(text.contains("Other a comment"), "{text}");
    assert!(!text.contains("commented on"), "{text}");
}

#[tokio::test]
async fn nothing_is_sent_to_who_did_not_ask_or_already_read() {
    let (api, relay) = with_mail().await;
    let ada = ada(&api).await;
    let (bob, page) = wiki(&api, &ada).await;
    comment(&api, &bob, &page, "Unasked").await;

    assert_eq!(choose(&api, &ada, true).await, 204);
    comment(&api, &bob, &page, "Read at once").await;
    let read = api
        .send("POST", "/me/notifications/read", &ada.headers(), json!({}))
        .await;
    assert_eq!(read.status, 204);
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(relay.messages().is_empty(), "{:?}", relay.messages());

    assert_eq!(choose(&api, &ada, false).await, 204);
    comment(&api, &bob, &page, "Off again").await;
    tokio::time::sleep(Duration::from_millis(900)).await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn without_a_relay_the_route_does_not_exist() {
    let (api, login) = logged_in().await;
    assert_eq!(choose(&api, &login, true).await, 404);
}
