//! Proof of a new address: with a relay, a code sent to it changes it (ADR 0026).

mod common;

use cadocs_mail::fake::{FakeRelay, Received};
use common::mail::{body, messages, settle, with_mail};
use common::{Api, Login, PASSWORD, Reply, logged_in};
use serde_json::json;

const ADA: &str = "ada@example.org";
const NEW: &str = "new@example.org";

impl Api {
    async fn ask_address(&self, by: &Login, email: &str) -> Reply {
        let body = json!({ "email": email, "current_password": PASSWORD });
        self.send("PATCH", "/me", &by.headers(), body).await
    }

    async fn confirm_address(&self, by: &Login, email: &str, code: &str) -> Reply {
        let body = json!({ "email": email, "code": code });
        self.send("POST", "/me/email/confirm", &by.headers(), body)
            .await
    }

    async fn address(&self, by: &Login) -> String {
        let reply = self.session(by).await;
        reply.body["user"]["email"].as_str().unwrap().to_owned()
    }

    async fn codes(&self) -> i64 {
        self.number("SELECT count(*) FROM email_codes").await
    }
}

/// The 6-digit code of a message.
fn code(message: &Received) -> String {
    let body = body(message);
    let words = body.split(|c: char| !c.is_ascii_digit());
    let codes: Vec<&str> = words.filter(|word| word.len() == 6).collect();
    assert_eq!(codes.len(), 1, "{body}");
    codes[0].to_owned()
}

fn wrong(code: &str) -> String {
    let number: u32 = code.parse().unwrap();
    format!("{:06}", (number + 1) % 1_000_000)
}

fn refused(reply: &Reply, field: &str, problem: &str) {
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"][field], problem, "{reply:?}");
}

/// Set up with mail and logged in as the first account.
async fn mail_and_login() -> (Api, FakeRelay, Login) {
    let (api, relay) = with_mail().await;
    let reply = api.login(ADA, PASSWORD).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    (api, relay, Login::from(&reply))
}

/// Asks to move to `email`, and returns the code mailed as message `count`.
async fn mailed_code(
    api: &Api,
    relay: &FakeRelay,
    by: &Login,
    email: &str,
    count: usize,
) -> String {
    let reply = api.ask_address(by, email).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    code(&messages(relay, count).await[count - 1])
}

#[tokio::test]
async fn the_address_only_changes_after_its_code() {
    let (api, relay, login) = mail_and_login().await;
    let reply = api.ask_address(&login, " NEW@example.org ").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email"], ADA);
    assert_eq!(api.address(&login).await, ADA);

    let message = &messages(&relay, 1).await[0];
    assert_eq!(message.to, "<new@example.org>");
    assert!(message.data.contains("Subject: Your cadocs code"));
    let code = code(message);

    let reply = api.confirm_address(&login, NEW, &wrong(&code)).await;
    refused(&reply, "code", "wrong");
    assert_eq!(api.address(&login).await, ADA);

    let reply = api.confirm_address(&login, "New@Example.org", &code).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email"], NEW);
    assert_eq!(api.address(&login).await, NEW);
    let verified = "SELECT count(*) FROM users WHERE email_verified_at IS NOT NULL";
    assert_eq!(api.number(verified).await, 1);
    assert_eq!(api.login(NEW, PASSWORD).await.status, 200);
    assert_eq!(api.login(ADA, PASSWORD).await.status, 401);

    // Used once.
    refused(
        &api.confirm_address(&login, NEW, &code).await,
        "code",
        "wrong",
    );
}

#[tokio::test]
async fn the_other_fields_change_at_once() {
    let (api, relay, login) = mail_and_login().await;
    let body = json!({ "name": "Grace", "email": NEW, "current_password": PASSWORD });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["name"], "Grace");
    assert_eq!(reply.body["email"], ADA);
    messages(&relay, 1).await;
}

