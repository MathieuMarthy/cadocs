//! Spaces and their grants, over real sockets. Each rule of the resolution order in
//! `docs/PERMISSIONS.md` has a test here.

mod common;

use cadocs_core::permissions::SpaceRole;
use cadocs_store::{GrantTarget, NewGrant, NewSpace, SpaceStore, StoreError};
use common::{Api, Login, Reply, logged_in};
use serde_json::{Value, json};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const CARA: &str = "11111111-0000-7000-8000-000000000002";
const GROUP: &str = "22222222-0000-7000-8000-000000000001";

impl Api {
    async fn create_space(&self, by: &Login, workspace: &str, body: Value) -> Reply {
        let path = format!("/workspaces/{workspace}/spaces");
        self.send("POST", &path, &by.headers(), body).await
    }

    /// A space with this default role, and its id.
    async fn space_with(&self, by: &Login, workspace: &str, default_role: Value) -> String {
        let body = json!({ "name": "Handbook", "default_role": default_role });
        let reply = self.create_space(by, workspace, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        reply.body["id"].as_str().unwrap().to_owned()
    }

    async fn patch_space(&self, by: &Login, space: &str, body: Value) -> Reply {
        let path = format!("/spaces/{space}");
        self.send("PATCH", &path, &by.headers(), body).await
    }

    async fn trash(&self, by: &Login, space: &str) -> Reply {
        let path = format!("/spaces/{space}");
        self.raw("DELETE", &path, &by.headers(), b"").await
    }

    async fn restore(&self, by: &Login, space: &str) -> Reply {
        let path = format!("/spaces/{space}/restore");
        self.raw("POST", &path, &by.headers(), b"").await
    }

    async fn put_grants(&self, by: &Login, space: &str, grants: Value) -> Reply {
        let path = format!("/spaces/{space}/grants");
        let body = json!({ "grants": grants });
        self.send("PUT", &path, &by.headers(), body).await
    }

    async fn grant(&self, by: &Login, space: &str, user: &str, role: &str) {
        let grants = json!([{ "user_id": user, "role": role }]);
        let reply = self.put_grants(by, space, grants).await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }

    /// The role the API reports, `None` for a space that answers `404`.
    async fn role_on(&self, by: &Login, space: &str) -> Option<String> {
        let reply = self.get(by, &format!("/spaces/{space}")).await;
        match reply.status {
            200 => Some(reply.body["role"].as_str().unwrap().to_owned()),
            404 => None,
            _ => panic!("{reply:?}"),
        }
    }

    async fn listed(&self, by: &Login, path: &str) -> Vec<String> {
        let reply = self.get(by, path).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        let items = reply.body["items"].as_array().unwrap();
        let id = |item: &Value| item["id"].as_str().unwrap().to_owned();
        items.iter().map(id).collect()
    }

    /// A group of the workspace holding `user`, with this grant on the space.
    async fn group_grant(&self, workspace: &str, space: &str, user: &str, role: &str) {
        self.sql(&format!(
            "INSERT INTO groups (id, workspace_id, name, created_at)
             VALUES ('{GROUP}', '{workspace}', 'Team', 1);
             INSERT INTO group_members (group_id, user_id) VALUES ('{GROUP}', '{user}');
             INSERT INTO space_grants (id, space_id, group_id, role, created_at)
             VALUES ('g-{role}', '{space}', '{GROUP}', '{role}', 1)"
        ))
        .await;
    }
}

/// The owner of the first workspace, and its id.
async fn workspace() -> (Api, Login, String) {
    let (api, owner) = logged_in().await;
    let id = api.first_workspace().await;
    (api, owner, id)
}

#[tokio::test]
async fn a_disabled_user_has_no_access() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "admin").await;
    let space = api.space_with(&owner, &ws, json!("editor")).await;
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("admin"));

    api.sql(&format!(
        "UPDATE users SET disabled_at = 1 WHERE id = '{BOB}'"
    ))
    .await;
    let reply = api.get(&bob, &format!("/spaces/{space}")).await;
    assert_eq!((reply.status, reply.code()), (401, "unauthenticated"));
}

