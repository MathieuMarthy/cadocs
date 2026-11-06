//! Workspaces, members and invitations, over real sockets.

mod common;

use cadocs_store::{NewWorkspace, StoreError, WorkspaceStore};
use common::{Api, Login, ORIGIN, PASSWORD, Reply, logged_in};
use serde_json::{Value, json};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const CARA: &str = "11111111-0000-7000-8000-000000000002";
const DAN: &str = "11111111-0000-7000-8000-000000000003";

impl Api {
    async fn get(&self, by: &Login, path: &str) -> Reply {
        self.raw("GET", path, &[("cookie", &by.cookie)], b"").await
    }

    async fn first_workspace(&self) -> String {
        sqlx::query_scalar("SELECT id FROM workspaces ORDER BY id LIMIT 1")
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn first_user(&self) -> String {
        sqlx::query_scalar("SELECT id FROM users ORDER BY id LIMIT 1")
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn add_member(&self, workspace: &str, id: &str, email: &str, role: &str) -> Login {
        let login = self.add_and_login(id, email).await;
        self.sql(&format!(
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             VALUES ('{workspace}', '{id}', '{role}', 1)"
        ))
        .await;
        login
    }

    async fn set_role(&self, by: &Login, workspace: &str, user: &str, role: &str) -> Reply {
        let path = format!("/workspaces/{workspace}/members/{user}");
        self.send("PATCH", &path, &by.headers(), json!({ "role": role }))
            .await
    }

    async fn remove(&self, by: &Login, workspace: &str, user: &str) -> Reply {
        let path = format!("/workspaces/{workspace}/members/{user}");
        self.raw("DELETE", &path, &by.headers(), b"").await
    }

    async fn invite(&self, by: &Login, workspace: &str, email: &str, role: &str) -> Reply {
        let path = format!("/workspaces/{workspace}/invitations");
        let body = json!({ "email": email, "role": role });
        self.send("POST", &path, &by.headers(), body).await
    }

    /// The token of an invitation just made.
    async fn invite_token(&self, by: &Login, workspace: &str, email: &str) -> String {
        let reply = self.invite(by, workspace, email, "member").await;
        assert_eq!(reply.status, 201, "{reply:?}");
        let link = reply.body["link"].as_str().unwrap();
        link.rsplit('/').next().unwrap().to_owned()
    }

    async fn accept(&self, token: &str, headers: &[(&str, &str)], body: Value) -> Reply {
        let path = format!("/invitations/{token}/accept");
        self.send("POST", &path, headers, body).await
    }

    async fn accept_as_new(&self, token: &str) -> Reply {
        let body = json!({ "name": "Eve", "password": PASSWORD });
        self.accept(token, &[], body).await
    }

    async fn role_of(&self, workspace: &str, user: &str) -> Option<String> {
        sqlx::query_scalar(
            "SELECT role FROM workspace_members WHERE workspace_id = ? AND user_id = ?",
        )
        .bind(workspace)
        .bind(user)
        .fetch_optional(&self.db)
        .await
        .unwrap()
    }
}

/// The owner of the first workspace, and its id.
async fn workspace() -> (Api, Login, String) {
    let (api, owner) = logged_in().await;
    let id = api.first_workspace().await;
    (api, owner, id)
}

#[tokio::test]
async fn a_member_reads_their_workspaces_and_a_stranger_finds_nothing() {
    let (api, owner, id) = workspace().await;
    let stranger = api.add_and_login(BOB, "bob@example.org").await;

    let reply = api.get(&owner, "/workspaces").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["next_cursor"], Value::Null);
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["id"], id.as_str());
    assert_eq!(items[0]["slug"], "acme-docs");
    assert_eq!(items[0]["name"], "Acme Docs");
    assert_eq!(items[0]["role"], "owner");

    let reply = api.get(&owner, &format!("/workspaces/{id}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["language"], "en");
    assert_eq!(reply.body["role"], "owner");

    let reply = api.get(&stranger, "/workspaces").await;
    assert_eq!(reply.body["items"], json!([]));
    let reply = api.get(&stranger, &format!("/workspaces/{id}")).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api
        .get(&stranger, &format!("/workspaces/{id}/members"))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));