#[tokio::test]
async fn asking_needs_the_current_password() {
    let (api, relay, login) = mail_and_login().await;
    let body = json!({ "email": NEW });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    refused(&reply, "current_password", "required");
    let body = json!({ "email": NEW, "current_password": "wrong password" });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    refused(&reply, "current_password", "wrong");
    settle().await;
    assert!(relay.messages().is_empty());
    assert_eq!(api.codes().await, 0);
}

#[tokio::test]
async fn the_same_address_sends_nothing() {
    let (api, relay, login) = mail_and_login().await;
    let reply = api.ask_address(&login, "Ada@Example.org").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email"], ADA);
    settle().await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn a_taken_address_gets_a_notice_and_no_code() {
    let (api, relay, login) = mail_and_login().await;
    api.add_user("other", "taken@example.org").await;
    let reply = api.ask_address(&login, "taken@example.org").await;
    // The same answer as for a free address.
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email"], ADA);

    let message = &messages(&relay, 1).await[0];
    assert_eq!(message.to, "<taken@example.org>");
    let text = body(message);
    assert!(text.contains("already has an account"), "{text}");
    assert!(!text.lines().any(|line| line.starts_with("    ")), "{text}");
    assert_eq!(api.codes().await, 0);

    // A disabled account gets nothing.
    api.sql("UPDATE users SET disabled_at = 1 WHERE id = 'other'")
        .await;
    api.ask_address(&login, "taken@example.org").await;
    settle().await;
    assert_eq!(relay.messages().len(), 1);
}

#[tokio::test]
async fn an_address_taken_meanwhile_is_a_conflict_that_keeps_the_code() {
    let (api, relay, login) = mail_and_login().await;
    let code = mailed_code(&api, &relay, &login, NEW, 1).await;
    api.add_user("other", NEW).await;
    let reply = api.confirm_address(&login, NEW, &code).await;
    assert_eq!(reply.status, 409, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"], json!({ "email": "taken" }));

    api.sql("DELETE FROM users WHERE id = 'other'").await;
    let reply = api.confirm_address(&login, NEW, &code).await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn a_code_works_for_its_address_and_account_only() {
    let (api, relay, login) = mail_and_login().await;
    let code = mailed_code(&api, &relay, &login, NEW, 1).await;

    let reply = api
        .confirm_address(&login, "other@example.org", &code)
        .await;
    refused(&reply, "code", "wrong");
    let other = api.add_and_login("other", "other@example.org").await;
    refused(
        &api.confirm_address(&other, NEW, &code).await,
        "code",
        "wrong",
    );
    assert_eq!(api.address(&other).await, "other@example.org");

    // Neither counted a try on it.
    let tries = "SELECT tries FROM email_codes";
    assert_eq!(api.number(tries).await, 0);
    let reply = api.confirm_address(&login, NEW, &code).await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn five_wrong_codes_kill_it() {
    let (api, relay, login) = mail_and_login().await;
    let code = mailed_code(&api, &relay, &login, NEW, 1).await;
    for _ in 0..5 {
        refused(
            &api.confirm_address(&login, NEW, &wrong(&code)).await,
            "code",
            "wrong",
        );
    }
    refused(
        &api.confirm_address(&login, NEW, &code).await,
        "code",
        "wrong",
    );
    assert_eq!(api.address(&login).await, ADA);
    assert_eq!(api.codes().await, 0);
}

#[tokio::test]
async fn confirming_needs_a_code_and_a_new_address() {
    let (api, relay, login) = mail_and_login().await;
    let reply = api.confirm_address(&login, NEW, " ").await;
    refused(&reply, "code", "required");
    let reply = api
        .confirm_address(&login, "not an address", "123456")
        .await;
    refused(&reply, "email", "invalid");

    // The code that turns the factor on proves the current address, and changes nothing.
    let reply = api
        .send("POST", "/me/email-2fa", &login.headers(), json!({}))
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let code = code(&messages(&relay, 1).await[0]);
    refused(
        &api.confirm_address(&login, ADA, &code).await,
        "code",
        "wrong",
    );
    let body = json!({ "code": code });
    let reply = api
        .send("POST", "/me/email-2fa/confirm", &login.headers(), body)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
}

#[tokio::test]
async fn a_change_code_does_not_turn_the_factor_on() {
    let (api, relay, login) = mail_and_login().await;
    let code = mailed_code(&api, &relay, &login, NEW, 1).await;
    let body = json!({ "code": code });
    let reply = api
        .send("POST", "/me/email-2fa/confirm", &login.headers(), body)
        .await;
    refused(&reply, "code", "wrong");
    let on = "SELECT count(*) FROM users WHERE email_2fa = 1";
    assert_eq!(api.number(on).await, 0);
}

#[tokio::test]
async fn a_proven_address_keeps_the_factor() {
    let (api, relay, login) = mail_and_login().await;
    api.sql("UPDATE users SET email_2fa = 1").await;
    let code = mailed_code(&api, &relay, &login, NEW, 1).await;
    let reply = api.confirm_address(&login, NEW, &code).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email_2fa"], true);

    // The next login code goes to the new address.
    let reply = api.login(NEW, PASSWORD).await;
    refused(&reply, "code", "required");
    assert_eq!(messages(&relay, 2).await[1].to, "<new@example.org>");
}

#[tokio::test]
async fn asking_shares_the_mail_limits() {
    let (api, relay, login) = mail_and_login().await;
    // 3 messages per address, whoever asks.
    for _ in 0..4 {
        assert_eq!(api.ask_address(&login, NEW).await.status, 200);
    }
    settle().await;
    assert_eq!(messages(&relay, 3).await.len(), 3);
    // 5 requests per client, the 4 above included.
    assert_eq!(api.ask_address(&login, "b@example.org").await.status, 200);
    let reply = api.ask_address(&login, "c@example.org").await;
    assert_eq!(reply.status, 429, "{reply:?}");
    // A name alone sends nothing, and counts nothing.
    let body = json!({ "name": "Grace" });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn without_mail_the_address_changes_at_once_unproven() {
    let (api, login) = logged_in().await;
    api.sql("UPDATE users SET email_2fa = 1, email_verified_at = 1")
        .await;
    let reply = api.ask_address(&login, NEW).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email"], NEW);
    // Nobody proved it: the factor, useless without a relay, does not wait for one on it.
    assert_eq!(reply.body["email_2fa"], false);
    let unproven = "SELECT count(*) FROM users
                    WHERE email_verified_at IS NULL AND email_2fa = 0";
    assert_eq!(api.number(unproven).await, 1);

    let reply = api.confirm_address(&login, NEW, "123456").await;
    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn without_mail_a_name_keeps_the_proof() {
    let (api, login) = logged_in().await;
    api.sql("UPDATE users SET email_2fa = 1, email_verified_at = 1")
        .await;
    let body = json!({ "name": "Grace", "email": ADA, "current_password": PASSWORD });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email_2fa"], true);
    let kept = "SELECT count(*) FROM users WHERE email_verified_at = 1 AND email_2fa = 1";
    assert_eq!(api.number(kept).await, 1);
}

const RESETS: &str = "SELECT count(*) FROM password_resets";
const ADD_RESET: &str = "INSERT INTO password_resets (token_hash, user_id, expires_at, created_at)
                         SELECT 'hash', id, 9999999999999, 1 FROM users LIMIT 1";

#[tokio::test]
async fn a_proven_address_voids_the_reset_link_of_the_old_one() {
    let (api, relay, login) = mail_and_login().await;
    api.sql(ADD_RESET).await;
    let code = mailed_code(&api, &relay, &login, NEW, 1).await;
    assert_eq!(api.number(RESETS).await, 1);
    let reply = api.confirm_address(&login, NEW, &code).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(api.number(RESETS).await, 0);
}

#[tokio::test]
async fn without_mail_a_new_address_voids_the_reset_link() {
    let (api, login) = logged_in().await;
    api.sql(ADD_RESET).await;
    let body = json!({ "name": "Grace", "email": ADA, "current_password": PASSWORD });
    api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(api.number(RESETS).await, 1);
    assert_eq!(api.ask_address(&login, NEW).await.status, 200);
    assert_eq!(api.number(RESETS).await, 0);
}
