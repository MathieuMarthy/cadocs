//! Pages, over real sockets: the tree, and the move and trash rules of `docs/DATA_MODEL.md`.

mod common;

use common::{Api, Login, Reply, logged_in};
use serde_json::{Value, json};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const OTHER: &str = "33333333-0000-7000-8000-000000000001";

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

fn ids(reply: &Reply) -> Vec<String> {
    assert_eq!(reply.status, 200, "{reply:?}");
    let items = reply.body["items"].as_array().unwrap();
    items
        .iter()
        .map(|item| item["id"].as_str().unwrap().to_owned())
        .collect()
}

impl Api {
    async fn make_space(&self, by: &Login, ws: &str, default_role: Value) -> String {
        let body = json!({ "name": "Handbook", "default_role": default_role });
        let path = format!("/workspaces/{ws}/spaces");
        let reply = self.send("POST", &path, &by.headers(), body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn create(&self, by: &Login, space: &str, body: Value) -> Reply {
        let path = format!("/spaces/{space}/pages");
        self.send("POST", &path, &by.headers(), body).await
    }

    /// A page at the end of the root of the space, or of `parent`.
    async fn page(&self, by: &Login, space: &str, parent: Option<&str>) -> String {
        let reply = self.create(by, space, json!({ "parent_id": parent })).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn read(&self, by: &Login, page: &str) -> Reply {
        self.get(by, &format!("/pages/{page}")).await
    }

    async fn tree(&self, by: &Login, space: &str, parent: Option<&str>) -> Vec<String> {
        let query = parent.map_or(String::new(), |parent| format!("?parent={parent}"));
        ids(&self.get(by, &format!("/spaces/{space}/tree{query}")).await)
    }

    async fn trash_list(&self, by: &Login, space: &str) -> Vec<String> {
        ids(&self.get(by, &format!("/spaces/{space}/trash")).await)
    }

    async fn move_to(&self, by: &Login, page: &str, body: Value) -> Reply {
        let path = format!("/pages/{page}/move");
        self.send("POST", &path, &by.headers(), body).await
    }

    async fn trash(&self, by: &Login, page: &str) -> Reply {
        self.raw("DELETE", &format!("/pages/{page}"), &by.headers(), b"")
            .await
    }

    async fn restore(&self, by: &Login, page: &str) -> Reply {
        let path = format!("/pages/{page}/restore");
        self.raw("POST", &path, &by.headers(), b"").await
    }

    async fn purge(&self, by: &Login, page: &str) -> Reply {
        let path = format!("/pages/{page}/purge");
        self.raw("DELETE", &path, &by.headers(), b"").await
    }

    async fn trash_ok(&self, by: &Login, page: &str) {
        let reply = self.trash(by, page).await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }

    async fn grant(&self, by: &Login, space: &str, user: &str, role: &str) {
        let body = json!({ "grants": [{ "user_id": user, "role": role }] });
        let path = format!("/spaces/{space}/grants");
        let reply = self.send("PUT", &path, &by.headers(), body).await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }

    async fn deleted_at(&self, page: &str) -> Option<i64> {
        sqlx::query_scalar("SELECT deleted_at FROM pages WHERE id = ?1")
            .bind(page)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn row_count(&self, page: &str) -> i64 {
        self.number(&format!("SELECT count(*) FROM pages WHERE id = '{page}'"))
            .await
    }
}

fn field<'a>(reply: &'a Reply, name: &str) -> &'a str {
    assert_eq!(
        (reply.status, reply.code()),
        (400, "validation_failed"),
        "{reply:?}"
    );
    reply.body["error"]["fields"][name]
        .as_str()
        .unwrap_or_default()
}

/// The owner of the first workspace, its id, and a space with no default role.
async fn space() -> (Api, Login, String, String) {
    let (api, owner) = logged_in().await;
    let ws = api.first_workspace().await;
    let space = api.make_space(&owner, &ws, Value::Null).await;
    (api, owner, ws, space)
}

#[tokio::test]
async fn the_routes_need_a_session_and_the_csrf_token() {
    let (api, owner, _, space) = space().await;
    let page = api.page(&owner, &space, None).await;

    let reply = api.raw("GET", &format!("/pages/{page}"), &[], b"").await;
    assert_eq!((reply.status, reply.code()), (401, "unauthenticated"));
    let cookie = [("cookie", owner.cookie.as_str())];
    let reply = api
        .raw("DELETE", &format!("/pages/{page}"), &cookie, b"")
        .await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
}

#[tokio::test]
async fn a_new_page_goes_where_it_is_asked() {
    let (api, owner, _, space) = space().await;
    let a = api.page(&owner, &space, None).await;
    let c = api.page(&owner, &space, None).await;

    let reply = api.create(&owner, &space, json!({ "after": a })).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let b = id(&reply);
    let first = id(&api.create(&owner, &space, json!({ "after": null })).await);

    assert_eq!(api.tree(&owner, &space, None).await, [first, a, b, c]);
}

#[tokio::test]
async fn a_page_answers_its_metadata_and_the_role_of_the_caller() {
    let (api, owner, ws, space) = space().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    api.grant(&owner, &space, BOB, "viewer").await;
    let parent = api.page(&owner, &space, None).await;

    let reply = api
        .create(&owner, &space, json!({ "parent_id": parent }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let body = &reply.body;
    assert_eq!(body["workspace_id"], ws.as_str());
    assert_eq!(body["space_id"], space.as_str());
    assert_eq!(body["parent_id"], parent.as_str());
    assert_eq!(body["title"], "");
    assert_eq!(body["role"], "admin");
    assert_eq!(body["has_children"], false);
    assert_eq!(body["deleted_at"], Value::Null);
    assert!(body["position"].is_string());
    let page = id(&reply);

    let slug = body["slug"].as_str().unwrap();
    assert!(
        slug.len() >= 10 && slug.bytes().all(|b| b.is_ascii_alphanumeric()),
        "{slug}"
    );

    let read = api.read(&bob, &page).await;
    assert_eq!((read.status, &read.body["role"]), (200, &json!("viewer")));
    let by_slug = api
        .get(&bob, &format!("/workspaces/{ws}/pages/by-slug/{slug}"))
        .await;
    assert_eq!(by_slug.body["id"], page.as_str());
    assert_eq!(api.read(&bob, &parent).await.body["has_children"], true);
}

#[tokio::test]
async fn a_page_names_its_ancestors_root_first() {
    let (api, owner, _, space) = space().await;
    let top = api.page(&owner, &space, None).await;
    let middle = api.page(&owner, &space, Some(&top)).await;
    let page = api.page(&owner, &space, Some(&middle)).await;
    sqlx::query("UPDATE pages SET title = 'Top' WHERE id = ?1")
        .bind(&top)
        .execute(&api.db)
        .await
        .unwrap();

    let read = api.read(&owner, &page).await;
    let expected = json!([{ "id": top, "title": "Top" }, { "id": middle, "title": "" }]);
    assert_eq!((read.status, &read.body["ancestors"]), (200, &expected));
    assert_eq!(api.read(&owner, &top).await.body["ancestors"], json!([]));
    // Only the single read carries them, not the lists.
    let tree = api.get(&owner, &format!("/spaces/{space}/tree")).await;
    assert_eq!(tree.body["items"][0].get("ancestors"), None);
}

#[tokio::test]
async fn a_slug_only_works_in_its_workspace() {
    let (api, owner, ws, space) = space().await;
    let reply = api.create(&owner, &space, json!({})).await;
    let slug = reply.body["slug"].as_str().unwrap();
    api.sql(&format!(
        "INSERT INTO workspaces (id, slug, name, language, created_at, updated_at)
         VALUES ('{OTHER}', 'other', 'Other', 'en', 1, 1);
         INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         SELECT '{OTHER}', user_id, 'owner', 1 FROM workspace_members WHERE workspace_id = '{ws}'"
    ))
    .await;
    let path = format!("/workspaces/{OTHER}/pages/by-slug/{slug}");
    assert_eq!(api.get(&owner, &path).await.status, 404);
}

#[tokio::test]
async fn the_roles_of_the_matrix_decide_who_writes_and_who_sees() {
    let (api, owner, ws, space) = space().await;
    let page = api.page(&owner, &space, None).await;

    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let hidden = |reply: Reply| assert_eq!((reply.status, reply.code()), (404, "not_found"));
    hidden(api.read(&bob, &page).await);
    hidden(api.create(&bob, &space, json!({})).await);
    hidden(api.trash(&bob, &page).await);
    hidden(api.get(&bob, &format!("/spaces/{space}/tree")).await);

    let denied = |reply: Reply| assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    for role in ["viewer", "commenter"] {
        api.grant(&owner, &space, BOB, role).await;
        assert_eq!(api.read(&bob, &page).await.status, 200);
        assert_eq!(
            api.tree(&bob, &space, None).await,
            std::slice::from_ref(&page)
        );
        denied(api.create(&bob, &space, json!({})).await);
        denied(api.move_to(&bob, &page, json!({})).await);
        denied(api.trash(&bob, &page).await);
        denied(api.get(&bob, &format!("/spaces/{space}/trash")).await);
    }

    api.grant(&owner, &space, BOB, "editor").await;
    let own = api.page(&bob, &space, None).await;
    assert_eq!(
        api.move_to(&bob, &own, json!({ "after": null }))
            .await
            .status,
        200
    );
    assert_eq!(api.trash(&bob, &own).await.status, 204);
    assert_eq!(api.restore(&bob, &own).await.status, 200);
}

#[tokio::test]
async fn the_tree_is_one_level_at_a_time_and_paginated() {
    let (api, owner, _, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let other = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;
    let grandchild = api.page(&owner, &space, Some(&child)).await;

    assert_eq!(
        api.tree(&owner, &space, None).await,
        [root.clone(), other.clone()]
    );
    assert_eq!(
        api.tree(&owner, &space, Some(&root)).await,
        std::slice::from_ref(&child)
    );
    assert_eq!(api.tree(&owner, &space, Some(&child)).await, [grandchild]);

    let first = api
        .get(&owner, &format!("/spaces/{space}/tree?limit=1"))
        .await;
    assert_eq!(ids(&first), std::slice::from_ref(&root));
    assert_eq!(first.body["items"][0]["has_children"], true);
    let cursor = first.body["next_cursor"].as_str().unwrap();
    let path = format!("/spaces/{space}/tree?limit=1&cursor={cursor}");
    let second = api.get(&owner, &path).await;
    assert_eq!(ids(&second), [other]);
    assert_eq!(second.body["next_cursor"], Value::Null);
}

#[tokio::test]
async fn a_parent_must_be_a_live_page_of_the_space() {
    let (api, owner, ws, space) = space().await;
    let elsewhere = api.make_space(&owner, &ws, Value::Null).await;
    let foreign = api.page(&owner, &elsewhere, None).await;
    let trashed = api.page(&owner, &space, None).await;
    api.trash_ok(&owner, &trashed).await;

    for parent in [foreign.as_str(), trashed.as_str(), "nope"] {
        let reply = api
            .create(&owner, &space, json!({ "parent_id": parent }))
            .await;
        assert_eq!(field(&reply, "parent_id"), "unknown");
    }
    let path = format!("/spaces/{space}/tree?parent={foreign}");
    assert_eq!(api.get(&owner, &path).await.status, 404);
    let path = format!("/spaces/{space}/tree?parent={trashed}");
    assert_eq!(api.get(&owner, &path).await.status, 404);
}

#[tokio::test]
async fn after_must_be_a_live_sibling() {
    let (api, owner, _, space) = space().await;
    let parent = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&parent)).await;
    let trashed = api.page(&owner, &space, None).await;
    api.trash_ok(&owner, &trashed).await;

    for after in [child.as_str(), trashed.as_str(), "nope"] {
        let reply = api.create(&owner, &space, json!({ "after": after })).await;
        assert_eq!(field(&reply, "after"), "unknown");
    }
    let reply = api
        .move_to(&owner, &parent, json!({ "after": parent }))
        .await;
    assert_eq!(field(&reply, "after"), "unknown");
}

#[tokio::test]
async fn a_move_in_a_space_changes_one_row() {
    let (api, owner, _, space) = space().await;
    let a = api.page(&owner, &space, None).await;
    let b = api.page(&owner, &space, None).await;
    let c = api.page(&owner, &space, None).await;
    let before_a = api.read(&owner, &a).await.body["position"].clone();
    let before_c = api.read(&owner, &c).await.body["position"].clone();

    let reply = api.move_to(&owner, &c, json!({ "after": a })).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        api.tree(&owner, &space, None).await,
        [a.clone(), c.clone(), b.clone()]
    );
    assert_eq!(api.read(&owner, &a).await.body["position"], before_a);
    assert_ne!(reply.body["position"], before_c);

    let reply = api
        .move_to(&owner, &c, json!({ "parent_id": b, "after": null }))
        .await;
    assert_eq!(reply.body["parent_id"], b.as_str());
    assert_eq!(
        api.tree(&owner, &space, Some(&b)).await,
        std::slice::from_ref(&c)
    );
    assert_eq!(api.tree(&owner, &space, None).await, [a.clone(), b.clone()]);

    let reply = api.move_to(&owner, &c, json!({})).await;
    assert_eq!(reply.body["parent_id"], Value::Null);
    assert_eq!(api.tree(&owner, &space, None).await, [a, b, c]);
}

#[tokio::test]
async fn a_page_cannot_go_under_itself_or_a_descendant() {
    let (api, owner, _, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;
    let grandchild = api.page(&owner, &space, Some(&child)).await;

    for parent in [&root, &child, &grandchild] {
        let reply = api
            .move_to(&owner, &root, json!({ "parent_id": parent }))
            .await;
        assert_eq!(field(&reply, "parent_id"), "cycle");
    }
    assert_eq!(api.tree(&owner, &space, None).await, [root]);
}

#[tokio::test]
async fn a_move_to_another_space_takes_all_descendants() {
    let (api, owner, ws, source) = space().await;
    let target = api.make_space(&owner, &ws, Value::Null).await;
    let root = api.page(&owner, &source, None).await;
    let child = api.page(&owner, &source, Some(&root)).await;
    let grandchild = api.page(&owner, &source, Some(&child)).await;
    let trashed = api.page(&owner, &source, Some(&root)).await;
    api.trash_ok(&owner, &trashed).await;

    let reply = api
        .move_to(&owner, &root, json!({ "space_id": target }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["space_id"], target.as_str());
    for page in [&root, &child, &grandchild, &trashed] {
        let count = api
            .number(&format!(
                "SELECT count(*) FROM pages WHERE id = '{page}' AND space_id = '{target}'"
            ))
            .await;
        assert_eq!(count, 1, "{page}");
    }
    assert!(api.tree(&owner, &source, None).await.is_empty());
    assert_eq!(
        api.tree(&owner, &target, None).await,
        std::slice::from_ref(&root)
    );
    assert_eq!(
        api.trash_list(&owner, &target).await,
        std::slice::from_ref(&trashed)
    );

    let reply = api.restore(&owner, &trashed).await;
    assert_eq!(reply.body["space_id"], target.as_str());
    assert_eq!(reply.body["parent_id"], root.as_str());
}

#[tokio::test]
async fn a_move_to_another_space_lands_under_a_parent_there() {
    let (api, owner, ws, source) = space().await;
    let target = api.make_space(&owner, &ws, Value::Null).await;
    let page = api.page(&owner, &source, None).await;
    let parent = api.page(&owner, &target, None).await;
    let here = api.page(&owner, &source, None).await;

    let body = json!({ "space_id": target, "parent_id": here });
    assert_eq!(
        field(&api.move_to(&owner, &page, body).await, "parent_id"),
        "unknown"
    );

    let body = json!({ "space_id": target, "parent_id": parent });
    assert_eq!(api.move_to(&owner, &page, body).await.status, 200);
    assert_eq!(api.tree(&owner, &target, Some(&parent)).await, [page]);
}

#[tokio::test]
async fn a_move_across_spaces_needs_editor_on_both() {
    let (api, owner, ws, source) = space().await;
    let target = api.make_space(&owner, &ws, Value::Null).await;
    let page = api.page(&owner, &source, None).await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let body = json!({ "space_id": target });

    api.grant(&owner, &source, BOB, "editor").await;
    assert_eq!(api.move_to(&bob, &page, body.clone()).await.status, 404);
    api.grant(&owner, &target, BOB, "viewer").await;
    assert_eq!(api.move_to(&bob, &page, body.clone()).await.status, 403);
    api.grant(&owner, &source, BOB, "viewer").await;
    api.grant(&owner, &target, BOB, "editor").await;
    assert_eq!(api.move_to(&bob, &page, body.clone()).await.status, 403);
    api.grant(&owner, &source, BOB, "editor").await;
    assert_eq!(api.move_to(&bob, &page, body).await.status, 200);
}

#[tokio::test]
async fn a_move_across_workspaces_is_refused() {
    let (api, owner, ws, source) = space().await;
    let page = api.page(&owner, &source, None).await;
    api.sql(&format!(
        "INSERT INTO workspaces (id, slug, name, language, created_at, updated_at)
         VALUES ('{OTHER}', 'other', 'Other', 'en', 1, 1);
         INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         SELECT '{OTHER}', user_id, 'owner', 1 FROM workspace_members WHERE workspace_id = '{ws}'"
    ))
    .await;
    let target = api.make_space(&owner, OTHER, Value::Null).await;

    let reply = api
        .move_to(&owner, &page, json!({ "space_id": target }))
        .await;
    assert_eq!(field(&reply, "space_id"), "other_workspace");
    assert_eq!(api.tree(&owner, &source, None).await, [page]);
}

#[tokio::test]
async fn the_depth_of_the_tree_has_a_limit() {
    let (api, owner, _, space) = space().await;
    // Deleting a chain of pages runs nested triggers, and SQLite stops at 1000.
    let root = api.page(&owner, &space, None).await;
    let mut last = root.clone();
    for level in 2..=100 {
        api.sql(&format!(
            "INSERT INTO pages (id, workspace_id, space_id, parent_id, position, slug,
                                created_at, updated_at)
             SELECT 'level-{level}', workspace_id, space_id, '{last}', 'V', 'level-{level}', 1, 1
             FROM pages WHERE id = '{root}'"
        ))
        .await;
        last = format!("level-{level}");
    }

    let reply = api
        .create(&owner, &space, json!({ "parent_id": last }))
        .await;
    assert_eq!(field(&reply, "parent_id"), "too_deep");

    let leaf = api.page(&owner, &space, Some("level-99")).await;
    let branch = api.page(&owner, &space, None).await;
    let tip = api.page(&owner, &space, Some(&branch)).await;
    let reply = api
        .move_to(&owner, &branch, json!({ "parent_id": "level-99" }))
        .await;
    assert_eq!(field(&reply, "parent_id"), "too_deep");
    let reply = api
        .move_to(&owner, &tip, json!({ "parent_id": "level-99" }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(api.trash(&owner, &leaf).await.status, 204);
}

#[tokio::test]
async fn trashing_takes_the_descendants_with_one_date() {
    let (api, owner, _, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;
    let grandchild = api.page(&owner, &space, Some(&child)).await;
    let other = api.page(&owner, &space, None).await;

    api.trash_ok(&owner, &root).await;
    let date = api.deleted_at(&root).await;
    assert!(date.is_some());
    assert_eq!(api.deleted_at(&child).await, date);
    assert_eq!(api.deleted_at(&grandchild).await, date);
    assert_eq!(api.deleted_at(&other).await, None);
    let by = api
        .number(&format!(
            "SELECT count(*) FROM pages WHERE deleted_by IS NOT NULL AND id = '{grandchild}'"
        ))
        .await;
    assert_eq!(by, 1);
}

#[tokio::test]
async fn a_trashed_page_behaves_as_absent() {
    let (api, owner, ws, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;
    let other = api.page(&owner, &space, None).await;
    let reply = api.read(&owner, &root).await;
    let slug = reply.body["slug"].as_str().unwrap().to_owned();
    api.trash_ok(&owner, &root).await;

    assert_eq!(api.read(&owner, &root).await.status, 404);
    assert_eq!(api.read(&owner, &child).await.status, 404);
    let path = format!("/workspaces/{ws}/pages/by-slug/{slug}");
    assert_eq!(api.get(&owner, &path).await.status, 404);
    assert_eq!(
        api.tree(&owner, &space, None).await,
        std::slice::from_ref(&other)
    );
    assert_eq!(api.trash(&owner, &root).await.status, 404);
    assert_eq!(api.move_to(&owner, &root, json!({})).await.status, 404);
    let reply = api
        .move_to(&owner, &other, json!({ "parent_id": root }))
        .await;
    assert_eq!(field(&reply, "parent_id"), "unknown");
    assert!(ids(&api.get(&owner, &format!("/workspaces/{ws}/recent")).await).contains(&other));
    let recent = ids(&api.get(&owner, &format!("/workspaces/{ws}/recent")).await);
    assert!(!recent.contains(&root) && !recent.contains(&child));
}

#[tokio::test]
async fn a_parent_stops_counting_its_trashed_children() {
    let (api, owner, _, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;
    api.trash_ok(&owner, &child).await;
    assert_eq!(api.read(&owner, &root).await.body["has_children"], false);
}

#[tokio::test]
async fn the_trash_lists_the_last_trashed_first() {
    let (api, owner, _, space) = space().await;
    let first = api.page(&owner, &space, None).await;
    let second = api.page(&owner, &space, None).await;
    let third = api.page(&owner, &space, None).await;
    let live = api.page(&owner, &space, None).await;
    for page in [&first, &second, &third] {
        api.trash_ok(&owner, page).await;
    }

    let list = api.trash_list(&owner, &space).await;
    assert_eq!(list, [third.clone(), second.clone(), first.clone()]);
    assert!(!list.contains(&live));

    let page = api
        .get(&owner, &format!("/spaces/{space}/trash?limit=2"))
        .await;
    assert_eq!(ids(&page), [third, second]);
    let cursor = page.body["next_cursor"].as_str().unwrap();
    let next = api
        .get(
            &owner,
            &format!("/spaces/{space}/trash?limit=2&cursor={cursor}"),
        )
        .await;
    assert_eq!(ids(&next), [first]);
    assert_eq!(next.body["next_cursor"], Value::Null);

    let reply = api
        .get(&owner, &format!("/spaces/{space}/trash?cursor=nope"))
        .await;
    assert_eq!(field(&reply, "cursor"), "invalid");
}

#[tokio::test]
async fn restoring_brings_back_what_was_trashed_with_the_page() {
    let (api, owner, _, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let kept = api.page(&owner, &space, Some(&root)).await;
    let earlier = api.page(&owner, &space, Some(&root)).await;
    let below = api.page(&owner, &space, Some(&earlier)).await;

    api.trash_ok(&owner, &earlier).await;
    api.trash_ok(&owner, &root).await;
    assert_ne!(api.deleted_at(&earlier).await, api.deleted_at(&root).await);

    let reply = api.restore(&owner, &root).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["deleted_at"], Value::Null);
    assert_eq!(reply.body["deleted_by"], Value::Null);
    assert_eq!(api.tree(&owner, &space, Some(&root)).await, [kept]);
    let trash = api.trash_list(&owner, &space).await;
    assert!(trash.contains(&earlier) && trash.contains(&below) && trash.len() == 2);
}

#[tokio::test]
async fn pages_trashed_within_one_millisecond_stay_apart() {
    let (api, owner, _, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;
    let future = i64::MAX / 2;
    api.sql(&format!(
        "UPDATE pages SET deleted_at = {future}, deleted_by = NULL WHERE id = '{child}'"
    ))
    .await;

    api.trash_ok(&owner, &root).await;
    assert_ne!(api.deleted_at(&root).await, api.deleted_at(&child).await);
    assert_eq!(api.restore(&owner, &root).await.status, 200);
    assert!(api.deleted_at(&child).await.is_some());
}

#[tokio::test]
async fn a_page_keeps_its_place_when_restored() {
    let (api, owner, _, space) = space().await;
    let a = api.page(&owner, &space, None).await;
    let b = api.page(&owner, &space, None).await;
    let c = api.page(&owner, &space, None).await;
    api.trash_ok(&owner, &b).await;

    // Their keys must not take the one `b` keeps in the trash.
    let new = id(&api.create(&owner, &space, json!({ "after": a })).await);
    let last = api.page(&owner, &space, None).await;
    assert_eq!(api.restore(&owner, &b).await.status, 200);

    let tree = api.tree(&owner, &space, None).await;
    assert_eq!(tree.len(), 5);
    assert_eq!((&tree[0], &tree[3], &tree[4]), (&a, &c, &last));
    assert!(tree[1..3].contains(&new) && tree[1..3].contains(&b));
    let distinct = api
        .number(&format!(
            "SELECT count(DISTINCT position) FROM pages WHERE space_id = '{space}'"
        ))
        .await;
    assert_eq!(distinct, 5);
}

#[tokio::test]
async fn a_page_goes_to_the_root_when_its_parent_stays_in_the_trash() {
    let (api, owner, _, space) = space().await;
    let root = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;
    let below = api.page(&owner, &space, Some(&child)).await;
    let other = api.page(&owner, &space, None).await;
    api.trash_ok(&owner, &root).await;

    let reply = api.restore(&owner, &child).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["parent_id"], Value::Null);
    assert_eq!(api.tree(&owner, &space, None).await, [other, child.clone()]);
    assert_eq!(api.tree(&owner, &space, Some(&child)).await, [below]);
    assert_eq!(api.trash_list(&owner, &space).await, [root]);
}

#[tokio::test]
async fn restoring_needs_an_editor_and_a_page_in_the_trash() {
    let (api, owner, ws, space) = space().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let page = api.page(&owner, &space, None).await;

    let reply = api.restore(&owner, &page).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    api.trash_ok(&owner, &page).await;
    assert_eq!(api.restore(&bob, &page).await.status, 404);
    api.grant(&owner, &space, BOB, "viewer").await;
    assert_eq!(api.restore(&bob, &page).await.status, 404);
    api.grant(&owner, &space, BOB, "editor").await;
    assert_eq!(api.restore(&bob, &page).await.status, 200);
}

#[tokio::test]
async fn purging_needs_a_space_admin_and_a_page_in_the_trash() {
    let (api, owner, ws, space) = space().await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    let root = api.page(&owner, &space, None).await;
    let child = api.page(&owner, &space, Some(&root)).await;

    let reply = api.purge(&owner, &root).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));

    api.trash_ok(&owner, &root).await;
    assert_eq!(api.purge(&bob, &root).await.status, 404);
    api.grant(&owner, &space, BOB, "viewer").await;
    assert_eq!(api.purge(&bob, &root).await.status, 404);
    api.grant(&owner, &space, BOB, "editor").await;
    let reply = api.purge(&bob, &root).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    assert_eq!(api.row_count(&root).await, 1);

    api.grant(&owner, &space, BOB, "admin").await;
    assert_eq!(api.purge(&bob, &root).await.status, 204);
    assert_eq!(api.row_count(&root).await, 0);
    assert_eq!(api.row_count(&child).await, 0);
    assert_eq!(api.purge(&bob, &root).await.status, 404);
}

#[tokio::test]
async fn a_trashed_space_hides_its_pages_from_everyone() {
    let (api, owner, ws, space) = space().await;
    let page = api.page(&owner, &space, None).await;
    let gone = api.page(&owner, &space, None).await;
    api.trash_ok(&owner, &gone).await;
    let reply = api
        .raw("DELETE", &format!("/spaces/{space}"), &owner.headers(), b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");

    assert_eq!(api.read(&owner, &page).await.status, 404);
    assert_eq!(api.create(&owner, &space, json!({})).await.status, 404);
    assert_eq!(api.trash(&owner, &page).await.status, 404);
    assert_eq!(api.restore(&owner, &gone).await.status, 404);
    assert_eq!(api.purge(&owner, &gone).await.status, 404);
    assert_eq!(
        api.get(&owner, &format!("/spaces/{space}/tree"))
            .await
            .status,
        404
    );
    assert_eq!(
        api.get(&owner, &format!("/spaces/{space}/trash"))
            .await
            .status,
        404
    );
    assert!(ids(&api.get(&owner, &format!("/workspaces/{ws}/recent")).await).is_empty());
}

#[tokio::test]
async fn a_move_cannot_reach_a_trashed_space() {
    let (api, owner, ws, space) = space().await;
    let target = api.make_space(&owner, &ws, Value::Null).await;
    let page = api.page(&owner, &space, None).await;
    let reply = api
        .raw(
            "DELETE",
            &format!("/spaces/{target}"),
            &owner.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");

    let reply = api
        .move_to(&owner, &page, json!({ "space_id": target }))
        .await;
    assert_eq!(reply.status, 404);
}

#[tokio::test]
async fn recent_pages_come_from_readable_spaces_last_changed_first() {
    let (api, owner, ws, open) = space().await;
    let closed = api.make_space(&owner, &ws, Value::Null).await;
    let bob = api.add_member(&ws, BOB, "bob@example.org", "member").await;
    api.grant(&owner, &open, BOB, "viewer").await;

    let older = api.page(&owner, &open, None).await;
    let newer = api.page(&owner, &open, None).await;
    let hidden = api.page(&owner, &closed, None).await;
    api.sql(&format!(
        "UPDATE pages SET updated_at = 100 WHERE id = '{older}'"
    ))
    .await;
    api.sql(&format!(
        "UPDATE pages SET updated_at = 200 WHERE id = '{newer}'"
    ))
    .await;
    api.sql(&format!(
        "UPDATE pages SET updated_at = 300 WHERE id = '{hidden}'"
    ))
    .await;

    let path = format!("/workspaces/{ws}/recent");
    let reply = api.get(&bob, &path).await;
    assert_eq!(ids(&reply), [newer.clone(), older.clone()]);
    assert_eq!(reply.body["items"][0]["role"], "viewer");
    assert_eq!(
        ids(&api.get(&owner, &path).await),
        [hidden, newer.clone(), older.clone()]
    );

    let first = api.get(&bob, &format!("{path}?limit=1")).await;
    assert_eq!(ids(&first), [newer]);
    let cursor = first.body["next_cursor"].as_str().unwrap();
    let second = api
        .get(&bob, &format!("{path}?limit=1&cursor={cursor}"))
        .await;
    assert_eq!(ids(&second), [older]);
    assert_eq!(second.body["next_cursor"], Value::Null);
}

#[tokio::test]
async fn recent_pages_are_for_members() {
    let (api, _, ws, _) = space().await;
    let stranger = api.add_and_login(BOB, "bob@example.org").await;
    let reply = api
        .get(&stranger, &format!("/workspaces/{ws}/recent"))
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn an_unknown_field_is_refused() {
    let (api, owner, _, space) = space().await;
    let reply = api.create(&owner, &space, json!({ "title": "Nope" })).await;
    assert_eq!(reply.status, 400, "{reply:?}");
}

#[tokio::test]
async fn a_cursor_the_server_did_not_give_is_refused() {
    let (api, owner, ws, space) = space().await;
    let page = api.page(&owner, &space, None).await;
    for cursor in ["5_".to_owned(), "5_nope".to_owned(), format!("+5_{page}")] {
        let reply = api
            .get(&owner, &format!("/workspaces/{ws}/recent?cursor={cursor}"))
            .await;
        assert_eq!(field(&reply, "cursor"), "invalid", "{cursor}");
    }
}

#[tokio::test]
async fn the_database_refuses_two_siblings_on_one_key() {
    let (api, owner, _, space) = space().await;
    let page = api.page(&owner, &space, None).await;
    let statement = format!(
        "INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
         SELECT 'twin', workspace_id, space_id, position, 'twin', 1, 1
         FROM pages WHERE id = '{page}'"
    );
    let result = sqlx::query(sqlx::AssertSqlSafe(statement))
        .execute(&api.db)
        .await;
    assert!(result.is_err());
}