    let reply = api.raw("GET", "/workspaces", &[], b"").await;
    assert_eq!((reply.status, reply.code()), (401, "unauthenticated"));
}

#[tokio::test]
async fn the_list_of_workspaces_has_pages() {
    let (api, owner) = logged_in().await;
    for name in ["Two", "Three"] {
        let reply = api
            .send(
                "POST",
                "/workspaces",
                &owner.headers(),
                json!({ "name": name }),
            )
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
    }

    let reply = api.get(&owner, "/workspaces?limit=2").await;
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 2);
    let cursor = reply.body["next_cursor"].as_str().unwrap();
    let reply = api
        .get(&owner, &format!("/workspaces?limit=2&cursor={cursor}"))
        .await;
    assert_eq!(reply.body["items"][0]["name"], "Three");
    assert_eq!(reply.body["next_cursor"], Value::Null);
}

#[tokio::test]
async fn only_an_instance_admin_creates_a_workspace_and_they_own_it() {
    let (api, admin) = logged_in().await;
    let user = api.add_and_login(BOB, "bob@example.org").await;

    let body = json!({ "name": "Team" });
    let reply = api
        .send("POST", "/workspaces", &user.headers(), body.clone())
        .await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    let reply = api
        .send("POST", "/workspaces", &admin.headers(), body)
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["slug"], "team");
    assert_eq!(reply.body["role"], "owner");
    assert_eq!(reply.body["language"], "en");

    let reply = api
        .send(
            "POST",
            "/workspaces",
            &admin.headers(),
            json!({ "name": " " }),
        )
        .await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["name"], "required");
}

#[tokio::test]
async fn two_workspaces_with_one_name_get_different_slugs() {
    let (api, admin) = logged_in().await;
    let long = "a".repeat(100);
    for (name, slug) in [
        ("Acme Docs", "acme-docs-2"),
        ("Acme  docs!", "acme-docs-3"),
        (long.as_str(), &"a".repeat(64)),
        (long.as_str(), &format!("{}-2", "a".repeat(62))),
    ] {
        let reply = api
            .send(
                "POST",
                "/workspaces",
                &admin.headers(),
                json!({ "name": name }),
            )
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        assert_eq!(reply.body["slug"], slug);
    }
}

#[tokio::test]
async fn an_admin_updates_a_workspace_and_a_member_cannot() {
    let (api, owner, id) = workspace().await;
    let member = api.add_member(&id, BOB, "bob@example.org", "member").await;
    let admin = api.add_member(&id, CARA, "cara@example.org", "admin").await;
    let stranger = api.add_and_login(DAN, "dan@example.org").await;
    let path = format!("/workspaces/{id}");

    let body = json!({ "name": "Renamed", "language": "fr" });
    let reply = api
        .send("PATCH", &path, &member.headers(), body.clone())
        .await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api
        .send("PATCH", &path, &stranger.headers(), body.clone())
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));

    let reply = api.send("PATCH", &path, &admin.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["name"], "Renamed");
    assert_eq!(reply.body["language"], "fr");
    assert_eq!(reply.body["slug"], "acme-docs");
    assert_eq!(reply.body["role"], "admin");

    let reply = api
        .send(
            "PATCH",
            &path,
            &owner.headers(),
            json!({ "language": "klingon" }),
        )
        .await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["language"], "invalid");
    let reply = api
        .send("PATCH", &path, &owner.headers(), json!({ "name": "" }))
        .await;
    assert_eq!(reply.body["error"]["fields"]["name"], "required");

    let reply = api.send("PATCH", &path, &owner.headers(), json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["name"], "Renamed");
}

#[tokio::test]
async fn a_change_needs_the_csrf_token() {
    let (api, owner, id) = workspace().await;
    let path = format!("/workspaces/{id}");
    let cookie = [("cookie", owner.cookie.as_str())];
    let reply = api
        .send("PATCH", &path, &cookie, json!({ "name": "X" }))
        .await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
}

