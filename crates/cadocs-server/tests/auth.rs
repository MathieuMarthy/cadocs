//! Accounts and sessions over real sockets: `ConnectInfo`, which the login limit reads, only exists
//! there.

mod common;

use cadocs_store::{AccountStore, NewSession, StoreError, UpdateBasis, UserChanges};
use common::{Api, Login, ORIGIN, PASSWORD, logged_in, start, start_with};
use serde_json::{Value, json};

const FIVE_MINUTES_MS: i64 = 5 * 60 * 1000;
const THIRTY_DAYS_SECS: i64 = 30 * 24 * 60 * 60;

#[tokio::test]
async fn first_run_creates_an_admin_who_owns_a_workspace() {
    let api = start().await;
    let reply = api.setup().await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["user"]["email"], "ada@example.org");
    assert_eq!(reply.body["user"]["name"], "Ada");
    assert_eq!(reply.body["user"]["is_instance_admin"], true);
    assert_eq!(reply.body["workspaces"][0]["name"], "Acme Docs");
    assert_eq!(reply.body["workspaces"][0]["slug"], "acme-docs");
    assert_eq!(reply.body["workspaces"][0]["role"], "owner");

    let stored: String = sqlx::query_scalar("SELECT password_hash FROM users")
        .fetch_one(&api.db)
        .await
        .unwrap();
    assert!(stored.starts_with("$argon2id$"), "{stored}");

    let session = api.session(&Login::from(&reply)).await;
    assert_eq!(session.status, 200);
    assert_eq!(session.body, reply.body);
}

#[tokio::test]
async fn setup_works_only_once() {
    let (api, _) = logged_in().await;
    let again = api.setup().await;
    assert_eq!(again.status, 409);
    assert_eq!(again.code(), "conflict");
    assert_eq!(api.number("SELECT count(*) FROM users").await, 1);
    assert_eq!(api.number("SELECT count(*) FROM workspaces").await, 1);
}

#[tokio::test]
async fn two_setups_at_once_make_one_account() {
    let api = start().await;
    let (first, second) = tokio::join!(api.setup(), api.setup());
    let mut statuses = [first.status, second.status];
    statuses.sort_unstable();
    assert_eq!(statuses, [201, 409]);
    assert_eq!(api.number("SELECT count(*) FROM users").await, 1);
    assert_eq!(api.number("SELECT count(*) FROM workspaces").await, 1);
    assert_eq!(api.number("SELECT count(*) FROM sessions").await, 1);
}

#[tokio::test]
async fn setup_names_the_fields_it_refuses() {
    let api = start().await;
    let body = json!({
        "email": "not an email",
        "name": "  ",
        "password": "short",
        "workspace_name": "",
    });
    let reply = api.send("POST", "/auth/setup", &[], body).await;
    assert_eq!(reply.status, 400);
    assert_eq!(reply.code(), "validation_failed");
    assert_eq!(
        reply.body["error"]["fields"],
        json!({
            "email": "invalid",
            "name": "required",
            "password": "too_short",
            "workspace_name": "required",
        })
    );
    assert_eq!(api.number("SELECT count(*) FROM users").await, 0);
}

#[tokio::test]
async fn the_cookie_has_the_flags_of_the_api_guide() {
    let api = start().await;
    let reply = api.setup().await;
    let cookie = reply.header("set-cookie").unwrap();
    assert!(cookie.starts_with("session="), "{cookie}");
    for flag in [
        "HttpOnly",
        "Secure",
        "SameSite=Lax",
        "Path=/",
        &format!("Max-Age={THIRTY_DAYS_SECS}"),
    ] {
        assert!(cookie.split("; ").any(|part| part == flag), "{cookie}");
    }
}

#[tokio::test]
async fn plain_http_drops_the_secure_flag() {
    let api = start_with("http://127.0.0.1:3000", "").await;
    let reply = api.setup().await;
    let cookie = reply.header("set-cookie").unwrap();
    assert!(!cookie.contains("Secure"), "{cookie}");
}

