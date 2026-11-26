//! A second factor by mail: a code after the password (ADR 0026).

mod common;

use cadocs_mail::fake::{FakeRelay, Received};
use common::mail::{body, messages, settle, with_mail};
use common::{Api, COOKIE, Login, PASSWORD, Reply, logged_in};
use serde_json::json;

const ADA: &str = "ada@example.org";
const TURN_ON: &str = "UPDATE users SET email_2fa = 1";

impl Api {
    async fn login_with(&self, email: &str, password: &str, code: &str) -> Reply {
        let body = json!({ "email": email, "password": password, "code": code });
        self.send("POST", "/auth/login", &[], body).await
    }

    async fn ask_factor(&self, by: &Login) -> Reply {
        self.send("POST", "/me/email-2fa", &by.headers(), json!({}))
            .await
    }

    async fn confirm_factor(&self, by: &Login, code: &str) -> Reply {
        let body = json!({ "code": code });
        self.send("POST", "/me/email-2fa/confirm", &by.headers(), body)
            .await
    }

    async fn drop_factor(&self, by: &Login, body: serde_json::Value) -> Reply {
        self.send("DELETE", "/me/email-2fa", &by.headers(), body)
            .await
    }

    async fn sessions(&self) -> i64 {
        self.number("SELECT count(*) FROM sessions").await
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

/// Another code than `code`.
fn wrong(code: &str) -> String {
    let number: u32 = code.parse().unwrap();
    format!("{:06}", (number + 1) % 1_000_000)
}

fn asks_code(reply: &Reply, problem: &str) {
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], problem, "{reply:?}");
    assert!(reply.header("set-cookie").is_none(), "{reply:?}");
}

/// Set up with mail, the factor on for the first account, and its sessions gone.
async fn factor_on() -> (Api, FakeRelay) {
    let (api, relay) = with_mail().await;
    api.sql(TURN_ON).await;
    api.sql("DELETE FROM sessions").await;
    (api, relay)
}

/// Logs in with the password, and returns the code mailed as message `count`.
async fn mailed_code(api: &Api, relay: &FakeRelay, count: usize) -> String {
    asks_code(&api.login(ADA, PASSWORD).await, "required");
    code(&messages(relay, count).await[count - 1])
}

#[tokio::test]
async fn a_login_with_the_factor_on_needs_the_mailed_code() {
    let (api, relay) = factor_on().await;
    let code = mailed_code(&api, &relay, 1).await;
    let message = &relay.messages()[0];
    assert_eq!(message.to, "<ada@example.org>");
    assert!(message.data.contains("Subject: Your cadocs login code"));
    assert_eq!(api.sessions().await, 0);

    asks_code(&api.login_with(ADA, PASSWORD, &wrong(&code)).await, "wrong");
    assert_eq!(api.sessions().await, 0);

    let reply = api.login_with(" Ada@Example.org ", PASSWORD, &code).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert!(reply.cookie().starts_with(COOKIE));
    assert_eq!(reply.body["user"]["email_2fa"], true);
    assert_eq!(api.session(&Login::from(&reply)).await.status, 200);
    assert_eq!(api.sessions().await, 1);
    // The first code used proves the address (ADR 0026).
    let verified = "SELECT count(*) FROM users WHERE email_verified_at IS NOT NULL";
    assert_eq!(api.number(verified).await, 1);

    // Used once.
    asks_code(&api.login_with(ADA, PASSWORD, &code).await, "wrong");
}

#[tokio::test]
async fn a_code_is_never_tried_without_the_right_password() {
    let (api, relay) = factor_on().await;
    let code = mailed_code(&api, &relay, 1).await;

    let reply = api.login_with(ADA, "wrong password", &code).await;
    assert_eq!(reply.status, 401, "{reply:?}");
    let reply = api.login("nobody@example.org", PASSWORD).await;
    assert_eq!(reply.status, 401, "{reply:?}");
    settle().await;
    assert_eq!(relay.messages().len(), 1);
    let tries = "SELECT tries FROM email_codes WHERE purpose = 'login'";
    assert_eq!(api.number(tries).await, 0);
    assert_eq!(api.login_with(ADA, PASSWORD, &code).await.status, 200);
}

#[tokio::test]
async fn five_wrong_codes_kill_a_login_code() {
    let (api, relay) = factor_on().await;
    let code = mailed_code(&api, &relay, 1).await;
    for _ in 0..5 {
        asks_code(&api.login_with(ADA, PASSWORD, &wrong(&code)).await, "wrong");
    }
    asks_code(&api.login_with(ADA, PASSWORD, &code).await, "wrong");
    assert_eq!(api.sessions().await, 0);

    // Asking again sends a new one.
    let code = mailed_code(&api, &relay, 2).await;
    assert_eq!(api.login_with(ADA, PASSWORD, &code).await.status, 200);
}