#[tokio::test]
async fn only_an_owner_deletes_a_workspace_with_its_name_as_confirmation() {
    let (api, owner, id) = workspace().await;
    let admin = api.add_member(&id, BOB, "bob@example.org", "admin").await;
    let token = api.invite_token(&owner, &id, "eve@example.org").await;
    let path = format!("/workspaces/{id}");
    let confirm = json!({ "name": "Acme Docs" });

    let reply = api
        .send("DELETE", &path, &admin.headers(), confirm.clone())
        .await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    let reply = api
        .send(
            "DELETE",
            &path,
            &owner.headers(),
            json!({ "name": "acme docs" }),
        )
        .await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["name"], "mismatch");

    let reply = api.send("DELETE", &path, &owner.headers(), confirm).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.get(&owner, &path).await;
    assert_eq!(reply.status, 404, "{reply:?}");
    assert_eq!(
        api.number("SELECT count(*) FROM workspace_members").await,
        0
    );
    let reply = api
        .raw("GET", &format!("/invitations/{token}"), &[], b"")
        .await;
    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn members_are_listed_by_pages() {
    let (api, owner, id) = workspace().await;
    let member = api.add_member(&id, BOB, "bob@example.org", "member").await;
    api.add_member(&id, CARA, "cara@example.org", "admin").await;
    api.sql(&format!(
        "UPDATE users SET disabled_at = 5 WHERE id = '{CARA}'"
    ))
    .await;

    let path = format!("/workspaces/{id}/members");
    let reply = api.get(&member, &format!("{path}?limit=2")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items[0]["role"], "owner");
    assert_eq!(items[1]["id"], BOB);
    assert_eq!(items[1]["email"], "bob@example.org");
    assert_eq!(items[1]["name"], "Other");
    assert_eq!(items[1]["role"], "member");
    assert_eq!(items[1]["disabled"], false);

    let cursor = reply.body["next_cursor"].as_str().unwrap();
    let reply = api
        .get(&owner, &format!("{path}?limit=2&cursor={cursor}"))
        .await;
    assert_eq!(reply.body["items"][0]["id"], CARA);
    assert_eq!(reply.body["items"][0]["disabled"], true);
    assert_eq!(reply.body["next_cursor"], Value::Null);
}

#[tokio::test]
async fn each_role_changes_what_the_matrix_allows() {
    let (api, owner, id) = workspace().await;
    let owner_id = api.first_user().await;
    let member = api.add_member(&id, BOB, "bob@example.org", "member").await;
    let admin = api.add_member(&id, CARA, "cara@example.org", "admin").await;
    api.add_member(&id, DAN, "dan@example.org", "member").await;

    // A member changes nobody, not even themself.
    for target in [BOB, DAN] {
        let reply = api.set_role(&member, &id, target, "admin").await;
        assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    }

    // An admin handles members and admins, and nothing about owners.
    let reply = api.set_role(&admin, &id, DAN, "admin").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["id"], DAN);
    assert_eq!(reply.body["role"], "admin");
    let reply = api.set_role(&admin, &id, DAN, "member").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let reply = api.set_role(&admin, &id, DAN, "owner").await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api.set_role(&admin, &id, CARA, "owner").await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api.set_role(&admin, &id, &owner_id, "member").await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api.remove(&admin, &id, &owner_id).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    // An owner does all of it.
    let reply = api.set_role(&owner, &id, DAN, "owner").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let reply = api.set_role(&owner, &id, DAN, "member").await;
    assert_eq!(reply.status, 200, "{reply:?}");

    let reply = api.set_role(&owner, &id, DAN, "king").await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["role"], "invalid");
    let reply = api.set_role(&owner, &id, "nobody", "member").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn a_stranger_finds_no_member_to_change() {
    let (api, _, id) = workspace().await;
    let owner_id = api.first_user().await;
    let stranger = api.add_and_login(BOB, "bob@example.org").await;

    let reply = api.set_role(&stranger, &id, &owner_id, "member").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.remove(&stranger, &id, &owner_id).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    // An instance admin has no implicit access either.
    let reply = api.remove(&stranger, &id, BOB).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn a_member_can_leave_and_an_admin_can_remove() {
    let (api, _, id) = workspace().await;
    let member = api.add_member(&id, BOB, "bob@example.org", "member").await;
    let admin = api.add_member(&id, CARA, "cara@example.org", "admin").await;
    api.add_member(&id, DAN, "dan@example.org", "member").await;

    let reply = api.remove(&member, &id, DAN).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    let reply = api.remove(&member, &id, BOB).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.role_of(&id, BOB).await, None);
    let reply = api.get(&member, &format!("/workspaces/{id}")).await;
    assert_eq!(reply.status, 404, "{reply:?}");

    let reply = api.remove(&admin, &id, DAN).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.remove(&admin, &id, CARA).await;
    assert_eq!(reply.status, 204, "{reply:?}");
}