#[tokio::test]
async fn only_a_hash_of_the_token_is_stored() {
    let (api, login) = logged_in().await;
    let token = login.cookie.strip_prefix("session=").unwrap();
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM sessions")
        .fetch_one(&api.db)
        .await
        .unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(stored.len(), 64);
    assert_ne!(stored, token);
}

#[tokio::test]
async fn login_opens_a_session_and_ignores_the_case_of_the_email() {
    let (api, _) = logged_in().await;
    let reply = api.login("ADA@example.ORG", PASSWORD).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["user"]["email"], "ada@example.org");
    assert_eq!(api.session(&Login::from(&reply)).await.status, 200);
    assert_eq!(api.number("SELECT count(*) FROM sessions").await, 2);
}

#[tokio::test]
async fn a_failed_login_does_not_say_what_was_wrong() {
    let (api, _) = logged_in().await;
    api.add_user("disabled", "off@example.org").await;
    api.sql("UPDATE users SET disabled_at = 1 WHERE id = 'disabled'")
        .await;
    api.add_user("oidc", "oidc@example.org").await;
    api.sql("UPDATE users SET password_hash = NULL WHERE id = 'oidc'")
        .await;

    let wrong = api.login("ada@example.org", "wrong password").await;
    assert_eq!(wrong.status, 401);
    assert_eq!(wrong.code(), "unauthenticated");
    assert!(wrong.header("set-cookie").is_none());
    for email in ["nobody@example.org", "off@example.org", "oidc@example.org"] {
        let reply = api.login(email, PASSWORD).await;
        assert_eq!(reply.status, 401, "{email}");
        assert_eq!(reply.body, wrong.body, "{email}");
    }
    assert_eq!(api.number("SELECT count(*) FROM sessions").await, 1);
}

#[tokio::test]
async fn no_valid_cookie_means_401() {
    let (api, _) = logged_in().await;
    let invalid = "0".repeat(64);
    for cookie in [
        String::new(),
        "session=".to_owned(),
        "session=nonsense".to_owned(),
        format!("session={invalid}"),
        format!("other={invalid}"),
    ] {
        let reply = api
            .raw("GET", "/session", &[("cookie", &cookie)], b"")
            .await;
        assert_eq!(reply.status, 401, "{cookie:?}");
        assert_eq!(reply.code(), "unauthenticated");
    }
}

#[tokio::test]
async fn the_cookie_is_found_among_others() {
    let (api, login) = logged_in().await;
    let cookie = format!("theme=dark; {}; lang=en", login.cookie);
    let reply = api
        .raw("GET", "/session", &[("cookie", &cookie)], b"")
        .await;
    assert_eq!(reply.status, 200);
}

#[tokio::test]
async fn an_expired_session_is_refused() {
    let (api, login) = logged_in().await;
    api.sql("UPDATE sessions SET expires_at = 1").await;
    assert_eq!(api.session(&login).await.status, 401);
}

#[tokio::test]
async fn a_disabled_user_loses_their_session_at_once() {
    let (api, login) = logged_in().await;
    api.sql("UPDATE users SET disabled_at = 1").await;
    assert_eq!(api.session(&login).await.status, 401);
}

#[tokio::test]
async fn use_extends_a_session_at_most_every_five_minutes() {
    let (api, login) = logged_in().await;

    // A recent session: reading it writes nothing.
    api.sql("UPDATE sessions SET expires_at = expires_at - 1000")
        .await;
    let before = api.number("SELECT expires_at FROM sessions").await;
    let recent = api.session(&login).await;
    assert_eq!(recent.status, 200);
    assert!(recent.header("set-cookie").is_none());
    assert_eq!(api.number("SELECT expires_at FROM sessions").await, before);

    // Last seen long ago: the row and the cookie both move forward.
    api.sql(&format!(
        "UPDATE sessions SET last_seen_at = last_seen_at - {FIVE_MINUTES_MS}"
    ))
    .await;
    let seen = api.number("SELECT last_seen_at FROM sessions").await;
    let old = api.session(&login).await;
    assert_eq!(old.status, 200);
    assert_eq!(old.cookie(), login.cookie);
    assert!(old.header("set-cookie").unwrap().contains("Max-Age="));
    assert!(api.number("SELECT expires_at FROM sessions").await > before);
    assert!(api.number("SELECT last_seen_at FROM sessions").await > seen);
}