#[tokio::test]
async fn a_stranger_finds_nothing_even_with_a_grant() {
    let (api, owner, ws) = workspace().await;
    let stranger = api.add_and_login(BOB, "bob@example.org").await;
    let space = api.space_with(&owner, &ws, json!("editor")).await;
    api.sql(&format!(
        "INSERT INTO space_grants (id, space_id, user_id, role, created_at)
         VALUES ('leftover', '{space}', '{BOB}', 'admin', 1)"
    ))
    .await;

    assert_eq!(api.role_on(&stranger, &space).await, None);
    let reply = api.get(&stranger, &format!("/spaces/{space}/grants")).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api
        .patch_space(&stranger, &space, json!({ "name": "X" }))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api
        .get(&stranger, &format!("/workspaces/{ws}/spaces"))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api
        .create_space(&stranger, &ws, json!({ "name": "X" }))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn workspace_owners_and_admins_are_admins_of_every_space() {
    let (api, owner, ws) = workspace().await;
    let admin = api.add_member(&ws, BOB, "bob@example.org", "admin").await;
    let space = api.space_with(&owner, &ws, Value::Null).await;

    assert_eq!(api.role_on(&owner, &space).await.as_deref(), Some("admin"));
    assert_eq!(api.role_on(&admin, &space).await.as_deref(), Some("admin"));

    // A grant does not lower them.
    api.grant(&owner, &space, BOB, "viewer").await;
    assert_eq!(api.role_on(&admin, &space).await.as_deref(), Some("admin"));
    let reply = api
        .patch_space(&admin, &space, json!({ "name": "Renamed" }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn a_grant_replaces_the_default_role_even_when_lower() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let space = api.space_with(&owner, &ws, json!("editor")).await;
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("editor"));

    api.grant(&owner, &space, BOB, "viewer").await;
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("viewer"));

    api.grant(&owner, &space, BOB, "admin").await;
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("admin"));
}

#[tokio::test]
async fn the_highest_of_direct_and_group_grants_decides() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let cara = api
        .add_member(&ws, CARA, "cara@example.org", "member")
        .await;
    let space = api.space_with(&owner, &ws, json!("commenter")).await;

    api.grant(&owner, &space, BOB, "viewer").await;
    api.group_grant(&ws, &space, BOB, "editor").await;
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("editor"));

    // A group grant alone also replaces the default role, and only for the group.
    api.sql(&format!(
        "UPDATE space_grants SET role = 'viewer' WHERE group_id = '{GROUP}';
         DELETE FROM space_grants WHERE user_id = '{BOB}'"
    ))
    .await;
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("viewer"));
    assert_eq!(
        api.role_on(&cara, &space).await.as_deref(),
        Some("commenter")
    );
}

#[tokio::test]
async fn the_default_role_applies_with_no_grant_and_nothing_else_does() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let open = api.space_with(&owner, &ws, json!("commenter")).await;
    let closed = api.space_with(&owner, &ws, Value::Null).await;
    let invited = api.space_with(&owner, &ws, Value::Null).await;
    api.grant(&owner, &invited, BOB, "viewer").await;

    assert_eq!(api.role_on(&bob, &open).await.as_deref(), Some("commenter"));
    assert_eq!(api.role_on(&bob, &closed).await, None);

    let path = format!("/workspaces/{ws}/spaces");
    assert_eq!(
        api.listed(&bob, &path).await,
        [open.clone(), invited.clone()]
    );
    assert_eq!(api.listed(&owner, &path).await, [open, closed, invited]);
}

