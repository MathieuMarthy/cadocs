//! Instance administration and password resets, over real sockets.

mod common;

use cadocs_server::session::token_hash;
use cadocs_store::{AccountStore, now_ms};
use common::{Api, Login, ORIGIN, PASSWORD, logged_in};
use serde_json::{Value, json};

const OTHER: &str = "11111111-0000-7000-8000-000000000000";
const NEW_PASSWORD: &str = "a whole new password";

impl Api {
    async fn add_and_login(&self, id: &str, email: &str) -> Login {
        self.add_user(id, email).await;
        let reply = self.login(email, PASSWORD).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        Login::from(&reply)
    }

    async fn patch_user(&self, by: &Login, id: &str, body: Value) -> common::Reply {
        let path = format!("/admin/users/{id}");
        self.send("PATCH", &path, &by.headers(), body).await
    }

    async fn delete_user(&self, by: &Login, id: &str) -> common::Reply {
        let path = format!("/admin/users/{id}");
        self.raw("DELETE", &path, &by.headers(), b"").await
    }

    async fn reset_link(&self, by: &Login, id: &str) -> common::Reply {
        let path = format!("/admin/users/{id}/password-reset");
        self.raw("POST", &path, &by.headers(), b"").await
    }

    /// The token of a link just generated.
    async fn reset_token(&self, by: &Login, id: &str) -> String {
        let reply = self.reset_link(by, id).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        let link = reply.body["link"].as_str().unwrap();
        link.rsplit('/').next().unwrap().to_owned()
    }

    async fn use_reset(&self, token: &str, password: &str) -> common::Reply {
        let path = format!("/auth/password-reset/{token}");
        self.send("POST", &path, &[], json!({ "password": password }))
            .await
    }

    async fn first_user_id(&self) -> String {
        sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(&self.db)
            .await
            .unwrap()
    }
}

#[tokio::test]
async fn only_an_instance_admin_gets_in() {
    let (api, _) = logged_in().await;
    let user = api.add_and_login(OTHER, "bob@example.org").await;
    let admin = api.first_user_id().await;

    let reply = api
        .raw("GET", "/admin/users", &[("cookie", &user.cookie)], b"")
        .await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api
        .patch_user(&user, &admin, json!({ "disabled": true }))
        .await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api.delete_user(&user, &admin).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api.reset_link(&user, &admin).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    // A user cannot promote themself either.
    let reply = api
        .patch_user(&user, OTHER, json!({ "is_instance_admin": true }))
        .await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    let reply = api.raw("GET", "/admin/users", &[], b"").await;
    assert_eq!((reply.status, reply.code()), (401, "unauthenticated"));
}

#[tokio::test]
async fn a_change_needs_the_csrf_token() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;
    let path = format!("/admin/users/{OTHER}");
    let cookie = [("cookie", admin.cookie.as_str())];

    let reply = api
        .send("PATCH", &path, &cookie, json!({ "disabled": true }))
        .await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
    let reply = api.raw("DELETE", &path, &cookie, b"").await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
    let path = format!("{path}/password-reset");
    let reply = api.raw("POST", &path, &cookie, b"").await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
}

#[tokio::test]
async fn users_are_listed_page_by_page() {
    let (api, admin) = logged_in().await;
    api.add_user("zz-1", "bob@example.org").await;
    api.add_user("zz-2", "eve@example.org").await;
    api.sql("UPDATE users SET disabled_at = 7 WHERE id = 'zz-2'")
        .await;
    let cookie = [("cookie", admin.cookie.as_str())];

    let reply = api.raw("GET", "/admin/users?limit=2", &cookie, b"").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["email"], "ada@example.org");
    assert_eq!(items[0]["is_instance_admin"], true);
    assert_eq!(items[0]["disabled_at"], Value::Null);
    assert!(items[0]["created_at"].is_i64());
    assert!(items[0].get("password_hash").is_none());
    assert_eq!(items[1]["id"], "zz-1");

    let cursor = reply.body["next_cursor"].as_str().unwrap();
    let path = format!("/admin/users?limit=2&cursor={cursor}");
    let reply = api.raw("GET", &path, &cookie, b"").await;
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], "zz-2");
    assert_eq!(items[0]["disabled_at"], 7);
    assert_eq!(reply.body["next_cursor"], Value::Null);

    let reply = api.raw("GET", "/admin/users", &cookie, b"").await;
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 3);
    assert_eq!(reply.body["next_cursor"], Value::Null);

    for limit in ["0", "101", "many", ""] {
        let path = format!("/admin/users?limit={limit}");
        let reply = api.raw("GET", &path, &cookie, b"").await;
        assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
        assert_eq!(reply.body["error"]["fields"]["limit"], "invalid");
    }
}