#[tokio::test]
async fn logout_ends_the_session_and_clears_the_cookie() {
    let (api, login) = logged_in().await;
    let reply = api
        .send("POST", "/auth/logout", &login.headers(), Value::Null)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let cookie = reply.header("set-cookie").unwrap();
    assert!(cookie.starts_with("session=;"), "{cookie}");
    assert!(cookie.contains("Max-Age=0"), "{cookie}");
    assert_eq!(api.session(&login).await.status, 401);
    assert_eq!(api.number("SELECT count(*) FROM sessions").await, 0);
}

#[tokio::test]
async fn a_mutating_request_needs_the_csrf_token_of_its_session() {
    let (api, login) = logged_in().await;
    let other = Login::from(&api.login("ada@example.org", PASSWORD).await);
    let bad = [
        vec![("cookie", login.cookie.as_str())],
        vec![("cookie", &login.cookie), ("x-csrf-token", "nonsense")],
        vec![("cookie", &login.cookie), ("x-csrf-token", &other.csrf)],
    ];
    for headers in bad {
        let reply = api
            .send("POST", "/auth/logout", &headers, Value::Null)
            .await;
        assert_eq!(reply.status, 403, "{headers:?}");
        assert_eq!(reply.code(), "csrf_failed");
        let patch = json!({ "name": "Mallory" });
        let reply = api.send("PATCH", "/me", &headers, patch).await;
        assert_eq!(reply.code(), "csrf_failed");
        let reply = api.send("DELETE", "/me", &headers, Value::Null).await;
        assert_eq!(reply.code(), "csrf_failed");
    }
    assert_eq!(api.session(&login).await.status, 200);
}

#[tokio::test]
async fn no_session_is_a_401_before_any_csrf_check() {
    let api = start().await;
    let reply = api.send("POST", "/auth/logout", &[], Value::Null).await;
    assert_eq!(reply.status, 401);
    assert_eq!(reply.code(), "unauthenticated");
}

#[tokio::test]
async fn a_foreign_origin_cannot_log_in() {
    let (api, _) = logged_in().await;
    let body = json!({ "email": "ada@example.org", "password": PASSWORD });
    let origin = [("origin", "https://evil.example")];
    let reply = api.send("POST", "/auth/login", &origin, body).await;
    assert_eq!(reply.status, 403);
    assert_eq!(reply.code(), "forbidden");
}

#[tokio::test]
async fn a_form_cannot_log_in() {
    // A cross-site form sends no JSON type, and a JSON type would need a CORS preflight.
    let (api, _) = logged_in().await;
    let form = [("content-type", "text/plain")];
    let body = json!({ "email": "ada@example.org", "password": PASSWORD }).to_string();
    let reply = api.raw("POST", "/auth/login", &form, body.as_bytes()).await;
    assert_eq!(reply.status, 400);
    assert_eq!(reply.code(), "invalid_request");
}

#[tokio::test]
async fn bad_bodies_answer_in_the_error_shape() {
    let (api, _) = logged_in().await;
    let json_type = [("content-type", "application/json")];
    let reply = api.raw("POST", "/auth/login", &json_type, b"{oops").await;
    assert_eq!(reply.status, 400);
    assert_eq!(reply.code(), "invalid_request");

    let huge = json!({ "email": "a@b.c", "password": "x".repeat(100_000) });
    let reply = api.send("POST", "/auth/login", &[], huge).await;
    assert_eq!(reply.status, 413);
    assert_eq!(reply.code(), "too_large");
}

