//! Templates of a space, over REST (docs/API.md#templates).

mod common;

use common::{Api, Login, ORIGIN, Reply, start_tuned};
use serde_json::{Value, json};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const OTHER: &str = "33333333-0000-7000-8000-000000000001";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";

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

impl Wiki {
    async fn save(&self, by: &Login, page: &str) -> Reply {
        let path = format!("/pages/{page}/template");
        self.api.send("POST", &path, &by.headers(), json!({})).await
    }

    async fn start(&self, by: &Login, template: &str, body: Value) -> Reply {
        let path = format!("/templates/{template}/pages");
        self.api.send("POST", &path, &by.headers(), body).await
    }

    async fn templates(&self, by: &Login, space: &str) -> Reply {
        self.api
            .get(by, &format!("/spaces/{space}/templates"))
            .await
    }
}

#[tokio::test]
async fn a_page_starts_from_a_template_with_copies_of_its_files() {
    let wiki = wiki(&[]).await;
    let holder = wiki.page(&wiki.space, None).await;
    let image = wiki.attach(&holder, "chart.png", "image/png", PNG).await;
    let source = wiki
        .imported(
            &format!(
                "# Weekly notes\n\nAgenda **here**.\n\n![chart](/api/v1/attachments/{image})\n"
            ),
            None,
        )
        .await;
    let reply = wiki.save(&wiki.ada, &source).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["title"], "Weekly notes");
    assert_eq!(reply.body["space_id"], wiki.space);
    let template = id(&reply);
    let listed = wiki.templates(&wiki.ada, &wiki.space).await;
    assert_eq!(ids(&listed), std::slice::from_ref(&template));

    // The template is a snapshot: the page and its files can go.
    let reply = wiki
        .api
        .send(
            "DELETE",
            &format!("/pages/{source}"),
            &wiki.ada.headers(),
            json!({}),
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");

    let reply = wiki.start(&wiki.ada, &template, json!({})).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["title"], "Weekly notes");
    assert_eq!(reply.body["created_by"].as_str().unwrap().len(), 36);
    let page = id(&reply);
    let exported = wiki.markdown(&page).await;
    let copied = linked(&exported);
    assert_eq!(copied.len(), 1, "{exported}");
    assert_ne!(copied[0], image);
    assert_eq!(
        exported,
        format!(
            "# Weekly notes\n\nAgenda **here**.\n\n![chart]({ORIGIN}/api/v1/attachments/{})\n",
            copied[0]
        )
    );
    // Starting again gives another page, with another copy of the file.
    let again = id(&wiki.start(&wiki.ada, &template, json!({})).await);
    assert_ne!(again, page);
    assert_ne!(linked(&wiki.markdown(&again).await), copied);
}

#[tokio::test]
async fn templates_follow_the_roles_of_their_space() {
    let wiki = wiki(&[]).await;
    let target = wiki.new_space(&wiki.workspace).await;
    let source = wiki.imported("# Form\n\nText.\n", None).await;
    let template = id(&wiki.save(&wiki.ada, &source).await);
    let bob = wiki
        .api
        .add_member(&wiki.workspace, BOB, "bob@example.org", "member")
        .await;

    // Nobody outside the space sees it.
    assert_eq!(wiki.templates(&bob, &wiki.space).await.status, 404);
    assert_eq!(wiki.start(&bob, &template, json!({})).await.status, 404);
    let reply = wiki
        .api
        .send(
            "DELETE",
            &format!("/templates/{template}"),
            &bob.headers(),
            json!({}),
        )
        .await;
    assert_eq!(reply.status, 404, "{reply:?}");

    // A viewer lists, and starts a page only where they edit.
    wiki.grant(&wiki.space, BOB, "viewer").await;
    assert_eq!(
        ids(&wiki.templates(&bob, &wiki.space).await),
        std::slice::from_ref(&template)
    );
    assert_eq!(wiki.save(&bob, &source).await.status, 403);
    assert_eq!(wiki.start(&bob, &template, json!({})).await.status, 403);
    let reply = wiki
        .api
        .send(
            "DELETE",
            &format!("/templates/{template}"),
            &bob.headers(),
            json!({}),
        )
        .await;
    assert_eq!(reply.status, 403, "{reply:?}");
    wiki.grant(&target, BOB, "editor").await;
    let reply = wiki
        .start(&bob, &template, json!({ "space_id": target }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["space_id"], target);
    assert_eq!(reply.body["title"], "Form");

    // An editor of the space saves and deletes.
    wiki.grant(&wiki.space, BOB, "editor").await;
    let second = id(&wiki.save(&bob, &source).await);
    assert_eq!(
        wiki.templates(&bob, &wiki.space).await.body["items"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    let reply = wiki
        .api
        .send(
            "DELETE",
            &format!("/templates/{second}"),
            &bob.headers(),
            json!({}),
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(wiki.start(&bob, &second, json!({})).await.status, 404);
}

#[tokio::test]
async fn a_template_does_not_cross_workspaces() {
    let wiki = wiki(&[]).await;
    let source = wiki.imported("# Form\n", None).await;
    let template = id(&wiki.save(&wiki.ada, &source).await);
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
        .start(&wiki.ada, &template, json!({ "space_id": target }))
        .await;
    assert_eq!(field(&reply, "space_id"), "other_workspace");
    assert!(wiki.tree(&target, None).await.is_empty());
}

#[tokio::test]
async fn a_space_holds_a_bounded_number_of_templates() {
    let wiki = wiki(&[]).await;
    let source = wiki.imported("# Form\n", None).await;
    let mut ids = Vec::new();
    for n in 0..200 {
        ids.push(format!(
            "('00000000-0000-7000-8000-{n:012}', '{}', 'T', x'00', 1)",
            wiki.space
        ));
    }
    wiki.api
        .sql(&format!(
            "INSERT INTO templates (id, space_id, title, state, created_at) VALUES {}",
            ids.join(",")
        ))
        .await;
    let reply = wiki.save(&wiki.ada, &source).await;
    assert_eq!((reply.status, reply.code()), (409, "conflict"), "{reply:?}");
}
