//! A second factor from an authenticator app (RFC 6238), and its recovery codes.

mod common;

use cadocs_server::totp::{code_at, step_at};
use cadocs_store::now_ms;
use common::mail::{messages, settle, with_mail};
use common::{Api, Login, PASSWORD, Reply, logged_in, start};
use data_encoding::BASE32_NOPAD;
use serde_json::{Value, json};

const ADA: &str = "ada@example.org";

impl Api {
    async fn start_totp(&self, by: &Login, body: Value) -> Reply {
        self.send("POST", "/me/totp", &by.headers(), body).await
    }

    async fn confirm_totp(&self, by: &Login, code: &str) -> Reply {
        let body = json!({ "code": code });
        self.send("POST", "/me/totp/confirm", &by.headers(), body)
            .await
    }

    async fn login_totp(&self, email: &str, password: &str, totp: &str) -> Reply {
        let body = json!({ "email": email, "password": password, "totp": totp });
        self.send("POST", "/auth/login", &[], body).await
    }

    /// Forgets which step was used last, so the code of the moment works again.
    async fn forget_step(&self) {
        self.sql("UPDATE user_totp SET last_step = 0").await;
    }

    async fn sessions(&self) -> i64 {
        self.number("SELECT count(*) FROM sessions").await
    }
}

fn now_code(secret: &[u8]) -> String {
    code_at(secret, step_at(now_ms()))
}

/// Another code than `code`.
fn wrong(code: &str) -> String {
    let number: u32 = code.parse().unwrap();
    format!("{:06}", (number + 1) % 1_000_000)
}

fn asks_totp(reply: &Reply, problem: &str) {
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["totp"], problem, "{reply:?}");
    assert!(reply.header("set-cookie").is_none(), "{reply:?}");
}

fn recovery_codes(reply: &Reply) -> Vec<String> {
    assert_eq!(reply.status, 200, "{reply:?}");
    let codes = reply.body["recovery_codes"].as_array().unwrap();
    codes
        .iter()
        .map(|c| c.as_str().unwrap().to_owned())
        .collect()
}

/// Starts the setup of the app, and returns the secret it answered.
async fn started(api: &Api, login: &Login) -> Vec<u8> {
    let reply = api
        .start_totp(login, json!({ "current_password": PASSWORD }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let secret = reply.body["secret"].as_str().unwrap();
    let uri = reply.body["uri"].as_str().unwrap();
    assert!(uri.starts_with("otpauth://totp/"), "{uri}");
    assert!(uri.contains(&format!("secret={secret}")), "{uri}");
    BASE32_NOPAD.decode(secret.as_bytes()).unwrap()
}

/// The app on for the first account: its secret and its recovery codes.
async fn totp_on(api: &Api, login: &Login) -> (Vec<u8>, Vec<String>) {
    let secret = started(api, login).await;
    let codes = recovery_codes(&api.confirm_totp(login, &now_code(&secret)).await);
    api.forget_step().await;
    (secret, codes)
}

#[tokio::test]
async fn setting_up_the_app_needs_the_current_password() {
    let (api, login) = logged_in().await;
    let reply = api.start_totp(&login, json!({})).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(
        reply.body["error"]["fields"]["current_password"],
        "required"
    );
    let body = json!({ "current_password": "wrong password" });
    let reply = api.start_totp(&login, body).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["current_password"], "wrong");
    assert_eq!(api.number("SELECT count(*) FROM user_totp").await, 0);
}

#[tokio::test]
async fn a_code_from_the_app_turns_it_on_and_gives_recovery_codes() {
    let (api, login) = logged_in().await;
    let secret = started(&api, &login).await;
    // Pending, it asks nothing at login.
    assert_eq!(api.session(&login).await.body["user"]["totp"], false);
    assert_eq!(api.login(ADA, PASSWORD).await.status, 200);

    let reply = api.confirm_totp(&login, "").await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], "required");
    let reply = api.confirm_totp(&login, &wrong(&now_code(&secret))).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
    assert_eq!(api.session(&login).await.body["user"]["totp"], false);

    let codes = recovery_codes(&api.confirm_totp(&login, &now_code(&secret)).await);
    assert_eq!(codes.len(), 10);
    assert_eq!(api.session(&login).await.body["user"]["totp"], true);
    // Stored hashed.
    let stored = api
        .number(&format!(
            "SELECT count(*) FROM user_recovery_codes WHERE code_hash = '{}'",
            codes[0]
        ))
        .await;
    assert_eq!(stored, 0);
    assert_eq!(
        api.number("SELECT count(*) FROM user_recovery_codes").await,
        10
    );
}

