//! The routes that mail turns on (ADR 0026), against a relay that keeps what it is sent.

mod common;

use std::time::{Duration, Instant};

use cadocs_mail::fake::{Behavior, Received};
use common::mail::{messages, settle, text, with_mail, with_relay};
use common::{Api, ORIGIN, PASSWORD, start};
use serde_json::json;

const NEW_PASSWORD: &str = "another horse battery";

impl Api {
    async fn forgot(&self, email: &str) -> common::Reply {
        self.forgot_from(email, &[]).await
    }

    async fn forgot_from(&self, email: &str, headers: &[(&str, &str)]) -> common::Reply {
        let body = json!({ "email": email });
        self.send("POST", "/auth/forgot-password", headers, body)
            .await
    }
}

/// The token of the reset link in a message.
fn reset_token(message: &Received) -> String {
    let text = text(message);
    let prefix = format!("{ORIGIN}/reset-password/");
    let start = text.find(&prefix).expect("a reset link") + prefix.len();
    let token: String = text[start..]
        .chars()
        .take_while(char::is_ascii_hexdigit)
        .collect();
    assert_eq!(token.len(), 64, "{text}");
    token
}

#[tokio::test]
async fn a_reset_link_arrives_by_mail_and_works() {
    let (api, relay) = with_mail().await;
    let reply = api.forgot(" Ada@Example.org ").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(reply.bytes.is_empty());

    let message = &messages(&relay, 1).await[0];
    assert_eq!(message.to, "<ada@example.org>");
    assert_eq!(message.from, "<wiki@example.org>");
    assert!(message.data.contains("Subject: Reset your cadocs password"));

    let path = format!("/auth/password-reset/{}", reset_token(message));
    let body = json!({ "password": NEW_PASSWORD });
    let reply = api.send("POST", &path, &[], body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.login("ada@example.org", PASSWORD).await.status, 401);
    assert_eq!(api.login("ada@example.org", NEW_PASSWORD).await.status, 200);
}

#[tokio::test]
async fn a_reset_voids_the_pending_codes() {
    let (api, relay) = with_mail().await;
    // A code of the account, and a registration code of nobody's.
    api.sql(
        "INSERT INTO email_codes (purpose, email, user_id, code_hash, expires_at, created_at)
         SELECT 'email', 'new@example.org', id, 'hash', 9999999999999, 1 FROM users LIMIT 1",
    )
    .await;
    api.sql(
        "INSERT INTO email_codes (purpose, email, code_hash, expires_at, created_at)
         VALUES ('register', 'grace@example.org', 'hash', 9999999999999, 1)",
    )
    .await;
    api.forgot("ada@example.org").await;
    let path = format!(
        "/auth/password-reset/{}",
        reset_token(&messages(&relay, 1).await[0])
    );
    let reply = api
        .send("POST", &path, &[], json!({ "password": NEW_PASSWORD }))
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let left = "SELECT count(*) FROM email_codes WHERE user_id IS NULL";
    assert_eq!(api.number(left).await, 1);
    assert_eq!(api.number("SELECT count(*) FROM email_codes").await, 1);
}

