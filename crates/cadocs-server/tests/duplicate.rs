//! Duplicating a page with its descendants and attachments, over REST (docs/API.md#pages).

mod common;

use common::{Api, Login, ORIGIN, Reply, start_tuned};
use serde_json::{Value, json};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const OTHER: &str = "33333333-0000-7000-8000-000000000001";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";
const PDF: &[u8] = b"%PDF-1.4 a report";

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

/// The attachment ids an exported page links to, in order.
fn linked(markdown: &str) -> Vec<String> {
    markdown
        .split(&format!("{ORIGIN}/api/v1/attachments/"))
        .skip(1)
        .map(|rest| rest[..36].to_owned())
        .collect()
}

struct Wiki {
    api: Api,
    ada: Login,
    workspace: String,
    space: String,
}

async fn wiki(settings: &[(&str, &str)]) -> Wiki {
    let api = start_tuned(ORIGIN, "", settings).await;
    let reply = api.setup().await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let ada = Login::from(&reply);
    let workspace = api.first_workspace().await;
    let mut wiki = Wiki {
        api,
        ada,
        workspace,
        space: String::new(),
    };
    wiki.space = wiki.new_space(&wiki.workspace.clone()).await;
    wiki
}

impl Wiki {
    async fn new_space(&self, workspace: &str) -> String {
        let path = format!("/workspaces/{workspace}/spaces");
        let body = json!({ "name": "Handbook", "default_role": null });
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), body)
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn page(&self, space: &str, parent: Option<&str>) -> String {
        let path = format!("/spaces/{space}/pages");
        let body = json!({ "parent_id": parent });
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), body)
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn attach(&self, page: &str, name: &str, kind: &str, body: &[u8]) -> String {
        let path = format!("/pages/{page}/attachments?name={name}");
        let mut headers = self.ada.headers().to_vec();
        headers.push(("content-type", kind));
        let reply = self.api.raw("POST", &path, &headers, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    /// A page imported by `ada` from Markdown, under `parent` if given.
    async fn imported(&self, markdown: &str, parent: Option<&str>) -> String {
        let parent = parent.map_or(String::new(), |parent| format!("&parent_id={parent}"));
        let path = format!("/spaces/{}/import?name=notes.md{parent}", self.space);
        let mut headers = self.ada.headers().to_vec();
        headers.push(("content-type", "text/markdown"));
        let reply = self
            .api
            .raw("POST", &path, &headers, markdown.as_bytes())
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn markdown(&self, page: &str) -> String {
        let path = format!("/pages/{page}/export?format=md");
        let reply = self.api.get(&self.ada, &path).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        String::from_utf8(reply.bytes).unwrap()
    }

    async fn duplicate(&self, by: &Login, page: &str, body: Value) -> Reply {
        let path = format!("/pages/{page}/duplicate");
        self.api.send("POST", &path, &by.headers(), body).await
    }

    async fn copy(&self, page: &str, body: Value) -> String {
        let reply = self.duplicate(&self.ada, page, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn tree(&self, space: &str, parent: Option<&str>) -> Vec<String> {
        let query = parent.map_or(String::new(), |parent| format!("?parent={parent}"));
        let path = format!("/spaces/{space}/tree{query}");
        ids(&self.api.get(&self.ada, &path).await)
    }

    async fn grant(&self, space: &str, user: &str, role: &str) {
        let body = json!({ "grants": [{ "user_id": user, "role": role }] });
        let path = format!("/spaces/{space}/grants");
        let reply = self.api.send("PUT", &path, &self.ada.headers(), body).await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }
}

#[tokio::test]
async fn a_copy_takes_its_descendants_with_copies_of_their_attachments() {
    let wiki = wiki(&[]).await;
    let holder = wiki.page(&wiki.space, None).await;
    let image = wiki.attach(&holder, "chart.png", "image/png", PNG).await;
    let file = wiki
        .attach(&holder, "Q3+report.pdf", "application/pdf", PDF)
        .await;
    let root = wiki
        .imported(
            &format!(
                "# Quarterly review\n\nNumbers went **up**.\n\n![the chart](/api/v1/attachments/{image})\n\n\
                 [Q3 report.pdf](/api/v1/attachments/{file})\n"
            ),
            None,
        )
        .await;
    let child = wiki
        .imported(
            &format!("# Details\n\n![the chart again](/api/v1/attachments/{image})\n"),
            Some(&root),
        )
        .await;
    let grandchild = wiki.imported("# Notes\n\nPlain.\n", Some(&child)).await;

    let reply = wiki.duplicate(&wiki.ada, &root, json!({})).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["title"], "Quarterly review");
    assert_eq!(reply.body["space_id"], wiki.space);
    assert_eq!(reply.body["parent_id"], Value::Null);
    assert_eq!(reply.body["has_children"], true);
    let copy = id(&reply);
    assert_eq!(
        wiki.tree(&wiki.space, None).await,
        [holder, root.clone(), copy.clone()]
    );

    // The same content, with attachments of its own that hold the same bytes.
    let exported = wiki.markdown(&copy).await;
    let copied = linked(&exported);
    assert_eq!(copied.len(), 2, "{exported}");
    assert!(
        !copied.contains(&image) && !copied.contains(&file),
        "{exported}"
    );
    assert_eq!(
        exported,
        format!(
            "# Quarterly review\n\nNumbers went **up**.\n\n\
             ![the chart]({ORIGIN}/api/v1/attachments/{})\n\n\
             [Q3 report.pdf]({ORIGIN}/api/v1/attachments/{})\n",
            copied[0], copied[1]
        )
    );

    // Descendants come along, in their order, and share the copies of the files they show.
    let children = wiki.tree(&wiki.space, Some(&copy)).await;
    assert_eq!(children.len(), 1);
    assert_ne!(children[0], child);
    let exported = wiki.markdown(&children[0]).await;
    assert_eq!(
        exported,
        format!(
            "# Details\n\n![the chart again]({ORIGIN}/api/v1/attachments/{})\n",
            copied[0]
        )
    );
    let grandchildren = wiki.tree(&wiki.space, Some(&children[0])).await;
    assert_eq!(grandchildren.len(), 1);
    assert_ne!(grandchildren[0], grandchild);
    assert_eq!(
        wiki.markdown(&grandchildren[0]).await,
        "# Notes\n\nPlain.\n"
    );

    // The copies do not depend on the source's files.
    for gone in [&image, &file] {
        let reply = wiki
            .api
            .raw(
                "DELETE",
                &format!("/attachments/{gone}"),
                &wiki.ada.headers(),
                b"",
            )
            .await;
        assert_eq!(reply.status, 204, "{reply:?}");
    }
    let reply = wiki
        .api
        .get(&wiki.ada, &format!("/attachments/{}", copied[0]))
        .await;
    assert_eq!((reply.status, reply.bytes.as_slice()), (200, PNG));
    assert_eq!(reply.header("content-type"), Some("image/png"));
    let reply = wiki
        .api
        .get(&wiki.ada, &format!("/attachments/{}", copied[1]))
        .await;
    assert_eq!((reply.status, reply.bytes.as_slice()), (200, PDF));
    assert!(
        reply
            .header("content-disposition")
            .unwrap()
            .contains("Q3%20report.pdf")
    );
}

#[tokio::test]
async fn links_between_copied_pages_point_at_the_copies() {
    let wiki = wiki(&[]).await;
    let outside = wiki.imported("# Outside\n\nAlone.\n", None).await;
    let root = wiki
        .imported(&format!("# Root\n\n[x](/?page={outside})\n"), None)
        .await;
    let first = wiki
        .imported(
            &format!("# First\n\n[x](/?page={root}) [x]({ORIGIN}/?page={outside})\n"),
            Some(&root),
        )
        .await;
    wiki.imported(&format!("# Second\n\n[x](/?page={first})\n"), Some(&root))
        .await;
    // Titles show as the store has them, which a flush may not have written yet: ids only.
    let pages = |markdown: String| -> Vec<String> {
        markdown
            .split(&format!("{ORIGIN}/?page="))
            .skip(1)
            .map(|rest| rest[..36].to_owned())
            .collect()
    };

    let other = wiki.new_space(&wiki.workspace).await;
    for space in [wiki.space.clone(), other] {
        let copy = wiki.copy(&root, json!({ "space_id": space })).await;
        let children = wiki.tree(&space, Some(&copy)).await;
        assert_eq!(children.len(), 2);
        assert_eq!(pages(wiki.markdown(&copy).await), [outside.as_str()]);
        assert_eq!(
            pages(wiki.markdown(&children[0]).await),
            [copy.clone(), outside.clone()]
        );
        assert_eq!(
            pages(wiki.markdown(&children[1]).await),
            [children[0].clone()]
        );
    }
}

#[tokio::test]
async fn a_copy_goes_where_it_is_asked() {
    let wiki = wiki(&[]).await;
    let first = wiki.page(&wiki.space, None).await;
    let second = wiki.page(&wiki.space, None).await;
    let child = wiki.page(&wiki.space, Some(&first)).await;

    // Right after the page, as the page menu asks.
    let copy = wiki
        .copy(&first, json!({ "parent_id": null, "after": first }))
        .await;
    assert_eq!(
        wiki.tree(&wiki.space, None).await,
        [first.clone(), copy.clone(), second.clone()]
    );

    // Under a page, first among its children, the source itself included.
    let under = wiki
        .copy(&second, json!({ "parent_id": first, "after": null }))
        .await;
    let into_itself = wiki.copy(&first, json!({ "parent_id": first })).await;
    assert_eq!(
        wiki.tree(&wiki.space, Some(&first)).await,
        [under, child.clone(), into_itself.clone()]
    );
    // What sat under the source when the copy started, and nothing more.
    assert_eq!(wiki.tree(&wiki.space, Some(&into_itself)).await.len(), 2);

    // At the root of another space.
    let other = wiki.new_space(&wiki.workspace).await;
    let reply = wiki
        .duplicate(&wiki.ada, &first, json!({ "space_id": other }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["space_id"], other);
    let landed = id(&reply);
    assert_eq!(wiki.tree(&other, None).await, [landed.as_str()]);
    let below = wiki.tree(&other, Some(&landed)).await;
    assert_eq!(below.len(), 3);
    for page in &below {
        let reply = wiki.api.get(&wiki.ada, &format!("/pages/{page}")).await;
        assert_eq!(reply.body["space_id"], other, "{reply:?}");
    }

    // A place that does not exist.
    let reply = wiki
        .duplicate(
            &wiki.ada,
            &first,
            json!({ "parent_id": second, "space_id": other }),
        )
        .await;
    assert_eq!(field(&reply, "parent_id"), "unknown");
    let reply = wiki
        .duplicate(&wiki.ada, &first, json!({ "after": child }))
        .await;
    assert_eq!(field(&reply, "after"), "unknown");
}

#[tokio::test]
async fn a_copy_leaves_trashed_pages_out() {
    let wiki = wiki(&[]).await;
    let root = wiki.page(&wiki.space, None).await;
    let kept = wiki.page(&wiki.space, Some(&root)).await;
    let trashed = wiki.page(&wiki.space, Some(&root)).await;
    wiki.page(&wiki.space, Some(&trashed)).await;
    let reply = wiki
        .api
        .raw(
            "DELETE",
            &format!("/pages/{trashed}"),
            &wiki.ada.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");

    let copy = wiki.copy(&root, json!({})).await;
    let children = wiki.tree(&wiki.space, Some(&copy)).await;
    assert_eq!(children.len(), 1);
    assert_ne!(children[0], kept);
    assert!(wiki.tree(&wiki.space, Some(&children[0])).await.is_empty());

    let reply = wiki.duplicate(&wiki.ada, &trashed, json!({})).await;
    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test]
async fn a_copy_needs_to_read_the_page_and_create_pages_in_the_target() {
    let wiki = wiki(&[]).await;
    let target = wiki.new_space(&wiki.workspace).await;
    let page = wiki.page(&wiki.space, None).await;
    let bob = wiki
        .api
        .add_member(&wiki.workspace, BOB, "bob@example.org", "member")
        .await;
    let elsewhere = json!({ "space_id": target });

    // A page bob cannot read is not there.
    wiki.grant(&target, BOB, "editor").await;
    let reply = wiki.duplicate(&bob, &page, elsewhere.clone()).await;
    assert_eq!(reply.status, 404, "{reply:?}");

    // Reading the page is enough to copy it where bob creates pages.
    wiki.grant(&wiki.space, BOB, "viewer").await;
    let reply = wiki.duplicate(&bob, &page, elsewhere.clone()).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["created_by"], BOB);
    assert_eq!(reply.body["role"], "editor");
    let reply = wiki.duplicate(&bob, &page, json!({})).await;
    assert_eq!(reply.status, 403, "{reply:?}");

    // A target bob reads but does not edit, then one he cannot read.
    wiki.grant(&target, BOB, "commenter").await;
    let reply = wiki.duplicate(&bob, &page, elsewhere.clone()).await;
    assert_eq!(reply.status, 403, "{reply:?}");
    wiki.api
        .sql(&format!(
            "DELETE FROM space_grants WHERE space_id = '{target}'"
        ))
        .await;
    let reply = wiki.duplicate(&bob, &page, elsewhere).await;
    assert_eq!(reply.status, 404, "{reply:?}");
    assert_eq!(wiki.tree(&target, None).await.len(), 1);
}

#[tokio::test]
async fn a_copy_across_workspaces_is_refused() {
    let wiki = wiki(&[]).await;
    let page = wiki.page(&wiki.space, None).await;
    let ws = &wiki.workspace;
    wiki.api
        .sql(&format!(
            "INSERT INTO workspaces (id, slug, name, language, created_at, updated_at)
             VALUES ('{OTHER}', 'other', 'Other', 'en', 1, 1);
             INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             SELECT '{OTHER}', user_id, 'owner', 1 FROM workspace_members WHERE workspace_id = '{ws}'"
        ))
        .await;
    let target = wiki.new_space(OTHER).await;

    let reply = wiki
        .duplicate(&wiki.ada, &page, json!({ "space_id": target }))
        .await;
    assert_eq!(field(&reply, "space_id"), "other_workspace");
    assert!(wiki.tree(&target, None).await.is_empty());
}

#[tokio::test]
async fn a_copy_has_a_size_and_a_depth() {
    let wiki = wiki(&[]).await;
    let root = wiki.page(&wiki.space, None).await;
    // One page over the limit, written straight into the database for speed.
    for n in 0..500 {
        wiki.api
            .sql(&format!(
                "INSERT INTO pages (id, workspace_id, space_id, parent_id, position, slug,
                                    created_at, updated_at)
                 SELECT 'many-{n}', workspace_id, space_id, '{root}', 'p{n:04}', 'many-{n}', 1, 1
                 FROM pages WHERE id = '{root}'"
            ))
            .await;
    }
    let reply = wiki.duplicate(&wiki.ada, &root, json!({})).await;
    assert_eq!(
        (reply.status, reply.code()),
        (413, "too_large"),
        "{reply:?}"
    );
    assert_eq!(wiki.tree(&wiki.space, None).await, [root.as_str()]);

    // A chain of 100 levels has no room for a copy below its top.
    let top = wiki.page(&wiki.space, None).await;
    let mut last = top.clone();
    for level in 2..=100 {
        wiki.api
            .sql(&format!(
                "INSERT INTO pages (id, workspace_id, space_id, parent_id, position, slug,
                                    created_at, updated_at)
                 SELECT 'level-{level}', workspace_id, space_id, '{last}', 'V', 'level-{level}', 1, 1
                 FROM pages WHERE id = '{top}'"
            ))
            .await;
        last = format!("level-{level}");
    }
    let reply = wiki
        .duplicate(&wiki.ada, &top, json!({ "parent_id": top }))
        .await;
    assert_eq!(field(&reply, "parent_id"), "too_deep");
    let copy = wiki.copy(&top, json!({})).await;
    assert_eq!(wiki.tree(&wiki.space, Some(&copy)).await.len(), 1);
}

#[tokio::test]
async fn a_copy_over_the_quota_goes_to_the_trash() {
    let wiki = wiki(&[("CADOCS_WORKSPACE_QUOTA_MB", "1")]).await;
    let holder = wiki.page(&wiki.space, None).await;
    let big = vec![7u8; 600 * 1024];
    let file = wiki
        .attach(&holder, "big.bin", "application/octet-stream", &big)
        .await;
    let root = wiki
        .imported(
            &format!("# Big\n\n[big.bin](/api/v1/attachments/{file})\n"),
            None,
        )
        .await;
    wiki.page(&wiki.space, Some(&root)).await;

    let reply = wiki.duplicate(&wiki.ada, &root, json!({})).await;
    assert_eq!(
        (reply.status, reply.code()),
        (413, "too_large"),
        "{reply:?}"
    );
    assert_eq!(wiki.tree(&wiki.space, None).await, [holder, root]);
    let path = format!("/spaces/{}/trash", wiki.space);
    assert_eq!(ids(&wiki.api.get(&wiki.ada, &path).await).len(), 2);
    let stored = wiki.api.number("SELECT count(*) FROM attachments").await;
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn the_route_needs_a_session_and_the_csrf_token() {
    let wiki = wiki(&[]).await;
    let page = wiki.page(&wiki.space, None).await;
    let path = format!("/pages/{page}/duplicate");
    let headers = [("content-type", "application/json")];
    let reply = wiki.api.raw("POST", &path, &headers, b"{}").await;
    assert_eq!(reply.status, 401, "{reply:?}");
    let cookie = [("cookie", wiki.ada.cookie.as_str()), headers[0]];
    let reply = wiki.api.raw("POST", &path, &cookie, b"{}").await;
    assert_eq!(reply.status, 403, "{reply:?}");
    let reply = wiki
        .duplicate(&wiki.ada, &page, json!({ "title": "Copy" }))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
}