#[tokio::test]
async fn the_last_owner_stays() {
    let (api, owner, id) = workspace().await;
    let owner_id = api.first_user().await;

    let reply = api.remove(&owner, &id, &owner_id).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));
    let reply = api.set_role(&owner, &id, &owner_id, "admin").await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));
    assert_eq!(api.role_of(&id, &owner_id).await.unwrap(), "owner");

    // Staying an owner is no loss.
    let reply = api.set_role(&owner, &id, &owner_id, "owner").await;
    assert_eq!(reply.status, 200, "{reply:?}");

    let other = api.add_member(&id, BOB, "bob@example.org", "owner").await;
    let reply = api.remove(&other, &id, &owner_id).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.remove(&other, &id, BOB).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));
}

#[tokio::test]
async fn a_disabled_owner_does_not_count_as_one_who_stays() {
    let (api, owner, id) = workspace().await;
    let owner_id = api.first_user().await;
    api.add_member(&id, BOB, "bob@example.org", "owner").await;
    api.sql(&format!(
        "UPDATE users SET disabled_at = 5 WHERE id = '{BOB}'"
    ))
    .await;

    let reply = api.remove(&owner, &id, &owner_id).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));
    let reply = api.set_role(&owner, &id, &owner_id, "member").await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));

    // Removing the disabled one leaves an enabled owner.
    let reply = api.remove(&owner, &id, BOB).await;
    assert_eq!(reply.status, 204, "{reply:?}");
}

#[tokio::test]
async fn two_owners_demoting_each_other_leave_one() {
    let (api, owner, id) = workspace().await;
    let owner_id = api.first_user().await;
    let other = api.add_member(&id, BOB, "bob@example.org", "owner").await;

    let (first, second) = tokio::join!(
        api.set_role(&owner, &id, BOB, "member"),
        api.set_role(&other, &id, &owner_id, "member"),
    );
    let statuses = [first.status, second.status];
    assert!(statuses.contains(&200), "{statuses:?}");
    let owners = format!(
        "SELECT count(*) FROM workspace_members WHERE workspace_id = '{id}' AND role = 'owner'"
    );
    assert_eq!(api.number(&owners).await, 1);
}

#[tokio::test]
async fn only_an_admin_handles_invitations() {
    let (api, owner, id) = workspace().await;
    let member = api.add_member(&id, BOB, "bob@example.org", "member").await;
    let admin = api.add_member(&id, CARA, "cara@example.org", "admin").await;
    let stranger = api.add_and_login(DAN, "dan@example.org").await;
    let path = format!("/workspaces/{id}/invitations");

    let reply = api.invite(&member, &id, "eve@example.org", "member").await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api.get(&member, &path).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = api
        .invite(&stranger, &id, "eve@example.org", "member")
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.get(&stranger, &path).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));

    let reply = api.invite(&admin, &id, "Eve@Example.org", "admin").await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.body["email"], "eve@example.org");
    assert_eq!(reply.body["role"], "admin");
    assert_eq!(reply.body["invited_by"], CARA);
    let link = reply.body["link"].as_str().unwrap();
    let token = link.strip_prefix(&format!("{ORIGIN}/invite/")).unwrap();
    assert_eq!(token.len(), 64);
    let week = 7 * 24 * 60 * 60 * 1000;
    let lifetime =
        reply.body["expires_at"].as_i64().unwrap() - reply.body["created_at"].as_i64().unwrap();
    assert_eq!(lifetime, week);

    // Only its hash is stored, and the list never gives the link back.
    let hash = cadocs_server::session::token_hash(token);
    let stored = format!("SELECT count(*) FROM workspace_invitations WHERE token_hash = '{hash}'");
    assert_eq!(api.number(&stored).await, 1);
    let reply = api.get(&owner, &path).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["email"], "eve@example.org");
    assert!(items[0].get("link").is_none());
    assert!(!reply.body.to_string().contains(token));
}