#[tokio::test]
async fn only_a_hash_of_the_mailed_token_is_stored() {
    let (api, relay) = with_mail().await;
    api.forgot("ada@example.org").await;
    let token = reset_token(&messages(&relay, 1).await[0]);
    let stored = api
        .number(&format!(
            "SELECT count(*) FROM password_resets WHERE token_hash = '{token}'"
        ))
        .await;
    assert_eq!(stored, 0);
    let rows = api
        .number("SELECT count(*) FROM password_resets WHERE created_by IS NULL")
        .await;
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn an_unknown_address_is_told_nothing() {
    let (api, relay) = with_mail().await;
    let known = api.forgot("ada@example.org").await;
    let unknown = api.forgot("nobody@example.org").await;
    assert_eq!(unknown.status, 204, "{unknown:?}");
    assert_eq!(unknown.bytes, known.bytes);
    let names = |reply: &common::Reply| -> Vec<String> {
        let names = reply.headers.iter().map(|(name, _)| name.clone());
        names.filter(|name| name != "date").collect()
    };
    assert_eq!(names(&unknown), names(&known));

    messages(&relay, 1).await;
    settle().await;
    let all = relay.messages();
    assert_eq!(all.len(), 1, "{all:?}");
    assert_eq!(all[0].to, "<ada@example.org>");
    assert_eq!(api.number("SELECT count(*) FROM password_resets").await, 1);
}

#[tokio::test]
async fn a_disabled_account_gets_no_message_and_the_same_answer() {
    let (api, relay) = with_mail().await;
    api.add_user("u2", "grace@example.org").await;
    api.sql("UPDATE users SET disabled_at = 1 WHERE id = 'u2'")
        .await;
    assert_eq!(api.forgot("grace@example.org").await.status, 204);
    settle().await;
    assert!(relay.messages().is_empty());
    assert_eq!(api.number("SELECT count(*) FROM password_resets").await, 0);
}

#[tokio::test]
async fn the_answer_does_not_wait_for_the_relay() {
    let silent = Behavior {
        silent: true,
        ..Behavior::default()
    };
    let (api, _relay) = with_relay(silent, "").await;
    let before = Instant::now();
    assert_eq!(api.forgot("ada@example.org").await.status, 204);
    assert!(before.elapsed() < Duration::from_secs(2));
}

#[tokio::test]
async fn a_refusing_relay_changes_nothing_for_the_client() {
    let refusing = Behavior {
        refuse_recipients: true,
        ..Behavior::default()
    };
    let (api, relay) = with_relay(refusing, "").await;
    assert_eq!(api.forgot("ada@example.org").await.status, 204);
    settle().await;
    assert!(relay.messages().is_empty());
    assert_eq!(api.forgot("ada@example.org").await.status, 204);
}

#[tokio::test]
async fn with_mail_off_the_route_does_not_exist() {
    let api = start().await;
    assert_eq!(api.setup().await.status, 201);
    let reply = api.forgot("ada@example.org").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    assert_eq!(reply.code(), "not_found");
    // Neither a wrong method nor a bad body tells the route apart from one that never was.
    let reply = api.raw("GET", "/auth/forgot-password", &[], b"").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = api.raw("POST", "/auth/forgot-password", &[], b"{").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    assert_eq!(api.number("SELECT count(*) FROM password_resets").await, 0);
}

#[tokio::test]
async fn setup_state_says_whether_mail_is_on() {
    let api = start().await;
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(reply.body, json!({ "required": true }));

    let (api, _relay) = with_mail().await;
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(reply.body["mail"], true);
}

#[tokio::test]
async fn a_bad_address_is_refused_by_name() {
    let (api, relay) = with_mail().await;
    for (email, problem) in [("", "required"), ("ada", "invalid")] {
        let reply = api.forgot(email).await;
        assert_eq!(reply.status, 400, "{reply:?}");
        assert_eq!(reply.body["error"]["fields"]["email"], problem);
    }
    let reply = api
        .send("POST", "/auth/forgot-password", &[], json!({}))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    settle().await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn a_foreign_origin_cannot_ask() {
    let (api, relay) = with_mail().await;
    let headers = [("origin", "https://evil.example")];
    let reply = api.forgot_from("ada@example.org", &headers).await;
    assert_eq!(reply.status, 403, "{reply:?}");
    settle().await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn each_client_can_ask_a_few_times_then_waits() {
    let (api, _relay) = with_mail().await;
    let mut replies = Vec::new();
    for n in 0..10 {
        replies.push(api.forgot(&format!("user{n}@example.org")).await);
    }
    let passed = replies.iter().filter(|reply| reply.status == 204).count();
    assert_eq!(passed, 5);
    let limited = &replies[5];
    assert_eq!(limited.status, 429, "{limited:?}");
    assert_eq!(limited.code(), "rate_limited");
    let wait: u64 = limited.header("retry-after").unwrap().parse().unwrap();
    assert!(wait >= 1);
    // The budget of password guesses is another one.
    assert_eq!(api.login("ada@example.org", PASSWORD).await.status, 200);
}

#[tokio::test]
async fn an_address_gets_a_few_messages_whoever_asks() {
    let (api, relay) = with_relay(Behavior::default(), "127.0.0.1").await;
    api.add_user("u2", "grace@example.org").await;
    for n in 0..8 {
        let client = format!("203.0.113.{n}");
        let headers = [("x-forwarded-for", client.as_str())];
        // In another case: the limit is on the account's address, not on what was typed.
        let email = if n % 2 == 0 {
            "ada@example.org"
        } else {
            "ADA@example.org"
        };
        let reply = api.forgot_from(email, &headers).await;
        // The same answer past the limit: it must not tell that others asked for this address.
        assert_eq!(reply.status, 204, "{reply:?}");
        assert!(reply.header("retry-after").is_none());
    }
    messages(&relay, 3).await;
    settle().await;
    assert_eq!(relay.messages().len(), 3);

    // Another address still gets its message.
    let headers = [("x-forwarded-for", "203.0.113.200")];
    api.forgot_from("grace@example.org", &headers).await;
    let all = messages(&relay, 4).await;
    assert_eq!(all[3].to, "<grace@example.org>");
}

#[tokio::test]
async fn a_new_link_replaces_the_one_mailed_before() {
    let (api, relay) = with_mail().await;
    api.forgot("ada@example.org").await;
    let first = reset_token(&messages(&relay, 1).await[0]);
    api.forgot("ada@example.org").await;
    let second = reset_token(&messages(&relay, 2).await[1]);
    assert_ne!(first, second);

    let body = json!({ "password": NEW_PASSWORD });
    let path = format!("/auth/password-reset/{first}");
    assert_eq!(api.send("POST", &path, &[], body.clone()).await.status, 404);
    let path = format!("/auth/password-reset/{second}");
    assert_eq!(api.send("POST", &path, &[], body).await.status, 204);
}

#[tokio::test]
async fn a_full_outbox_leaves_the_earlier_links_alone() {
    let silent = Behavior {
        silent: true,
        ..Behavior::default()
    };
    let (api, _relay) = with_relay(silent, "127.0.0.1").await;
    // More accounts than the outbox holds while the relay says nothing.
    for n in 0..90 {
        api.add_user(&format!("u{n}"), &format!("user{n}@example.org"))
            .await;
        api.sql(&format!(
            "INSERT INTO password_resets (token_hash, user_id, expires_at, created_at)
             VALUES ('old{n}', 'u{n}', 9999999999999, 1)"
        ))
        .await;
    }
    for n in 0..90 {
        let client = format!("203.0.113.{n}");
        let headers = [("x-forwarded-for", client.as_str())];
        let reply = api
            .forgot_from(&format!("user{n}@example.org"), &headers)
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }
    settle().await;
    // A link is only replaced when its message has a place: 64 wait, 4 are being sent, and one
    // waits for its turn.
    let kept = api
        .number("SELECT count(*) FROM password_resets WHERE token_hash LIKE 'old%'")
        .await;
    assert!(kept >= 90 - 69, "{kept} earlier links are left");
    assert_eq!(api.number("SELECT count(*) FROM password_resets").await, 90);
}