#[tokio::test]
async fn a_wrong_method_on_an_auth_route_is_405() {
    let api = start().await;
    let reply = api.raw("GET", "/auth/login", &[], b"").await;
    assert_eq!(reply.status, 405);
    assert_eq!(reply.code(), "method_not_allowed");
}

#[tokio::test]
async fn past_the_limit_logins_get_429_and_a_delay() {
    let (api, _) = logged_in().await;
    let mut replies = Vec::new();
    for _ in 0..30 {
        replies.push(api.login("ada@example.org", "wrong password").await);
    }
    let limited = replies.iter().find(|reply| reply.status == 429);
    let limited = limited.expect("the limit must be reached");
    assert_eq!(limited.code(), "rate_limited");
    let wait: u64 = limited.header("retry-after").unwrap().parse().unwrap();
    assert!(wait >= 1);

    // Even the right password waits.
    assert_eq!(api.login("ada@example.org", PASSWORD).await.status, 429);
}

#[tokio::test]
async fn behind_a_trusted_proxy_each_client_has_its_own_login_limit() {
    let api = start_with(ORIGIN, "127.0.0.1").await;
    api.setup().await;
    let body = json!({ "email": "ada@example.org", "password": "wrong password" });
    let first = [("x-forwarded-for", "203.0.113.1")];
    let mut last = 0;
    for _ in 0..30 {
        last = api
            .send("POST", "/auth/login", &first, body.clone())
            .await
            .status;
    }
    assert_eq!(last, 429);
    let second = [("x-forwarded-for", "203.0.113.2")];
    let reply = api.send("POST", "/auth/login", &second, body).await;
    assert_eq!(reply.status, 401);
}

#[tokio::test]
async fn setup_is_rate_limited_too() {
    let (api, _) = logged_in().await;
    let mut last = 0;
    for _ in 0..30 {
        last = api.setup().await.status;
    }
    assert_eq!(last, 429);
}

#[tokio::test]
async fn a_name_changes_without_the_password() {
    let (api, login) = logged_in().await;
    let body = json!({ "name": "  Grace " });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["name"], "Grace");
    assert_eq!(api.session(&login).await.body["user"]["name"], "Grace");
}

#[tokio::test]
async fn an_email_change_needs_the_current_password() {
    let (api, login) = logged_in().await;
    let headers = login.headers();

    let reply = api
        .send(
            "PATCH",
            "/me",
            &headers,
            json!({ "email": "new@example.org" }),
        )
        .await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        reply.body["error"]["fields"],
        json!({ "current_password": "required" })
    );

    let wrong = json!({ "email": "new@example.org", "current_password": "wrong password" });
    let reply = api.send("PATCH", "/me", &headers, wrong).await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        reply.body["error"]["fields"],
        json!({ "current_password": "wrong" })
    );

    let right = json!({ "email": "NEW@example.org", "current_password": PASSWORD });
    let reply = api.send("PATCH", "/me", &headers, right).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email"], "new@example.org");
    assert_eq!(api.login("new@example.org", PASSWORD).await.status, 200);
    assert_eq!(api.login("ada@example.org", PASSWORD).await.status, 401);
}

#[tokio::test]
async fn an_email_in_use_is_a_conflict() {
    let (api, login) = logged_in().await;
    api.add_user("other", "taken@example.org").await;
    let body = json!({ "email": "taken@example.org", "current_password": PASSWORD });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(reply.status, 409);
    assert_eq!(reply.code(), "conflict");
    assert_eq!(reply.body["error"]["fields"], json!({ "email": "taken" }));
}

#[tokio::test]
async fn a_new_password_revokes_the_other_sessions() {
    let (api, login) = logged_in().await;
    let other = Login::from(&api.login("ada@example.org", PASSWORD).await);

    let body = json!({ "password": "a brand new password", "current_password": PASSWORD });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");

    assert_eq!(api.session(&login).await.status, 200);
    assert_eq!(api.session(&other).await.status, 401);
    assert_eq!(api.login("ada@example.org", PASSWORD).await.status, 401);
    let reply = api.login("ada@example.org", "a brand new password").await;
    assert_eq!(reply.status, 200);
}

