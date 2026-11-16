//! Groups and their members over REST, and what a grant to a group gives its members. Who may call
//! each route is in `matrix.rs`, what a change does to open sockets in `socket_closes.rs` and
//! `events.rs`.

mod common;

use common::{Api, Login, Reply, logged_in};
use serde_json::{Value, json};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const CARA: &str = "11111111-0000-7000-8000-000000000002";
const EVE: &str = "22222222-0000-7000-8000-000000000001";

/// The owner of the first workspace, and its id.
async fn workspace() -> (Api, Login, String) {
    let (api, owner) = logged_in().await;
    let id = api.first_workspace().await;
    (api, owner, id)
}

impl Api {
    async fn create_group(&self, by: &Login, workspace: &str, body: Value) -> Reply {
        let path = format!("/workspaces/{workspace}/groups");
        self.send("POST", &path, &by.headers(), body).await
    }

    async fn rename_group(&self, by: &Login, group: &str, name: &str) -> Reply {
        let path = format!("/groups/{group}");
        let body = json!({ "name": name });
        self.send("PATCH", &path, &by.headers(), body).await
    }

    async fn items(&self, by: &Login, path: &str) -> Vec<Value> {
        let reply = self.get(by, path).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        reply.body["items"].as_array().unwrap().clone()
    }