#[tokio::test]
async fn an_invitation_is_checked() {
    let (api, owner, id) = workspace().await;
    api.add_member(&id, BOB, "bob@example.org", "member").await;

    let reply = api.invite(&owner, &id, "eve@example.org", "owner").await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["role"], "invalid");
    let reply = api.invite(&owner, &id, "nope", "member").await;
    assert_eq!(reply.body["error"]["fields"]["email"], "invalid");

    let reply = api.invite(&owner, &id, "BOB@example.org", "member").await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));
    assert_eq!(reply.body["error"]["fields"]["email"], "member");
}

#[tokio::test]
async fn a_new_invitation_replaces_the_last_one_for_that_email() {
    let (api, owner, id) = workspace().await;
    let old = api.invite_token(&owner, &id, "eve@example.org").await;
    let new = api.invite_token(&owner, &id, "eve@example.org").await;

    let reply = api
        .raw("GET", &format!("/invitations/{old}"), &[], b"")
        .await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = api
        .raw("GET", &format!("/invitations/{new}"), &[], b"")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let count = "SELECT count(*) FROM workspace_invitations";
    assert_eq!(api.number(count).await, 1);
}

#[tokio::test]
async fn anyone_with_the_link_reads_the_invitation_until_it_expires() {
    let (api, owner, id) = workspace().await;
    api.add_and_login(BOB, "bob@example.org").await;
    let token = api.invite_token(&owner, &id, "eve@example.org").await;
    let path = format!("/invitations/{token}");

    let reply = api.raw("GET", &path, &[], b"").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.body["workspace_name"], "Acme Docs");
    assert_eq!(reply.body["email"], "eve@example.org");
    assert_eq!(reply.body["has_account"], false);

    let token = api.invite_token(&owner, &id, "bob@example.org").await;
    let reply = api
        .raw("GET", &format!("/invitations/{token}"), &[], b"")
        .await;
    assert_eq!(reply.body["has_account"], true);

    let reply = api.raw("GET", "/invitations/unknown", &[], b"").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));

    api.sql("UPDATE workspace_invitations SET expires_at = 1")
        .await;
    let reply = api.raw("GET", &path, &[], b"").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = api.accept_as_new(&token).await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = api
        .get(&owner, &format!("/workspaces/{id}/invitations"))
        .await;
    assert_eq!(reply.body["items"], json!([]));
}

#[tokio::test]
async fn a_new_user_joins_with_a_link_and_is_logged_in() {
    let (api, owner, id) = workspace().await;
    let reply = api.invite(&owner, &id, "eve@example.org", "admin").await;
    let link = reply.body["link"].as_str().unwrap();
    let token = link.rsplit('/').next().unwrap();

    let body = json!({ "name": "Eve", "password": "short" });
    let reply = api.accept(token, &[], body).await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["password"], "too_short");
    let reply = api
        .accept(token, &[], json!({ "password": PASSWORD }))
        .await;
    assert_eq!(reply.body["error"]["fields"]["name"], "required");
    let reply = api.accept(token, &[], json!({ "name": "Eve" })).await;
    assert_eq!(reply.body["error"]["fields"]["password"], "required");

    let reply = api.accept_as_new(token).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.body["user"]["email"], "eve@example.org");
    assert_eq!(reply.body["user"]["name"], "Eve");
    assert_eq!(reply.body["user"]["is_instance_admin"], false);
    assert_eq!(reply.body["workspaces"][0]["id"], id.as_str());
    assert_eq!(reply.body["workspaces"][0]["role"], "admin");

    let eve = Login::from(&reply);
    let reply = api.get(&eve, &format!("/workspaces/{id}")).await;
    assert_eq!(reply.body["role"], "admin");
    let reply = api.login("eve@example.org", PASSWORD).await;
    assert_eq!(reply.status, 200, "{reply:?}");

    // The link works once.
    let reply = api.accept_as_new(token).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api
        .get(&owner, &format!("/workspaces/{id}/invitations"))
        .await;
    assert_eq!(reply.body["items"], json!([]));
}