#[tokio::test]
async fn a_changed_name_or_email_keeps_the_other_sessions() {
    let (api, login) = logged_in().await;
    let other = Login::from(&api.login("ada@example.org", PASSWORD).await);
    let body = json!({ "name": "Grace" });
    api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(api.session(&other).await.status, 200);
}

#[tokio::test]
async fn a_new_password_is_validated() {
    let (api, login) = logged_in().await;
    let body = json!({ "password": "short", "current_password": PASSWORD });
    let reply = api.send("PATCH", "/me", &login.headers(), body).await;
    assert_eq!(reply.status, 400);
    assert_eq!(
        reply.body["error"]["fields"],
        json!({ "password": "too_short" })
    );
}

#[tokio::test]
async fn guessing_the_current_password_is_rate_limited() {
    let (api, login) = logged_in().await;
    let body = json!({ "email": "x@example.org", "current_password": "wrong password" });
    let mut last = 0;
    for _ in 0..30 {
        last = api
            .send("PATCH", "/me", &login.headers(), body.clone())
            .await
            .status;
    }
    assert_eq!(last, 429);
}

#[tokio::test]
async fn the_last_owner_cannot_delete_their_account() {
    let (api, login) = logged_in().await;
    // Another instance admin, so only the workspace stands in the way.
    api.add_user("admin", "admin@example.org").await;
    api.sql("UPDATE users SET is_instance_admin = 1 WHERE id = 'admin'")
        .await;
    let reply = api
        .send("DELETE", "/me", &login.headers(), Value::Null)
        .await;
    assert_eq!(reply.status, 409);
    assert_eq!(reply.code(), "conflict");
    assert_eq!(api.session(&login).await.status, 200);
}

#[tokio::test]
async fn the_last_instance_admin_cannot_delete_their_account() {
    let (api, login) = logged_in().await;
    // Another owner, so only the instance stands in the way. A disabled admin does not count.
    api.add_user("owner", "owner@example.org").await;
    api.add_user("admin", "admin@example.org").await;
    api.sql("UPDATE users SET is_instance_admin = 1, disabled_at = 1 WHERE id = 'admin'")
        .await;
    api.sql(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         SELECT id, 'owner', 'owner', 1 FROM workspaces",
    )
    .await;
    let reply = api
        .send("DELETE", "/me", &login.headers(), Value::Null)
        .await;
    assert_eq!(reply.status, 409);
    assert_eq!(reply.code(), "conflict");
}

#[tokio::test]
async fn deleting_an_account_ends_its_sessions() {
    let (api, _) = logged_in().await;
    api.add_user("other", "other@example.org").await;
    api.sql(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         SELECT id, 'other', 'member', 1 FROM workspaces",
    )
    .await;
    let login = Login::from(&api.login("other@example.org", PASSWORD).await);
    assert_eq!(
        api.session(&login).await.body["workspaces"][0]["role"],
        "member"
    );

    let reply = api
        .send("DELETE", "/me", &login.headers(), Value::Null)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(reply.header("set-cookie").unwrap().contains("Max-Age=0"));
    assert_eq!(api.session(&login).await.status, 401);
    assert_eq!(
        api.number("SELECT count(*) FROM users WHERE id = 'other'")
            .await,
        0
    );
    assert_eq!(
        api.number("SELECT count(*) FROM workspace_members").await,
        1
    );
}

#[tokio::test]
async fn a_disabled_owner_does_not_count_as_another_owner() {
    let (api, login) = logged_in().await;
    api.add_user("admin", "admin@example.org").await;
    api.sql("UPDATE users SET is_instance_admin = 1 WHERE id = 'admin'")
        .await;
    api.add_user("owner", "owner@example.org").await;
    api.sql("UPDATE users SET disabled_at = 1 WHERE id = 'owner'")
        .await;
    api.sql(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         SELECT id, 'owner', 'owner', 1 FROM workspaces",
    )
    .await;
    let reply = api
        .send("DELETE", "/me", &login.headers(), Value::Null)
        .await;
    assert_eq!(reply.status, 409, "{reply:?}");
}