    async fn space(&self, by: &Login, workspace: &str, default_role: Value) -> String {
        let path = format!("/workspaces/{workspace}/spaces");
        let body = json!({ "name": "Handbook", "default_role": default_role });
        let reply = self.send("POST", &path, &by.headers(), body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        reply.body["id"].as_str().unwrap().to_owned()
    }

    async fn page(&self, by: &Login, space: &str) -> String {
        let path = format!("/spaces/{space}/pages");
        let reply = self.send("POST", &path, &by.headers(), json!({})).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        reply.body["id"].as_str().unwrap().to_owned()
    }

    async fn grant_group(&self, by: &Login, space: &str, group: &str, role: &str) {
        let path = format!("/spaces/{space}/grants");
        let body = json!({ "grants": [{ "group_id": group, "role": role }] });
        let reply = self.send("PUT", &path, &by.headers(), body).await;
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
}

#[tokio::test]
async fn an_admin_makes_renames_and_deletes_a_group() {
    let (api, owner, ws) = workspace().await;

    let reply = api
        .create_group(&owner, &ws, json!({ "name": "  Writers " }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let group = reply.body["id"].as_str().unwrap().to_owned();
    assert_eq!(reply.body["workspace_id"], ws);
    assert_eq!(reply.body["name"], "Writers");
    assert_eq!(reply.body["member_count"], 0);
    assert!(reply.body["created_at"].as_i64().unwrap() > 0);

    let listed = api.items(&owner, &format!("/workspaces/{ws}/groups")).await;
    assert_eq!(listed, std::slice::from_ref(&reply.body));

    let reply = api.rename_group(&owner, &group, "Editors").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["id"], group);
    assert_eq!(reply.body["name"], "Editors");
    // Keeping its own name is no clash.
    assert_eq!(
        api.rename_group(&owner, &group, "Editors").await.status,
        200
    );

    let path = format!("/groups/{group}");
    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(
        api.items(&owner, &format!("/workspaces/{ws}/groups"))
            .await
            .is_empty()
    );
    let reply = api.rename_group(&owner, &group, "Back").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = api.get(&owner, &format!("{path}/members")).await;
    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn a_group_name_is_checked_and_unique_in_its_workspace() {
    let (api, owner, ws) = workspace().await;
    let writers = api.new_group(&owner, &ws, "Writers").await;
    let readers = api.new_group(&owner, &ws, "Readers").await;

    let reply = api
        .create_group(&owner, &ws, json!({ "name": "Writers" }))
        .await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"));
    assert_eq!(reply.body["error"]["fields"]["name"], "taken");
    let reply = api.rename_group(&owner, &readers, "Writers").await;
    assert_eq!(reply.status, 409, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["name"], "taken");

    for (name, problem) in [("   ", "required"), (&*"x".repeat(101), "too_long")] {
        let reply = api.create_group(&owner, &ws, json!({ "name": name })).await;
        assert_eq!(reply.body["error"]["fields"]["name"], problem, "{reply:?}");
        let reply = api.rename_group(&owner, &writers, name).await;
        assert_eq!(reply.body["error"]["fields"]["name"], problem, "{reply:?}");
    }
    let reply = api.create_group(&owner, &ws, json!({})).await;
    assert_eq!(reply.status, 400, "{reply:?}");

    // Another workspace may use the same name.
    let reply = api
        .send(
            "POST",
            "/workspaces",
            &owner.headers(),
            json!({ "name": "Other" }),
        )
        .await;
    let other = reply.body["id"].as_str().unwrap();
    api.new_group(&owner, other, "Writers").await;
    let names: Vec<Value> = api
        .items(&owner, &format!("/workspaces/{ws}/groups"))
        .await
        .iter()
        .map(|group| group["name"].clone())
        .collect();
    assert_eq!(names, ["Writers", "Readers"]);
}

#[tokio::test]
async fn the_list_of_groups_has_pages() {
    let (api, owner, ws) = workspace().await;
    for name in ["One", "Two", "Three"] {
        api.new_group(&owner, &ws, name).await;
    }
    let reply = api
        .get(&owner, &format!("/workspaces/{ws}/groups?limit=2"))
        .await;
    assert_eq!(reply.body["items"].as_array().unwrap().len(), 2);
    let cursor = reply.body["next_cursor"].as_str().unwrap();
    let path = format!("/workspaces/{ws}/groups?limit=2&cursor={cursor}");
    let reply = api.get(&owner, &path).await;
    assert_eq!(reply.body["items"][0]["name"], "Three");
    assert_eq!(reply.body["next_cursor"], Value::Null);
}

#[tokio::test]
async fn members_of_the_workspace_join_and_leave_a_group() {
    let (api, owner, ws) = workspace().await;
    api.add_member(&ws, BOB, "bob@example.org", "member").await;
    api.add_member(&ws, CARA, "cara@example.org", "member")
        .await;
    api.add_user(EVE, "eve@example.org").await;
    let group = api.new_group(&owner, &ws, "Writers").await;

    api.join_group(&owner, &group, CARA).await;
    api.join_group(&owner, &group, BOB).await;
    // Joining twice changes nothing.
    api.join_group(&owner, &group, BOB).await;

    let members = api.items(&owner, &format!("/groups/{group}/members")).await;
    let ids: Vec<&Value> = members.iter().map(|member| &member["id"]).collect();
    assert_eq!(ids, [BOB, CARA]);
    assert_eq!(members[0]["email"], "bob@example.org");
    assert_eq!(members[0]["role"], "member");
    assert_eq!(members[0]["disabled"], false);
    let reply = api
        .get(&owner, &format!("/groups/{group}/members?limit=1"))
        .await;
    assert_eq!(reply.body["next_cursor"], BOB);
    let groups = api.items(&owner, &format!("/workspaces/{ws}/groups")).await;
    assert_eq!(groups[0]["member_count"], 2);

    // Only members of the workspace join, and only an account that exists.
    for user in [EVE, "nobody"] {
        let reply = api.group_member(&owner, "PUT", &group, user).await;
        assert_eq!((reply.status, reply.code()), (404, "not_found"), "{user}");
    }

    let reply = api.group_member(&owner, "DELETE", &group, BOB).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.group_member(&owner, "DELETE", &group, BOB).await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let members = api.items(&owner, &format!("/groups/{group}/members")).await;
    assert_eq!(members.len(), 1);
    assert_eq!(members[0]["id"], CARA);
}

#[tokio::test]
async fn a_member_who_leaves_the_workspace_leaves_its_groups() {
    let (api, owner, ws) = workspace().await;
    api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let group = api.new_group(&owner, &ws, "Writers").await;
    api.join_group(&owner, &group, BOB).await;

    let path = format!("/workspaces/{ws}/members/{BOB}");
    let reply = api.raw("DELETE", &path, &owner.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert!(
        api.items(&owner, &format!("/groups/{group}/members"))
            .await
            .is_empty()
    );
}

/// Rule 4 of the resolution order, with groups made over REST.
#[tokio::test]
async fn a_grant_to_a_group_follows_its_members() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let space = api.space(&owner, &ws, Value::Null).await;
    let page = api.page(&owner, &space).await;
    let group = api.new_group(&owner, &ws, "Writers").await;
    api.grant_group(&owner, &space, &group, "editor").await;
    assert_eq!(api.role_on(&bob, &space).await, None);

    api.join_group(&owner, &group, BOB).await;
    assert_eq!(api.role_on(&bob, &space).await.as_deref(), Some("editor"));
    let reply = api.get(&bob, &format!("/pages/{page}")).await;
    assert_eq!(reply.body["role"], "editor", "{reply:?}");
    let spaces = api.items(&bob, &format!("/workspaces/{ws}/spaces")).await;
    assert_eq!(spaces.len(), 1);
    let reply = api
        .send(
            "PATCH",
            &format!("/pages/{page}"),
            &bob.headers(),
            json!({ "title": "Mine" }),
        )
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");

    api.group_member(&owner, "DELETE", &group, BOB).await;
    assert_eq!(api.role_on(&bob, &space).await, None);
    assert_eq!(api.get(&bob, &format!("/pages/{page}")).await.status, 404);

    // A deleted group takes its grants with it.
    api.join_group(&owner, &group, BOB).await;
    let path = format!("/groups/{group}");
    assert_eq!(
        api.raw("DELETE", &path, &owner.headers(), b"").await.status,
        204
    );
    assert_eq!(api.role_on(&bob, &space).await, None);
    let grants = api.items(&owner, &format!("/spaces/{space}/grants")).await;
    assert!(grants.is_empty(), "{grants:?}");
}

#[tokio::test]
async fn a_group_of_another_workspace_is_out_of_reach() {
    let (api, owner, ws) = workspace().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "admin").await;
    let reply = api
        .send(
            "POST",
            "/workspaces",
            &owner.headers(),
            json!({ "name": "Other" }),
        )
        .await;
    let other = reply.body["id"].as_str().unwrap().to_owned();
    let theirs = api.new_group(&owner, &other, "Theirs").await;

    // An admin here is a stranger there.
    let paths = [
        ("GET", format!("/groups/{theirs}/members")),
        ("PATCH", format!("/groups/{theirs}")),
        ("DELETE", format!("/groups/{theirs}")),
        ("PUT", format!("/groups/{theirs}/members/{BOB}")),
        ("DELETE", format!("/groups/{theirs}/members/{BOB}")),
    ];
    for (method, path) in paths {
        let reply = api
            .send(method, &path, &bob.headers(), json!({ "name": "Mine" }))
            .await;
        assert_eq!(reply.status, 404, "{method} {path}: {reply:?}");
    }

    // A member of this workspace does not join a group of another.
    let ours = api.new_group(&owner, &ws, "Ours").await;
    let reply = api.group_member(&owner, "PUT", &theirs, BOB).await;
    assert_eq!(reply.status, 404, "{reply:?}");
    api.join_group(&owner, &ours, BOB).await;
}