#[tokio::test]
async fn a_disabled_account_gets_no_code() {
    let (api, relay) = factor_on().await;
    api.sql("UPDATE users SET disabled_at = 1").await;
    let reply = api.login(ADA, PASSWORD).await;
    assert_eq!(reply.status, 401, "{reply:?}");
    settle().await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn a_login_without_the_factor_ignores_a_code() {
    let (api, relay) = with_mail().await;
    let reply = api.login_with(ADA, PASSWORD, "123456").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["user"]["email_2fa"], false);
    settle().await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn with_mail_off_the_factor_is_not_asked_and_cannot_be_changed() {
    let (api, login) = logged_in().await;
    api.sql(TURN_ON).await;
    let reply = api.login(ADA, PASSWORD).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["user"]["email_2fa"], true);

    assert_eq!(api.ask_factor(&login).await.status, 404);
    assert_eq!(api.confirm_factor(&login, "123456").await.status, 404);
    let body = json!({ "current_password": PASSWORD });
    assert_eq!(api.drop_factor(&login, body).await.status, 404);
}

#[tokio::test]
async fn turning_the_factor_on_needs_a_code_sent_to_the_address() {
    let (api, relay) = with_mail().await;
    let reply = api.login(ADA, PASSWORD).await;
    let login = Login::from(&reply);
    assert_eq!(reply.body["user"]["email_2fa"], false);

    let reply = api.ask_factor(&login).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let message = &messages(&relay, 1).await[0];
    assert_eq!(message.to, "<ada@example.org>");
    let code = code(message);

    let reply = api.confirm_factor(&login, &wrong(&code)).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
    let reply = api.confirm_factor(&login, "").await;
    assert_eq!(reply.body["error"]["fields"]["code"], "required");
    assert_eq!(api.number("SELECT email_2fa FROM users").await, 0);

    let reply = api.confirm_factor(&login, &code).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.number("SELECT email_2fa FROM users").await, 1);
    let verified = "SELECT count(*) FROM users WHERE email_verified_at IS NOT NULL";
    assert_eq!(api.number(verified).await, 1);
    assert_eq!(api.session(&login).await.body["user"]["email_2fa"], true);
    // The session that turned it on stays.
    assert_eq!(api.session(&login).await.status, 200);

    // A login code is no use to turn it on, and the other way round.
    let login_code = mailed_code(&api, &relay, 2).await;
    let reply = api.confirm_factor(&login, &login_code).await;
    assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
    assert_eq!(api.login_with(ADA, PASSWORD, &login_code).await.status, 200);
}

#[tokio::test]
async fn a_code_to_turn_it_on_is_not_one_to_log_in() {
    let (api, relay) = with_mail().await;
    let login = Login::from(&api.login(ADA, PASSWORD).await);
    assert_eq!(api.ask_factor(&login).await.status, 204);
    let code = code(&messages(&relay, 1).await[0]);
    api.sql(TURN_ON).await;
    asks_code(&api.login_with(ADA, PASSWORD, &code).await, "wrong");
}

#[tokio::test]
async fn turning_the_factor_off_needs_the_current_password() {
    let (api, relay) = with_mail().await;
    let login = Login::from(&api.login(ADA, PASSWORD).await);
    api.sql(TURN_ON).await;

    let reply = api.drop_factor(&login, json!({})).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(
        reply.body["error"]["fields"]["current_password"],
        "required"
    );
    let reply = api
        .drop_factor(&login, json!({ "current_password": "wrong password" }))
        .await;
    assert_eq!(reply.body["error"]["fields"]["current_password"], "wrong");
    assert_eq!(api.number("SELECT email_2fa FROM users").await, 1);

    let body = json!({ "current_password": PASSWORD });
    let reply = api.drop_factor(&login, body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.number("SELECT email_2fa FROM users").await, 0);
    assert_eq!(api.session(&login).await.body["user"]["email_2fa"], false);
    let reply = api.login(ADA, PASSWORD).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    settle().await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn the_routes_of_the_factor_need_a_session() {
    let (api, _relay) = with_mail().await;
    let reply = api.send("POST", "/me/email-2fa", &[], json!({})).await;
    assert_eq!(reply.status, 401, "{reply:?}");
}

#[tokio::test]
async fn login_codes_share_the_limits_of_mail() {
    let (api, relay) = factor_on().await;
    for _ in 0..4 {
        asks_code(&api.login(ADA, PASSWORD).await, "required");
    }
    settle().await;
    // Three per address, then silence: the answer stays the same.
    assert_eq!(relay.messages().len(), 3);
    // Five per client, a budget the other routes that send mail share.
    asks_code(&api.login(ADA, PASSWORD).await, "required");
    let reply = api.login(ADA, PASSWORD).await;
    assert_eq!(reply.status, 429, "{reply:?}");
}

#[tokio::test]
async fn turning_the_factor_on_ends_the_other_sessions() {
    let (api, relay) = with_mail().await;
    let login = Login::from(&api.login(ADA, PASSWORD).await);
    let other = Login::from(&api.login(ADA, PASSWORD).await);
    assert_eq!(api.ask_factor(&login).await.status, 204);
    let code = code(&messages(&relay, 1).await[0]);
    assert_eq!(api.confirm_factor(&login, &code).await.status, 204);
    assert_eq!(api.session(&login).await.status, 200);
    assert_eq!(api.session(&other).await.status, 401);
}