#[tokio::test]
async fn a_disabled_user_is_locked_out_until_enabled() {
    let (api, admin) = logged_in().await;
    let user = api.add_and_login(OTHER, "bob@example.org").await;

    let reply = api
        .patch_user(&admin, OTHER, json!({ "disabled": true }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["email"], "bob@example.org");
    assert!(reply.body["disabled_at"].is_i64());
    let disabled_at = reply.body["disabled_at"].clone();

    assert_eq!(api.session(&user).await.status, 401);
    assert_eq!(api.login("bob@example.org", PASSWORD).await.status, 401);

    // Disabling twice keeps the first date.
    api.sql(&format!(
        "UPDATE users SET disabled_at = 7 WHERE id = '{OTHER}'"
    ))
    .await;
    let reply = api
        .patch_user(&admin, OTHER, json!({ "disabled": true }))
        .await;
    assert_eq!(reply.body["disabled_at"], 7, "was {disabled_at}");

    let reply = api
        .patch_user(&admin, OTHER, json!({ "disabled": false }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["disabled_at"], Value::Null);

    // The old session stays dead: enabling an account must not revive a stolen cookie.
    assert_eq!(api.session(&user).await.status, 401);
    assert_eq!(api.login("bob@example.org", PASSWORD).await.status, 200);
}

#[tokio::test]
async fn an_empty_change_answers_the_user_and_an_unknown_field_is_refused() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;

    let reply = api.patch_user(&admin, OTHER, json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["is_instance_admin"], false);

    let reply = api
        .patch_user(&admin, OTHER, json!({ "email": "x@y.z" }))
        .await;
    assert_eq!((reply.status, reply.code()), (400, "invalid_request"));
}

#[tokio::test]
async fn the_admin_role_applies_at_once() {
    let (api, admin) = logged_in().await;
    let user = api.add_and_login(OTHER, "bob@example.org").await;
    let cookie = [("cookie", user.cookie.as_str())];

    let reply = api
        .patch_user(&admin, OTHER, json!({ "is_instance_admin": true }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["is_instance_admin"], true);
    assert_eq!(
        api.raw("GET", "/admin/users", &cookie, b"").await.status,
        200
    );

    let reply = api
        .patch_user(&admin, OTHER, json!({ "is_instance_admin": false }))
        .await;
    assert_eq!(reply.body["is_instance_admin"], false);
    assert_eq!(
        api.raw("GET", "/admin/users", &cookie, b"").await.status,
        403
    );
}

#[tokio::test]
async fn the_last_enabled_instance_admin_stays() {
    let (api, admin) = logged_in().await;
    let id = api.first_user_id().await;
    api.add_user(OTHER, "bob@example.org").await;
    // An admin who is disabled does not count.
    api.sql(&format!(
        "UPDATE users SET is_instance_admin = 1, disabled_at = 7 WHERE id = '{OTHER}'"
    ))
    .await;

    for change in [
        json!({ "disabled": true }),
        json!({ "is_instance_admin": false }),
        json!({ "disabled": true, "is_instance_admin": true }),
    ] {
        let reply = api.patch_user(&admin, &id, change).await;
        assert_eq!((reply.status, reply.code()), (409, "conflict"), "{reply:?}");
    }
    let reply = api.delete_user(&admin, &id).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"), "{reply:?}");
    assert_eq!(api.session(&admin).await.status, 200);
    assert_eq!(
        api.number(
            "SELECT count(*) FROM users WHERE is_instance_admin = 1 AND disabled_at IS NULL"
        )
        .await,
        1
    );

    // Once the other one is back, either can go.
    let reply = api
        .patch_user(&admin, OTHER, json!({ "disabled": false }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let reply = api
        .patch_user(&admin, &id, json!({ "is_instance_admin": false }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn an_unknown_user_is_a_404() {
    let (api, admin) = logged_in().await;
    let reply = api
        .patch_user(&admin, OTHER, json!({ "disabled": true }))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.delete_user(&admin, OTHER).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.reset_link(&admin, OTHER).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn a_path_that_is_not_text_is_refused_in_the_error_shape() {
    let (api, admin) = logged_in().await;
    let reply = api.delete_user(&admin, "%FF").await;
    assert_eq!((reply.status, reply.code()), (400, "invalid_request"));
    let reply = api.use_reset("%FF", NEW_PASSWORD).await;
    assert_eq!((reply.status, reply.code()), (400, "invalid_request"));
}

#[tokio::test]
async fn deleting_a_user_ends_the_account_and_its_sessions() {
    let (api, admin) = logged_in().await;
    let user = api.add_and_login(OTHER, "bob@example.org").await;

    let reply = api.delete_user(&admin, OTHER).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.session(&user).await.status, 401);
    assert_eq!(api.number("SELECT count(*) FROM users").await, 1);
    assert_eq!(
        api.number(&format!(
            "SELECT count(*) FROM sessions WHERE user_id = '{OTHER}'"
        ))
        .await,
        0
    );
}

impl Api {
    /// Makes `id` an owner of the first workspace.
    async fn add_owner(&self, id: &str) {
        self.sql(&format!(
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             SELECT id, '{id}', 'owner', 1 FROM workspaces"
        ))
        .await;
    }
}

#[tokio::test]
async fn the_last_owner_of_a_workspace_cannot_be_deleted() {
    let (api, admin) = logged_in().await;
    let first = api.first_user_id().await;
    api.add_user(OTHER, "bob@example.org").await;
    api.sql(&format!(
        "UPDATE users SET is_instance_admin = 1 WHERE id = '{OTHER}'"
    ))
    .await;

    let reply = api.delete_user(&admin, &first).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"), "{reply:?}");

    // A second owner who is disabled does not count, as they cannot act.
    api.add_owner(OTHER).await;
    api.sql(&format!(
        "UPDATE users SET disabled_at = 7 WHERE id = '{OTHER}'"
    ))
    .await;
    let reply = api.delete_user(&admin, &first).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"), "{reply:?}");
}

#[tokio::test]
async fn a_disabled_owner_goes_only_if_an_enabled_owner_stays() {
    let (api, admin) = logged_in().await;
    let first = api.first_user_id().await;
    api.add_user(OTHER, "bob@example.org").await;
    api.add_owner(OTHER).await;
    api.sql(&format!(
        "UPDATE users SET disabled_at = 7 WHERE id = '{OTHER}'"
    ))
    .await;

    let reply = api.delete_user(&admin, OTHER).await;
    assert_eq!(reply.status, 204, "{reply:?}");

    // Alone and disabled, an owner stays: the workspace would have none.
    api.add_user(OTHER, "bob@example.org").await;
    api.sql(&format!(
        "UPDATE users SET is_instance_admin = 1 WHERE id = '{OTHER}'"
    ))
    .await;
    let second = api.login("bob@example.org", PASSWORD).await;
    let second = Login::from(&second);
    let reply = api
        .patch_user(&second, &first, json!({ "disabled": true }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let reply = api.delete_user(&second, &first).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"), "{reply:?}");
}

#[tokio::test]
async fn a_reset_link_sets_a_password_and_ends_every_session() {
    let (api, admin) = logged_in().await;
    let user = api.add_and_login(OTHER, "bob@example.org").await;

    let reply = api.reset_link(&admin, OTHER).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    let link = reply.body["link"].as_str().unwrap();
    let token = link
        .strip_prefix(&format!("{ORIGIN}/reset-password/"))
        .unwrap_or_else(|| panic!("{link}"));
    assert_eq!(token.len(), 64);
    let hour_ms = 60 * 60 * 1000;
    let created: i64 = api.number("SELECT created_at FROM password_resets").await;
    assert_eq!(reply.body["expires_at"], created + hour_ms);

    // Only the hash is stored, and the admin who made the link is on record.
    let stored: String = sqlx::query_scalar("SELECT token_hash FROM password_resets")
        .fetch_one(&api.db)
        .await
        .unwrap();
    assert_eq!(stored, token_hash(token));
    let by: Option<String> = sqlx::query_scalar("SELECT created_by FROM password_resets")
        .fetch_one(&api.db)
        .await
        .unwrap();
    assert_eq!(by, Some(api.first_user_id().await));

    // A link alone changes nothing.
    assert_eq!(api.session(&user).await.status, 200);

    let reply = api.use_reset(token, NEW_PASSWORD).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.session(&user).await.status, 401);
    assert_eq!(api.login("bob@example.org", PASSWORD).await.status, 401);
    assert_eq!(api.login("bob@example.org", NEW_PASSWORD).await.status, 200);

    let reply = api.use_reset(token, "yet another password").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    assert_eq!(api.number("SELECT count(*) FROM password_resets").await, 0);
}

#[tokio::test]
async fn a_reset_link_expires() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;
    let token = api.reset_token(&admin, OTHER).await;

    api.sql("UPDATE password_resets SET expires_at = 1").await;
    let reply = api.use_reset(&token, NEW_PASSWORD).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    assert_eq!(api.login("bob@example.org", PASSWORD).await.status, 200);
}

#[tokio::test]
async fn an_unknown_token_is_a_404_whatever_the_password() {
    let (api, _) = logged_in().await;
    let reply = api.use_reset(&"0".repeat(64), NEW_PASSWORD).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.use_reset(&"0".repeat(64), "short").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn a_refused_password_leaves_the_link_usable() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;
    let token = api.reset_token(&admin, OTHER).await;

    let reply = api.use_reset(&token, "short").await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["password"], "too_short");
    assert_eq!(api.use_reset(&token, NEW_PASSWORD).await.status, 204);
}

#[tokio::test]
async fn using_reset_links_is_rate_limited() {
    let (api, _) = logged_in().await;
    let token = "0".repeat(64);
    // Setup took one from the same budget.
    for _ in 1..cadocs_server::api::auth::GUESS_BURST {
        assert_eq!(api.use_reset(&token, NEW_PASSWORD).await.status, 404);
    }
    let reply = api.use_reset(&token, NEW_PASSWORD).await;
    assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
}

#[tokio::test]
async fn a_new_link_replaces_the_one_before() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;
    let before = api.reset_token(&admin, OTHER).await;
    let after = api.reset_token(&admin, OTHER).await;

    assert_eq!(api.use_reset(&before, NEW_PASSWORD).await.status, 404);
    assert_eq!(api.use_reset(&after, NEW_PASSWORD).await.status, 204);
}

#[tokio::test]
async fn changing_the_password_voids_a_link() {
    let (api, admin) = logged_in().await;
    let user = api.add_and_login(OTHER, "bob@example.org").await;
    let token = api.reset_token(&admin, OTHER).await;

    let body = json!({ "password": NEW_PASSWORD, "current_password": PASSWORD });
    let reply = api.send("PATCH", "/me", &user.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");

    let reply = api.use_reset(&token, "yet another password").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn a_disabled_user_has_no_reset_link() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;
    let token = api.reset_token(&admin, OTHER).await;

    api.patch_user(&admin, OTHER, json!({ "disabled": true }))
        .await;
    let reply = api.reset_link(&admin, OTHER).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"), "{reply:?}");

    // Enabling does not bring the link back.
    api.patch_user(&admin, OTHER, json!({ "disabled": false }))
        .await;
    let reply = api.use_reset(&token, NEW_PASSWORD).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

/// The route checks the link, then hashes for a while: what counts is the state at the write.
#[tokio::test]
async fn the_write_decides_whether_a_link_is_still_good() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;
    let token = api.reset_token(&admin, OTHER).await;
    let hash = token_hash(&token);
    let now = now_ms();

    assert!(api.store.password_reset_usable(&hash, now).await.unwrap());
    let expired = now + 2 * 60 * 60 * 1000;
    assert!(
        !api.store
            .use_password_reset(&hash, "x", expired)
            .await
            .unwrap()
    );
    assert_eq!(api.login("bob@example.org", PASSWORD).await.status, 200);

    assert!(api.store.use_password_reset(&hash, "x", now).await.unwrap());
    assert!(!api.store.use_password_reset(&hash, "y", now).await.unwrap());
    let stored = api.store.password_hash(OTHER).await.unwrap();
    assert_eq!(stored.as_deref(), Some("x"));
}

/// Two admins: the first account, and `OTHER`, logged in. A third account is the target.
async fn two_admins() -> (Api, Login, String, Login) {
    let (api, first) = logged_in().await;
    let first_id = api.first_user_id().await;
    let second = api.add_and_login(OTHER, "bob@example.org").await;
    api.patch_user(&first, OTHER, json!({ "is_instance_admin": true }))
        .await;
    (api, first, first_id, second)
}

#[tokio::test]
async fn an_admin_who_loses_the_role_loses_the_links_they_made() {
    let changes = [
        json!({ "disabled": true }),
        json!({ "is_instance_admin": false }),
    ];
    for change in changes {
        let (api, first, first_id, second) = two_admins().await;
        let token = api.reset_token(&second, &first_id).await;

        let reply = api.patch_user(&first, OTHER, change).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        let reply = api.use_reset(&token, NEW_PASSWORD).await;
        assert_eq!((reply.status, reply.code()), (404, "not_found"));
    }
}

#[tokio::test]
async fn a_deleted_admin_loses_the_links_they_made() {
    let (api, first, first_id, second) = two_admins().await;
    let token = api.reset_token(&second, &first_id).await;

    let reply = api.delete_user(&first, OTHER).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.use_reset(&token, NEW_PASSWORD).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn a_change_that_keeps_the_role_keeps_the_links() {
    let (api, first, first_id, second) = two_admins().await;
    let token = api.reset_token(&second, &first_id).await;

    api.patch_user(&first, OTHER, json!({ "disabled": false }))
        .await;
    assert_eq!(api.use_reset(&token, NEW_PASSWORD).await.status, 204);
}

/// Disabling voids the link, but the rule must not depend on how `disabled_at` was set.
#[tokio::test]
async fn a_link_never_works_on_a_disabled_account() {
    let (api, admin) = logged_in().await;
    api.add_user(OTHER, "bob@example.org").await;
    let token = api.reset_token(&admin, OTHER).await;
    let hash = token_hash(&token);
    api.sql(&format!(
        "UPDATE users SET disabled_at = 7 WHERE id = '{OTHER}'"
    ))
    .await;

    let reply = api.use_reset(&token, NEW_PASSWORD).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    assert!(
        !api.store
            .use_password_reset(&hash, "x", now_ms())
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn administration_answers_are_not_cached() {
    let (api, admin) = logged_in().await;
    let cookie = [("cookie", admin.cookie.as_str())];
    let reply = api.raw("GET", "/admin/users", &cookie, b"").await;
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    let id = api.first_user_id().await;
    let reply = api.patch_user(&admin, &id, json!({})).await;
    assert_eq!(reply.header("cache-control"), Some("no-store"));
}