#[tokio::test]
async fn a_new_setup_while_on_is_refused() {
    let (api, login) = logged_in().await;
    totp_on(&api, &login).await;
    let reply = api
        .start_totp(&login, json!({ "current_password": PASSWORD }))
        .await;
    assert_eq!(reply.status, 409, "{reply:?}");
    // A second confirmation changes nothing either.
    let reply = api.confirm_totp(&login, "123456").await;
    assert_eq!(reply.status, 400, "{reply:?}");
}

#[tokio::test]
async fn a_restarted_setup_replaces_the_secret() {
    let (api, login) = logged_in().await;
    let first = started(&api, &login).await;
    let second = started(&api, &login).await;
    assert_ne!(first, second);
    let reply = api.confirm_totp(&login, &now_code(&first)).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    recovery_codes(&api.confirm_totp(&login, &now_code(&second)).await);
}

#[tokio::test]
async fn a_login_with_the_app_on_needs_its_code() {
    let (api, login) = logged_in().await;
    let (secret, _) = totp_on(&api, &login).await;
    api.sql("DELETE FROM sessions").await;

    asks_totp(&api.login(ADA, PASSWORD).await, "required");
    let code = now_code(&secret);
    asks_totp(&api.login_totp(ADA, PASSWORD, &wrong(&code)).await, "wrong");
    assert_eq!(api.sessions().await, 0);

    let reply = api.login_totp(ADA, PASSWORD, &code).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["user"]["totp"], true);
    assert_eq!(api.sessions().await, 1);
}

#[tokio::test]
async fn a_code_from_the_app_works_once() {
    let (api, login) = logged_in().await;
    let (secret, _) = totp_on(&api, &login).await;
    let step = step_at(now_ms());
    let code = code_at(&secret, step);
    assert_eq!(api.login_totp(ADA, PASSWORD, &code).await.status, 200);
    asks_totp(&api.login_totp(ADA, PASSWORD, &code).await, "wrong");
    // Nor does an older one once a newer one went through.
    let older = code_at(&secret, step - 1);
    asks_totp(&api.login_totp(ADA, PASSWORD, &older).await, "wrong");
    let newer = code_at(&secret, step + 1);
    assert_eq!(api.login_totp(ADA, PASSWORD, &newer).await.status, 200);
}

#[tokio::test]
async fn a_recovery_code_logs_in_once() {
    let (api, login) = logged_in().await;
    let (_, codes) = totp_on(&api, &login).await;
    // Typed in capitals, with spaces.
    let typed = codes[3].to_uppercase().replace('-', " ");
    let reply = api.login_totp(ADA, PASSWORD, &typed).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    asks_totp(&api.login_totp(ADA, PASSWORD, &codes[3]).await, "wrong");
    asks_totp(
        &api.login_totp(ADA, PASSWORD, "abcd-efgh-ijkl-mnop").await,
        "wrong",
    );
    assert_eq!(
        api.number("SELECT count(*) FROM user_recovery_codes").await,
        9
    );
}

#[tokio::test]
async fn a_wrong_password_answers_alike_with_the_app_on() {
    let (api, login) = logged_in().await;
    let (secret, _) = totp_on(&api, &login).await;
    for totp in ["", "123456"] {
        let reply = api.login_totp(ADA, "wrong password", totp).await;
        assert_eq!(reply.status, 401, "{reply:?}");
    }
    // A right code behind a wrong password opens nothing, and is not spent.
    let code = now_code(&secret);
    let reply = api.login_totp(ADA, "wrong password", &code).await;
    assert_eq!(reply.status, 401, "{reply:?}");
    assert_eq!(api.login_totp(ADA, PASSWORD, &code).await.status, 200);
}

#[tokio::test]
async fn guesses_at_the_code_of_an_account_are_bounded() {
    let (api, login) = logged_in().await;
    let (secret, codes) = totp_on(&api, &login).await;
    let bad = wrong(&now_code(&secret));
    for _ in 0..5 {
        asks_totp(&api.login_totp(ADA, PASSWORD, &bad).await, "wrong");
    }
    let reply = api.login_totp(ADA, PASSWORD, &codes[0]).await;
    assert_eq!(reply.status, 429, "{reply:?}");
    assert!(reply.header("retry-after").is_some(), "{reply:?}");
    // A login with no code is not a guess.
    asks_totp(&api.login(ADA, PASSWORD).await, "required");
}