#[tokio::test]
async fn a_workspace_admin_has_no_access_to_another_workspace() {
    let (api, owner, ws) = workspace().await;
    let body = json!({ "name": "Other" });
    let reply = api
        .send("POST", "/workspaces", &owner.headers(), body)
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let other = reply.body["id"].as_str().unwrap();
    let space = api.space_with(&owner, other, Value::Null).await;

    let bob = api.add_member(&ws, BOB, "bob@example.org", "admin").await;
    assert_eq!(api.role_on(&bob, &space).await, None);
}

#[tokio::test]
async fn each_role_can_do_what_the_matrix_says() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let space = api.space_with(&owner, &ws, Value::Null).await;

    let reply = api.create_space(&bob, &ws, json!({ "name": "Mine" })).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    for role in ["viewer", "commenter", "editor"] {
        api.grant(&owner, &space, BOB, role).await;
        let reply = api.get(&bob, &format!("/spaces/{space}/grants")).await;
        assert_eq!(reply.status, 200, "{role}: {reply:?}");

        let reply = api.patch_space(&bob, &space, json!({ "name": "X" })).await;
        assert_eq!((reply.status, reply.code()), (403, "forbidden"), "{role}");
        let reply = api.put_grants(&bob, &space, json!([])).await;
        assert_eq!((reply.status, reply.code()), (403, "forbidden"), "{role}");
        let reply = api.trash(&bob, &space).await;
        assert_eq!((reply.status, reply.code()), (403, "forbidden"), "{role}");
    }

    api.grant(&owner, &space, BOB, "admin").await;
    let reply = api
        .patch_space(&bob, &space, json!({ "name": "Mine" }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    api.grant(&bob, &space, BOB, "admin").await;
    let reply = api.trash(&bob, &space).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.restore(&bob, &space).await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn a_new_space_has_a_slug_of_its_own() {
    let (api, owner, ws) = workspace().await;
    let body =
        json!({ "name": "  Team Notes ", "description": " All of it ", "default_role": "viewer" });
    let reply = api.create_space(&owner, &ws, body.clone()).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["workspace_id"], ws.as_str());
    assert_eq!(reply.body["slug"], "team-notes");
    assert_eq!(reply.body["name"], "Team Notes");
    assert_eq!(reply.body["description"], "All of it");
    assert_eq!(reply.body["default_role"], "viewer");
    assert_eq!(reply.body["role"], "admin");
    assert_eq!(reply.body["deleted_at"], Value::Null);

    let reply = api.create_space(&owner, &ws, body).await;
    assert_eq!(reply.body["slug"], "team-notes-2");

    let body = json!({ "name": "日本語" });
    let reply = api.create_space(&owner, &ws, body).await;
    assert_eq!(reply.body["slug"], "space");
    assert_eq!(reply.body["default_role"], Value::Null);
    assert_eq!(reply.body["description"], "");
}

#[tokio::test]
async fn a_space_refuses_bad_fields() {
    let (api, owner, ws) = workspace().await;
    let body = json!({ "name": " ", "description": "x".repeat(1001), "default_role": "admin" });
    let reply = api.create_space(&owner, &ws, body).await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    let fields = &reply.body["error"]["fields"];
    assert_eq!(fields["name"], "required");
    assert_eq!(fields["description"], "too_long");
    assert_eq!(fields["default_role"], "invalid");

    let space = api.space_with(&owner, &ws, Value::Null).await;
    let reply = api
        .patch_space(&owner, &space, json!({ "default_role": "admin" }))
        .await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
}

#[tokio::test]
async fn an_update_keeps_what_it_does_not_name() {
    let (api, owner, ws) = workspace().await;
    let space = api.space_with(&owner, &ws, json!("editor")).await;

    let reply = api
        .patch_space(&owner, &space, json!({ "name": "Renamed" }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["name"], "Renamed");
    assert_eq!(reply.body["slug"], "handbook");
    assert_eq!(reply.body["default_role"], "editor");

    let body = json!({ "description": "New", "default_role": Value::Null });
    let reply = api.patch_space(&owner, &space, body).await;
    assert_eq!(reply.body["name"], "Renamed");
    assert_eq!(reply.body["description"], "New");
    assert_eq!(reply.body["default_role"], Value::Null);
}

#[tokio::test]
async fn a_change_needs_the_csrf_token() {
    let (api, owner, ws) = workspace().await;
    let space = api.space_with(&owner, &ws, Value::Null).await;
    let cookie = [("cookie", owner.cookie.as_str())];
    let reply = api
        .send(
            "PATCH",
            &format!("/spaces/{space}"),
            &cookie,
            json!({ "name": "X" }),
        )
        .await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
}

#[tokio::test]
async fn a_trashed_space_is_absent_until_an_admin_restores_it() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let cara = api
        .add_member(&ws, CARA, "cara@example.org", "member")
        .await;
    let space = api.space_with(&owner, &ws, json!("editor")).await;
    api.grant(&owner, &space, BOB, "admin").await;
    let list = format!("/workspaces/{ws}/spaces");
    let trash = format!("/workspaces/{ws}/spaces/trash");
    assert_eq!(api.listed(&owner, &trash).await, Vec::<String>::new());

    assert_eq!(api.trash(&bob, &space).await.status, 204);

    // Everything answers as if it was gone, for its admins too.
    assert_eq!(api.role_on(&owner, &space).await, None);
    assert_eq!(api.listed(&owner, &list).await, Vec::<String>::new());
    let reply = api.get(&owner, &format!("/spaces/{space}/grants")).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api
        .patch_space(&owner, &space, json!({ "name": "X" }))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.put_grants(&owner, &space, json!([])).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.trash(&owner, &space).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));

    // Only those who can restore it see it, and an editor cannot.
    assert_eq!(
        api.listed(&owner, &trash).await,
        std::slice::from_ref(&space)
    );
    assert_eq!(api.listed(&bob, &trash).await, std::slice::from_ref(&space));
    assert_eq!(api.listed(&cara, &trash).await, Vec::<String>::new());
    let reply = api.restore(&cara, &space).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));

    let reply = api.restore(&bob, &space).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["deleted_at"], Value::Null);
    assert_eq!(reply.body["role"], "admin");
    assert_eq!(
        api.listed(&owner, &list).await,
        std::slice::from_ref(&space)
    );
    assert_eq!(api.listed(&owner, &trash).await, Vec::<String>::new());
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("admin"));

    let reply = api.restore(&owner, &space).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
}