#[tokio::test]
async fn a_link_cannot_make_a_second_account_for_an_email() {
    let (api, owner, id) = workspace().await;
    api.add_and_login(BOB, "bob@example.org").await;
    let token = api.invite_token(&owner, &id, "bob@example.org").await;

    let reply = api.accept_as_new(&token).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));
    assert_eq!(reply.body["error"]["fields"]["email"], "taken");
    assert_eq!(api.role_of(&id, BOB).await, None);
    let reply = api
        .raw("GET", &format!("/invitations/{token}"), &[], b"")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn a_logged_in_user_with_the_invited_email_just_joins() {
    let (api, owner, id) = workspace().await;
    let bob = api.add_and_login(BOB, "bob@example.org").await;
    let reply = api.invite(&owner, &id, "bob@example.org", "admin").await;
    let link = reply.body["link"].as_str().unwrap();
    let token = link.rsplit('/').next().unwrap();

    let reply = api.accept(token, &bob.headers(), json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("cache-control"), Some("no-store"));
    assert_eq!(reply.body["user"]["id"], BOB);
    assert_eq!(reply.body["workspaces"][0]["role"], "admin");
    assert_eq!(reply.body["csrf_token"], bob.csrf.as_str());
    assert_eq!(api.role_of(&id, BOB).await.unwrap(), "admin");

    let reply = api.accept(token, &bob.headers(), json!({})).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn joining_needs_the_invited_account_and_its_csrf_token() {
    let (api, owner, id) = workspace().await;
    let bob = api.add_and_login(BOB, "bob@example.org").await;
    let cara = api.add_and_login(CARA, "cara@example.org").await;
    let token = api.invite_token(&owner, &id, "bob@example.org").await;

    let reply = api.accept(&token, &[], json!({})).await;
    assert_eq!((reply.status, reply.code()), (401, "unauthenticated"));

    let reply = api.accept(&token, &cara.headers(), json!({})).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    assert_eq!(api.role_of(&id, CARA).await, None);

    let cookie = [("cookie", bob.cookie.as_str())];
    let reply = api.accept(&token, &cookie, json!({})).await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
    assert_eq!(api.role_of(&id, BOB).await, None);

    let reply = api.accept(&token, &bob.headers(), json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn accepting_never_changes_the_role_of_a_member() {
    let (api, owner, id) = workspace().await;
    let bob = api.add_and_login(BOB, "bob@example.org").await;
    let token = api.invite_token(&owner, &id, "bob@example.org").await;
    // They got in another way while the link was out.
    api.sql(&format!(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         VALUES ('{id}', '{BOB}', 'admin', 1)"
    ))
    .await;

    let reply = api.accept(&token, &bob.headers(), json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(api.role_of(&id, BOB).await.unwrap(), "admin");
}

#[tokio::test]
async fn two_accepts_at_once_make_one_account() {
    let (api, owner, id) = workspace().await;
    let token = api.invite_token(&owner, &id, "eve@example.org").await;

    let (first, second) = tokio::join!(api.accept_as_new(&token), api.accept_as_new(&token));
    let mut statuses = [first.status, second.status];
    statuses.sort();
    assert_eq!(statuses[0], 201, "{statuses:?}");
    assert!(matches!(statuses[1], 404 | 409), "{statuses:?}");
    let eves = "SELECT count(*) FROM users WHERE email = 'eve@example.org'";
    assert_eq!(api.number(eves).await, 1);
}

#[tokio::test]
async fn an_admin_revokes_an_invitation_others_find_nothing() {
    let (api, owner, id) = workspace().await;
    let member = api.add_member(&id, BOB, "bob@example.org", "member").await;
    let stranger = api.add_and_login(CARA, "cara@example.org").await;
    let reply = api
        .send(
            "POST",
            "/workspaces",
            &owner.headers(),
            json!({ "name": "Other" }),
        )
        .await;
    let other = reply.body["id"].as_str().unwrap().to_owned();
    let other_admin = api
        .add_member(&other, DAN, "dan@example.org", "admin")
        .await;

    let reply = api.invite(&owner, &id, "eve@example.org", "member").await;
    let invitation = reply.body["id"].as_str().unwrap().to_owned();
    let link = reply.body["link"].as_str().unwrap();
    let token = link.rsplit('/').next().unwrap();
    let path = format!("/invitations/{invitation}");

    for by in [&member, &stranger, &other_admin] {
        let reply = api.raw("DELETE", &path, &by.headers(), b"").await;
        assert_eq!((reply.status, reply.code()), (404, "not_found"));
    }

    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api
        .raw("GET", &format!("/invitations/{token}"), &[], b"")
        .await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn accepting_counts_as_a_password_guess() {
    let (api, _, _) = workspace().await;
    let body = json!({ "name": "Eve", "password": PASSWORD });
    let mut last = None;
    for _ in 0..12 {
        last = Some(api.accept("unknown", &[], body.clone()).await);
    }
    let reply = last.unwrap();
    assert_eq!((reply.status, reply.code()), (429, "rate_limited"));
    assert!(reply.header("retry-after").is_some());
}

/// An admin who keeps a link could come back through it with a second email.
#[tokio::test]
async fn the_links_of_an_admin_end_with_their_role() {
    let (api, owner, id) = workspace().await;
    let path = format!("/admin/users/{BOB}");
    let admin = api.add_member(&id, BOB, "bob@example.org", "admin").await;
    let links = "SELECT count(*) FROM workspace_invitations";

    api.invite_token(&admin, &id, "eve@example.org").await;
    let reply = api.set_role(&owner, &id, BOB, "member").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(api.number(links).await, 0);

    api.set_role(&owner, &id, BOB, "admin").await;
    api.invite_token(&admin, &id, "eve@example.org").await;
    let reply = api.remove(&owner, &id, BOB).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.number(links).await, 0);

    api.sql(&format!(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         VALUES ('{id}', '{BOB}', 'admin', 1)"
    ))
    .await;
    api.invite_token(&admin, &id, "eve@example.org").await;
    let body = json!({ "disabled": true });
    let reply = api.send("PATCH", &path, &owner.headers(), body).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(api.number(links).await, 0);

    let body = json!({ "disabled": false });
    api.send("PATCH", &path, &owner.headers(), body).await;
    let reply = api.login("bob@example.org", PASSWORD).await;
    let admin = Login::from(&reply);
    api.invite_token(&admin, &id, "eve@example.org").await;
    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(api.number(links).await, 0);
}

#[tokio::test]
async fn the_links_of_an_admin_stay_while_they_can_still_invite() {
    let (api, owner, id) = workspace().await;
    let other = api.add_member(&id, BOB, "bob@example.org", "owner").await;
    let token = api.invite_token(&other, &id, "eve@example.org").await;
    // Other admins' links stay too.
    let kept = api.invite_token(&owner, &id, "fay@example.org").await;

    let reply = api.set_role(&owner, &id, BOB, "admin").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let reply = api
        .raw("GET", &format!("/invitations/{token}"), &[], b"")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");

    api.remove(&owner, &id, BOB).await;
    let reply = api
        .raw("GET", &format!("/invitations/{kept}"), &[], b"")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn a_bad_path_is_refused_in_the_error_shape() {
    let (api, owner, id) = workspace().await;
    let path = format!("/workspaces/{id}/members/%FF");
    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!((reply.status, reply.code()), (400, "invalid_request"));
}

#[tokio::test]
async fn a_workspace_needs_an_instance_admin_when_it_is_written() {
    let (api, _) = logged_in().await;
    api.add_user(BOB, "bob@example.org").await;
    let new = NewWorkspace {
        id: "new".to_owned(),
        slug: "new".to_owned(),
        name: "New".to_owned(),
        owner_id: BOB.to_owned(),
        now: 1,
    };
    let refused = api.store.create_workspace(&new).await;
    assert!(
        matches!(refused, Err(StoreError::NotAllowed)),
        "{refused:?}"
    );
}