#[tokio::test]
async fn the_app_replaces_the_code_by_mail() {
    let (api, relay) = with_mail().await;
    let login = Login::from(&api.login(ADA, PASSWORD).await);
    let (secret, _) = totp_on(&api, &login).await;
    api.sql("UPDATE users SET email_2fa = 1").await;
    asks_totp(&api.login(ADA, PASSWORD).await, "required");
    settle().await;
    assert!(relay.messages().is_empty());
    let reply = api.login_totp(ADA, PASSWORD, &now_code(&secret)).await;
    assert_eq!(reply.status, 200, "{reply:?}");

    // Off again, the code by mail comes back.
    let body = json!({ "current_password": PASSWORD });
    let reply = api.send("DELETE", "/me/totp", &login.headers(), body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.login(ADA, PASSWORD).await;
    assert_eq!(
        reply.body["error"]["fields"]["code"], "required",
        "{reply:?}"
    );
    messages(&relay, 1).await;
}

#[tokio::test]
async fn turning_the_app_off_needs_the_password() {
    let (api, login) = logged_in().await;
    totp_on(&api, &login).await;
    let reply = api
        .send("DELETE", "/me/totp", &login.headers(), json!({}))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(
        reply.body["error"]["fields"]["current_password"],
        "required"
    );
    let body = json!({ "current_password": "wrong password" });
    let reply = api.send("DELETE", "/me/totp", &login.headers(), body).await;
    assert_eq!(reply.body["error"]["fields"]["current_password"], "wrong");
    assert_eq!(api.session(&login).await.body["user"]["totp"], true);

    let body = json!({ "current_password": PASSWORD });
    let reply = api.send("DELETE", "/me/totp", &login.headers(), body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.session(&login).await.body["user"]["totp"], false);
    assert_eq!(api.number("SELECT count(*) FROM user_totp").await, 0);
    assert_eq!(
        api.number("SELECT count(*) FROM user_recovery_codes").await,
        0
    );
    assert_eq!(api.login(ADA, PASSWORD).await.status, 200);
}

#[tokio::test]
async fn new_recovery_codes_replace_the_old_ones() {
    let (api, login) = logged_in().await;
    let path = "/me/totp/recovery-codes";
    let body = json!({ "current_password": PASSWORD });
    // Nothing to replace while the app is off.
    let reply = api.send("POST", path, &login.headers(), body.clone()).await;
    assert_eq!(reply.status, 409, "{reply:?}");

    let (_, old) = totp_on(&api, &login).await;
    let reply = api.send("POST", path, &login.headers(), json!({})).await;
    assert_eq!(
        reply.body["error"]["fields"]["current_password"],
        "required"
    );
    let new = recovery_codes(&api.send("POST", path, &login.headers(), body).await);
    assert_eq!(new.len(), 10);
    asks_totp(&api.login_totp(ADA, PASSWORD, &old[0]).await, "wrong");
    assert_eq!(api.login_totp(ADA, PASSWORD, &new[0]).await.status, 200);
}

#[tokio::test]
async fn the_routes_of_the_app_need_a_session_and_its_csrf_token() {
    let (api, login) = logged_in().await;
    let routes = [
        ("POST", "/me/totp"),
        ("DELETE", "/me/totp"),
        ("POST", "/me/totp/confirm"),
        ("POST", "/me/totp/recovery-codes"),
    ];
    let body = json!({ "current_password": PASSWORD, "code": "123456" });
    for (method, path) in routes {
        let reply = api.send(method, path, &[], body.clone()).await;
        assert_eq!(reply.status, 401, "{method} {path} {reply:?}");
        let cookie = [("cookie", login.cookie.as_str())];
        let reply = api.send(method, path, &cookie, body.clone()).await;
        assert_eq!(reply.code(), "csrf_failed", "{method} {path} {reply:?}");
    }
    assert_eq!(api.number("SELECT count(*) FROM user_totp").await, 0);
}

#[tokio::test]
async fn the_app_needs_no_relay() {
    let api = start().await;
    assert_eq!(api.setup().await.status, 201);
    let login = Login::from(&api.login(ADA, PASSWORD).await);
    let (secret, _) = totp_on(&api, &login).await;
    asks_totp(&api.login(ADA, PASSWORD).await, "required");
    let reply = api.login_totp(ADA, PASSWORD, &now_code(&secret)).await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn one_account_cannot_touch_the_app_of_another() {
    let (api, ada) = logged_in().await;
    let (secret, _) = totp_on(&api, &ada).await;
    let grace = api.add_and_login("u-grace", "grace@example.org").await;
    // Grace's own setup, confirmed with Ada's code, does nothing to either.
    started(&api, &grace).await;
    let reply = api.confirm_totp(&grace, &now_code(&secret)).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    let body = json!({ "current_password": PASSWORD });
    let reply = api.send("DELETE", "/me/totp", &grace.headers(), body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.session(&ada).await.body["user"]["totp"], true);
    assert_eq!(api.login(ADA, PASSWORD).await.status, 400);
}