#[tokio::test]
async fn the_trash_of_a_workspace_is_hidden_from_strangers() {
    let (api, _, ws) = workspace().await;
    let stranger = api.add_and_login(BOB, "bob@example.org").await;
    let reply = api
        .get(&stranger, &format!("/workspaces/{ws}/spaces/trash"))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn a_put_replaces_every_grant() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    api.add_member(&ws, CARA, "cara@example.org", "member")
        .await;
    let space = api.space_with(&owner, &ws, Value::Null).await;
    api.sql(&format!(
        "INSERT INTO groups (id, workspace_id, name, created_at)
         VALUES ('{GROUP}', '{ws}', 'Team', 1)"
    ))
    .await;

    let grants = json!([
        { "user_id": BOB, "role": "editor" },
        { "group_id": GROUP, "role": "viewer" },
    ]);
    let reply = api.put_grants(&owner, &space, grants).await;
    assert_eq!(reply.status, 204, "{reply:?}");

    let path = format!("/spaces/{space}/grants");
    let reply = api.get(&bob, &path).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["user_id"], BOB);
    assert_eq!(items[0]["group_id"], Value::Null);
    assert_eq!(items[0]["role"], "editor");
    assert_eq!(items[1]["group_id"], GROUP);
    assert_eq!(items[1]["user_id"], Value::Null);
    assert_eq!(items[1]["role"], "viewer");

    let grants = json!([{ "user_id": CARA, "role": "admin" }]);
    assert_eq!(api.put_grants(&owner, &space, grants).await.status, 204);
    let reply = api.get(&owner, &path).await;
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["user_id"], CARA);
    assert_eq!(api.role_on(&bob, &space).await, None);

    assert_eq!(api.put_grants(&owner, &space, json!([])).await.status, 204);
    assert_eq!(api.number("SELECT count(*) FROM space_grants").await, 0);
}