#[tokio::test]
async fn a_same_named_cookie_of_another_app_does_not_hide_ours() {
    let (api, login) = logged_in().await;
    let invalid = "0".repeat(64);
    let cookie = format!("session={invalid}; {}", login.cookie);
    let reply = api
        .raw("GET", "/session", &[("cookie", &cookie)], b"")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");

    let headers = [("cookie", cookie.as_str()), ("x-csrf-token", &login.csrf)];
    let reply = api
        .send("POST", "/auth/logout", &headers, Value::Null)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
}

#[tokio::test]
async fn a_cookie_that_is_not_ascii_does_not_hide_ours() {
    let (api, login) = logged_in().await;
    let cookie = format!("name=Zoé; {}", login.cookie);
    let reply = api
        .raw("GET", "/session", &[("cookie", &cookie)], b"")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn changing_a_name_spends_no_password_guess() {
    let (api, login) = logged_in().await;
    for round in 0..30 {
        let body = json!({ "name": format!("Name {round}") });
        let reply = api.send("PATCH", "/me", &login.headers(), body).await;
        assert_eq!(reply.status, 200, "{reply:?}");
    }
    assert_eq!(api.login("ada@example.org", PASSWORD).await.status, 200);
}

fn session_of(user_id: String) -> NewSession {
    NewSession {
        token_hash: "late".to_owned(),
        user_id,
        now: 1,
        expires_at: i64::MAX,
    }
}

async fn stored_user(api: &Api) -> (String, String) {
    sqlx::query_as("SELECT id, password_hash FROM users")
        .fetch_one(&api.db)
        .await
        .unwrap()
}

#[tokio::test]
async fn a_login_checked_against_an_old_password_opens_no_session() {
    let (api, _) = logged_in().await;
    let (id, hash) = stored_user(&api).await;
    let late = session_of(id);

    assert!(!api.store.create_session(&late, "old hash").await.unwrap());
    api.sql("UPDATE users SET disabled_at = 1").await;
    assert!(!api.store.create_session(&late, &hash).await.unwrap());
    assert_eq!(api.number("SELECT count(*) FROM sessions").await, 1);

    api.sql("UPDATE users SET disabled_at = NULL").await;
    assert!(api.store.create_session(&late, &hash).await.unwrap());
    assert_eq!(api.number("SELECT count(*) FROM sessions").await, 2);
}

#[tokio::test]
async fn a_change_checked_against_an_old_password_is_refused() {
    let (api, login) = logged_in().await;
    let (id, hash) = stored_user(&api).await;
    let token = login.cookie.strip_prefix("session=").unwrap();
    let session = cadocs_server::session::token_hash(token);
    let changes = UserChanges {
        password_hash: Some("new hash".to_owned()),
        ..UserChanges::default()
    };

    let stale = UpdateBasis {
        session: &session,
        checked_hash: Some("old hash"),
    };
    let result = api.store.update_user(&id, &changes, &stale, 2).await;
    assert!(
        matches!(result, Err(StoreError::StaleCredentials)),
        "{result:?}"
    );

    let revoked = UpdateBasis {
        session: "revoked",
        checked_hash: Some(&hash),
    };
    let result = api.store.update_user(&id, &changes, &revoked, 2).await;
    assert!(
        matches!(result, Err(StoreError::StaleCredentials)),
        "{result:?}"
    );
    assert_eq!(stored_user(&api).await.1, hash);
    assert_eq!(api.session(&login).await.status, 200);

    let current = UpdateBasis {
        session: &session,
        checked_hash: Some(&hash),
    };
    api.store
        .update_user(&id, &changes, &current, 2)
        .await
        .unwrap();
    assert_eq!(stored_user(&api).await.1, "new hash");
}
