//! Registration with a code sent by mail (ADR 0026).

mod common;

use cadocs_mail::fake::{Behavior, FakeRelay, Received};
use common::mail::{before_setup, body, messages, settle, with_mail, with_relay, with_relay_and};
use common::{Api, COOKIE, Login, PASSWORD, Reply, start};
use serde_json::json;

const GRACE: &str = "grace@example.org";

impl Api {
    async fn register(&self, email: &str) -> Reply {
        self.register_from(email, &[]).await
    }

    async fn register_from(&self, email: &str, headers: &[(&str, &str)]) -> Reply {
        let body = json!({ "email": email });
        self.send("POST", "/auth/register", headers, body).await
    }

    async fn confirm(&self, email: &str, code: &str) -> Reply {
        let body = json!({ "email": email, "code": code, "name": "Grace", "password": PASSWORD });
        self.send("POST", "/auth/register/confirm", &[], body).await
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

async fn mailed_code(api: &Api, relay: &FakeRelay, email: &str, count: usize) -> String {
    let reply = api.register(email).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    code(&messages(relay, count).await[count - 1])
}

async fn users(api: &Api) -> i64 {
    api.number("SELECT count(*) FROM users").await
}

#[tokio::test]
async fn a_mailed_code_makes_an_account_with_no_workspace() {
    let (api, relay) = with_mail().await;
    let reply = api.register(" Grace@Example.org ").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(reply.bytes.is_empty());

    let message = &messages(&relay, 1).await[0];
    assert_eq!(message.to, "<grace@example.org>");
    assert!(message.data.contains("Subject: Your cadocs code"));
    let code = code(message);

    let reply = api.confirm("GRACE@example.org", &code).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert!(reply.cookie().starts_with(COOKIE));
    assert_eq!(reply.body["user"]["email"], GRACE);
    assert_eq!(reply.body["user"]["name"], "Grace");
    assert_eq!(reply.body["user"]["is_instance_admin"], false);
    assert_eq!(reply.body["workspaces"], json!([]));
    assert_eq!(reply.body["can_create_workspace"], true);
    assert_eq!(api.session(&Login::from(&reply)).await.status, 200);

    let verified = "SELECT count(*) FROM users WHERE email = 'grace@example.org'
                    AND email_verified_at IS NOT NULL";
    assert_eq!(api.number(verified).await, 1);
    assert_eq!(api.number("SELECT count(*) FROM email_codes").await, 0);
    assert_eq!(api.login(GRACE, PASSWORD).await.status, 200);
    // Used once.
    assert_eq!(
        api.confirm(GRACE, &code).await.body["error"]["fields"]["code"],
        "wrong"
    );
}

#[tokio::test]
async fn only_a_hash_of_the_code_is_stored() {
    let (api, relay) = with_mail().await;
    let code = mailed_code(&api, &relay, GRACE, 1).await;
    let stored = api
        .number(&format!(
            "SELECT count(*) FROM email_codes WHERE instr(code_hash, '{code}') > 0"
        ))
        .await;
    assert_eq!(stored, 0);
    let rows = "SELECT count(*) FROM email_codes
                WHERE email = 'grace@example.org' AND purpose = 'register' AND tries = 0";
    assert_eq!(api.number(rows).await, 1);
}

#[tokio::test]
async fn five_wrong_codes_kill_a_code() {
    let (api, relay) = with_mail().await;
    let code = mailed_code(&api, &relay, GRACE, 1).await;
    for _ in 0..5 {
        let reply = api.confirm(GRACE, &wrong(&code)).await;
        assert_eq!(reply.status, 400, "{reply:?}");
        assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
    }
    let reply = api.confirm(GRACE, &code).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
    assert_eq!(users(&api).await, 1);
    assert_eq!(api.number("SELECT count(*) FROM email_codes").await, 0);
}

#[tokio::test]
async fn four_wrong_codes_leave_the_right_one_working() {
    let (api, relay) = with_mail().await;
    let code = mailed_code(&api, &relay, GRACE, 1).await;
    for _ in 0..4 {
        assert_eq!(api.confirm(GRACE, &wrong(&code)).await.status, 400);
    }
    let reply = api.confirm(GRACE, &code).await;
    assert_eq!(reply.status, 201, "{reply:?}");
}

#[tokio::test]
async fn a_code_works_for_its_address_only() {
    let (api, relay) = with_mail().await;
    let code = mailed_code(&api, &relay, GRACE, 1).await;
    let reply = api.confirm("alan@example.org", &code).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
    assert_eq!(api.confirm(GRACE, &code).await.status, 201);
}

#[tokio::test]
async fn an_expired_code_is_wrong() {
    let (api, relay) = with_mail().await;
    let code = mailed_code(&api, &relay, GRACE, 1).await;
    api.sql("UPDATE email_codes SET expires_at = 1").await;
    let reply = api.confirm(GRACE, &code).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
    assert_eq!(users(&api).await, 1);
}

#[tokio::test]
async fn asking_again_replaces_the_code() {
    let (api, relay) = with_mail().await;
    let first = mailed_code(&api, &relay, GRACE, 1).await;
    let second = mailed_code(&api, &relay, GRACE, 2).await;
    if first != second {
        assert_eq!(api.confirm(GRACE, &first).await.status, 400);
    }
    assert_eq!(api.confirm(GRACE, &second).await.status, 201);
}

#[tokio::test]
async fn an_address_with_an_account_is_told_nothing() {
    let (api, relay) = with_mail().await;
    let taken = api.register("ada@example.org").await;
    let free = api.register(GRACE).await;
    assert_eq!(taken.status, 204, "{taken:?}");
    assert_eq!(taken.bytes, free.bytes);
    let names = |reply: &Reply| -> Vec<String> {
        let names = reply.headers.iter().map(|(name, _)| name.clone());
        names.filter(|name| name != "date").collect()
    };
    assert_eq!(names(&taken), names(&free));

    // The owner of the account learns that someone tried, and gets no code.
    let all = messages(&relay, 2).await;
    let to_ada = all.iter().find(|m| m.to == "<ada@example.org>").unwrap();
    assert!(
        body(to_ada).contains("already has an account"),
        "{to_ada:?}"
    );
    assert!(
        !body(to_ada)
            .split(|c: char| !c.is_ascii_digit())
            .any(|w| w.len() == 6)
    );
    let rows = "SELECT count(*) FROM email_codes WHERE email = 'ada@example.org'";
    assert_eq!(api.number(rows).await, 0);
    // So any code for it is wrong, as for an address that asked for nothing.
    let reply = api.confirm("ada@example.org", "123456").await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["code"], "wrong");
}

#[tokio::test]
async fn a_disabled_account_gets_no_message() {
    let (api, relay) = with_mail().await;
    api.add_user("u2", GRACE).await;
    api.sql("UPDATE users SET disabled_at = 1 WHERE id = 'u2'")
        .await;
    assert_eq!(api.register(GRACE).await.status, 204);
    settle().await;
    assert!(relay.messages().is_empty());
    assert_eq!(api.number("SELECT count(*) FROM email_codes").await, 0);
}

#[tokio::test]
async fn an_account_made_meanwhile_keeps_its_address() {
    let (api, relay) = with_mail().await;
    let code = mailed_code(&api, &relay, GRACE, 1).await;
    api.add_user("u2", GRACE).await;
    let reply = api.confirm(GRACE, &code).await;
    assert_eq!(reply.status, 409, "{reply:?}");
    assert_eq!(users(&api).await, 2);
}

#[tokio::test]
async fn bad_fields_are_refused_by_name() {
    let (api, relay) = with_mail().await;
    for (email, problem) in [("", "required"), ("grace", "invalid")] {
        let reply = api.register(email).await;
        assert_eq!(reply.status, 400, "{reply:?}");
        assert_eq!(reply.body["error"]["fields"]["email"], problem);
    }
    settle().await;
    assert!(relay.messages().is_empty());

    let body = json!({ "email": "grace", "code": "", "name": " ", "password": "short" });
    let reply = api.send("POST", "/auth/register/confirm", &[], body).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    let fields = &reply.body["error"]["fields"];
    assert_eq!(fields["email"], "invalid");
    assert_eq!(fields["code"], "required");
    assert_eq!(fields["name"], "required");
    assert_eq!(fields["password"], "too_short");
}

#[tokio::test]
async fn a_refused_field_leaves_the_code_its_tries() {
    let (api, relay) = with_mail().await;
    let code = mailed_code(&api, &relay, GRACE, 1).await;
    let body = json!({ "email": GRACE, "code": code, "name": "Grace", "password": "short" });
    let reply = api.send("POST", "/auth/register/confirm", &[], body).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(api.number("SELECT tries FROM email_codes").await, 0);
    assert_eq!(api.confirm(GRACE, &code).await.status, 201);
}

#[tokio::test]
async fn sending_shares_the_budget_of_each_client() {
    let (api, _relay) = with_mail().await;
    for n in 0..3 {
        let email = format!("user{n}@example.org");
        let body = json!({ "email": email });
        let reply = api.send("POST", "/auth/forgot-password", &[], body).await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }
    for n in 3..5 {
        assert_eq!(
            api.register(&format!("user{n}@example.org")).await.status,
            204
        );
    }
    let reply = api.register("user5@example.org").await;
    assert_eq!(reply.status, 429, "{reply:?}");
}

#[tokio::test]
async fn an_address_gets_a_few_codes_whoever_asks() {
    let (api, relay) = with_relay(Behavior::default(), "127.0.0.1").await;
    for n in 0..6 {
        let client = format!("203.0.113.{n}");
        let headers = [("x-forwarded-for", client.as_str())];
        let reply = api.register_from(GRACE, &headers).await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }
    messages(&relay, 3).await;
    settle().await;
    assert_eq!(relay.messages().len(), 3);
}

#[tokio::test]
async fn confirming_shares_the_budget_of_password_guesses() {
    let (api, _relay) = with_mail().await;
    let mut statuses = Vec::new();
    for _ in 0..12 {
        statuses.push(api.confirm(GRACE, "123456").await.status);
    }
    // Setup took one.
    assert_eq!(statuses.iter().filter(|status| **status == 429).count(), 3);
}

#[tokio::test]
async fn a_closed_instance_has_no_registration() {
    let settings = [("CADOCS_REGISTRATION", "closed")];
    let (api, relay) = with_relay_and(Behavior::default(), "", &settings).await;
    for path in ["/auth/register", "/auth/register/confirm"] {
        let reply = api.send("POST", path, &[], json!({ "email": GRACE })).await;
        assert_eq!(reply.status, 404, "{path} {reply:?}");
    }
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(reply.body, json!({ "required": false, "mail": true }));
    // Recovery stays.
    let body = json!({ "email": "ada@example.org" });
    let reply = api.send("POST", "/auth/forgot-password", &[], body).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    messages(&relay, 1).await;
}

#[tokio::test]
async fn with_mail_off_registration_does_not_exist() {
    let api = start().await;
    assert_eq!(api.setup().await.status, 201);
    for path in ["/auth/register", "/auth/register/confirm"] {
        let reply = api.send("POST", path, &[], json!({ "email": GRACE })).await;
        assert_eq!(reply.status, 404, "{path} {reply:?}");
        assert_eq!(reply.code(), "not_found");
        assert_eq!(api.raw("GET", path, &[], b"").await.status, 404);
        assert_eq!(api.raw("POST", path, &[], b"{").await.status, 404);
    }
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(reply.body, json!({ "required": false }));
}

#[tokio::test]
async fn setup_state_says_whether_registration_is_open() {
    let (api, _relay) = with_mail().await;
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(
        reply.body,
        json!({ "required": false, "mail": true, "registration": true })
    );
}

#[tokio::test]
async fn a_foreign_origin_cannot_register() {
    let (api, relay) = with_mail().await;
    let headers = [("origin", "https://evil.example")];
    let reply = api.register_from(GRACE, &headers).await;
    assert_eq!(reply.status, 403, "{reply:?}");
    settle().await;
    assert!(relay.messages().is_empty());
}

#[tokio::test]
async fn no_one_registers_before_first_run_setup() {
    let (api, relay) = before_setup(Behavior::default(), "", &[]).await;
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(reply.body, json!({ "required": true, "mail": true }));

    // The same answer, and no code: an account would take the instance from its operator.
    assert_eq!(api.register(GRACE).await.status, 204);
    settle().await;
    assert!(relay.messages().is_empty());
    assert_eq!(api.number("SELECT count(*) FROM email_codes").await, 0);
    let reply = api.confirm(GRACE, "123456").await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(users(&api).await, 0);
    assert_eq!(api.setup().await.status, 201);
}