#[tokio::test]
async fn the_lists_have_pages() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    api.add_member(&ws, CARA, "cara@example.org", "member")
        .await;
    let first = api.space_with(&owner, &ws, json!("viewer")).await;
    let second = api.space_with(&owner, &ws, Value::Null).await;
    let third = api.space_with(&owner, &ws, json!("viewer")).await;

    // The page is cut after the filter: an unreadable space takes no place in it.
    let path = format!("/workspaces/{ws}/spaces?limit=1");
    let reply = api.get(&bob, &path).await;
    assert_eq!(reply.body["items"][0]["id"], first.as_str());
    let cursor = reply.body["next_cursor"].as_str().unwrap();
    let reply = api.get(&bob, &format!("{path}&cursor={cursor}")).await;
    assert_eq!(reply.body["items"][0]["id"], third.as_str());
    assert_eq!(reply.body["next_cursor"], Value::Null);

    let grants = json!([
        { "user_id": BOB, "role": "viewer" },
        { "user_id": CARA, "role": "viewer" },
    ]);
    assert_eq!(api.put_grants(&owner, &second, grants).await.status, 204);
    let path = format!("/spaces/{second}/grants?limit=1");
    let reply = api.get(&owner, &path).await;
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 1);
    let cursor = reply.body["next_cursor"].as_str().unwrap();
    let reply = api.get(&owner, &format!("{path}&cursor={cursor}")).await;
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 1);
    assert_eq!(reply.body["next_cursor"], Value::Null);
}

#[tokio::test]
async fn a_grant_needs_one_target_of_this_workspace() {
    let (api, owner, ws) = workspace().await;
    api.add_member(&ws, BOB, "bob@example.org", "member").await;
    api.add_and_login(CARA, "cara@example.org").await;
    let space = api.space_with(&owner, &ws, Value::Null).await;
    let body = json!({ "name": "Other" });
    let reply = api
        .send("POST", "/workspaces", &owner.headers(), body)
        .await;
    let other = reply.body["id"].as_str().unwrap();
    api.sql(&format!(
        "INSERT INTO groups (id, workspace_id, name, created_at)
         VALUES ('{GROUP}', '{other}', 'Elsewhere', 1)"
    ))
    .await;
    api.grant(&owner, &space, BOB, "editor").await;

    let refused = [
        (
            json!([{ "user_id": CARA, "role": "viewer" }]),
            "grants",
            "unknown_target",
        ),
        (
            json!([{ "group_id": GROUP, "role": "viewer" }]),
            "grants",
            "unknown_target",
        ),
        (json!([{ "role": "viewer" }]), "grants", "invalid"),
        (
            json!([{ "user_id": BOB, "group_id": GROUP, "role": "viewer" }]),
            "grants",
            "invalid",
        ),
        (
            json!([{ "user_id": BOB, "role": "owner" }]),
            "role",
            "invalid",
        ),
        (
            json!([{ "user_id": BOB, "role": "viewer" }, { "user_id": BOB, "role": "editor" }]),
            "grants",
            "duplicate",
        ),
    ];
    for (grants, field, problem) in refused {
        let reply = api.put_grants(&owner, &space, grants.clone()).await;
        assert_eq!(
            (reply.status, reply.code()),
            (400, "validation_failed"),
            "{grants}"
        );
        assert_eq!(reply.body["error"]["fields"][field], problem, "{grants}");
    }

    let many: Vec<Value> = (0..501)
        .map(|n| json!({ "user_id": format!("u{n}"), "role": "viewer" }))
        .collect();
    let reply = api.put_grants(&owner, &space, json!(many)).await;
    assert_eq!(reply.body["error"]["fields"]["grants"], "too_many");

    // A refused change leaves the grants as they were.
    let reply = api.get(&owner, &format!("/spaces/{space}/grants")).await;
    assert_eq!(reply.body["items"][0]["role"], "editor");
}

#[tokio::test]
async fn a_member_who_leaves_loses_their_grants_for_good() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let space = api.space_with(&owner, &ws, Value::Null).await;
    api.grant(&owner, &space, BOB, "admin").await;
    api.group_grant(&ws, &space, BOB, "editor").await;

    let path = format!("/workspaces/{ws}/members/{BOB}");
    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");

    api.sql(&format!(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         VALUES ('{ws}', '{BOB}', 'member', 1)"
    ))
    .await;
    assert_eq!(api.role_on(&bob, &space).await, None);
}

#[tokio::test]
async fn a_put_keeps_the_identity_of_the_grants_that_stay() {
    let (api, owner, ws) = workspace().await;
    api.add_member(&ws, BOB, "bob@example.org", "member").await;
    api.add_member(&ws, CARA, "cara@example.org", "member")
        .await;
    let space = api.space_with(&owner, &ws, Value::Null).await;
    api.grant(&owner, &space, BOB, "viewer").await;
    api.sql("UPDATE space_grants SET created_at = 7").await;
    let path = format!("/spaces/{space}/grants");
    let before = api.get(&owner, &path).await.body["items"][0].clone();

    let grants = json!([
        { "user_id": CARA, "role": "viewer" },
        { "user_id": BOB, "role": "editor" },
    ]);
    assert_eq!(api.put_grants(&owner, &space, grants).await.status, 204);

    let reply = api.get(&owner, &path).await;
    let items = reply.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], before["id"]);
    assert_eq!(items[0]["created_at"], 7);
    assert_eq!(items[0]["role"], "editor");
    assert_eq!(items[1]["user_id"], CARA);
}

#[tokio::test]
async fn a_null_description_is_no_description() {
    let (api, owner, ws) = workspace().await;
    let body = json!({ "name": "Notes", "description": Value::Null });
    let reply = api.create_space(&owner, &ws, body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["description"], "");
}

#[tokio::test]
async fn the_store_refuses_a_user_disabled_in_the_meantime() {
    let (api, _, ws) = workspace().await;
    let owner: String = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&api.db)
        .await
        .unwrap();
    api.sql("UPDATE users SET disabled_at = 1").await;

    let new = NewSpace {
        id: "new".to_owned(),
        workspace_id: ws.clone(),
        slug: "new".to_owned(),
        name: "New".to_owned(),
        description: String::new(),
        default_role: None,
        actor: owner.clone(),
        now: 1,
    };
    let refused = api.store.create_space(&new).await;
    assert!(
        matches!(refused, Err(StoreError::WorkspaceNotFound)),
        "{refused:?}"
    );
    let refused = api.store.list_spaces(&ws, &owner, None, 10).await;
    assert!(
        matches!(refused, Err(StoreError::WorkspaceNotFound)),
        "{refused:?}"
    );
}

#[tokio::test]
async fn the_store_refuses_the_same_target_twice() {
    let (api, owner, ws) = workspace().await;
    let space = api.space_with(&owner, &ws, Value::Null).await;
    let user: String = sqlx::query_scalar("SELECT id FROM users LIMIT 1")
        .fetch_one(&api.db)
        .await
        .unwrap();
    let grant = |id: &str| NewGrant {
        id: id.to_owned(),
        target: GrantTarget::User(user.clone()),
        role: SpaceRole::Viewer,
    };
    let refused = api
        .store
        .set_grants(&space, &user, &[grant("a"), grant("b")], 1)
        .await;
    assert!(
        matches!(refused, Err(StoreError::DuplicateGrant)),
        "{refused:?}"
    );
}
