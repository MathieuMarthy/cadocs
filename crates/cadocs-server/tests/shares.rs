//! Public shares (step 45): the REST routes that list, create and revoke them, and the pages
//! under `/s/` they open to anyone, rendered on the server with no script allowed to run.

mod common;

use cadocs_content::ast::{Block, Document, Inline, Marks};
use cadocs_content::yjs::replace_page;
use common::{Api, Login, ORIGIN, Reply, start_tuned};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::time::{Duration, Instant, sleep};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const VIC: &str = "33333333-0000-7000-8000-000000000001";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";
const PATIENCE: Duration = Duration::from_secs(10);

struct Wiki {
    api: Api,
    ada: Login,
    /// An editor of the space.
    bob: Login,
    /// A viewer of the space.
    vic: Login,
    space: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

fn text(reply: &Reply) -> String {
    String::from_utf8(reply.bytes.clone()).unwrap()
}

async fn wiki() -> Wiki {
    wiki_flushing_every("100").await
}

/// A wiki whose rooms flush every `ms` milliseconds.
async fn wiki_flushing_every(ms: &str) -> Wiki {
    let settings = [("CADOCS_FLUSH_INTERVAL_MS", ms)];
    let api = start_tuned(ORIGIN, "", &settings).await;
    let reply = api.setup().await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let ada = Login::from(&reply);
    let workspace = api.first_workspace().await;
    let bob = api
        .add_member(&workspace, BOB, "bob@example.org", "member")
        .await;
    let vic = api
        .add_member(&workspace, VIC, "vic@example.org", "member")
        .await;
    let path = format!("/workspaces/{workspace}/spaces");
    let body = json!({ "name": "Handbook", "default_role": null });
    let reply = api.send("POST", &path, &ada.headers(), body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let space = id(&reply);
    let grants = json!({ "grants": [
        { "user_id": BOB, "role": "editor" },
        { "user_id": VIC, "role": "viewer" },
    ] });
    let reply = api
        .send(
            "PUT",
            &format!("/spaces/{space}/grants"),
            &ada.headers(),
            grants,
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    Wiki {
        api,
        ada,
        bob,
        vic,
        space,
    }
}

impl Wiki {
    /// A page imported from Markdown under `parent`, by id.
    async fn page(&self, markdown: &str, parent: Option<&str>) -> String {
        let page = self.import(markdown, parent).await;
        // `pages.title`, which lists and links show, follows at the next flush.
        let deadline = Instant::now() + PATIENCE;
        loop {
            let reply = self.api.get(&self.ada, &format!("/pages/{page}")).await;
            if reply.body["title"] != "" {
                break page;
            }
            assert!(
                Instant::now() < deadline,
                "the title never reached the page"
            );
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// As `page`, without waiting for a flush.
    async fn import(&self, markdown: &str, parent: Option<&str>) -> String {
        let mut path = format!("/spaces/{}/import?name=notes.md", self.space);
        if let Some(parent) = parent {
            path.push_str(&format!("&parent_id={parent}"));
        }
        let mut headers = self.ada.headers().to_vec();
        headers.push(("content-type", "text/markdown"));
        let reply = self
            .api
            .raw("POST", &path, &headers, markdown.as_bytes())
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn slug(&self, page: &str) -> String {
        let reply = self.api.get(&self.ada, &format!("/pages/{page}")).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        reply.body["slug"].as_str().unwrap().to_owned()
    }

    async fn create(&self, by: &Login, page: &str, body: Value) -> Reply {
        let path = format!("/pages/{page}/shares");
        self.api.send("POST", &path, &by.headers(), body).await
    }

    /// The token of a new share of `page`.
    async fn share(&self, page: &str, body: Value) -> String {
        let reply = self.create(&self.ada, page, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        reply.body["token"].as_str().unwrap().to_owned()
    }

    async fn public(&self, path: &str) -> Reply {
        self.api.outside("GET", path, &[]).await
    }

    async fn upload(&self, page: &str) -> String {
        let path = format!("/pages/{page}/attachments?name=diagram.png");
        let mut headers = self.ada.headers().to_vec();
        headers.push(("content-type", "image/png"));
        let reply = self.api.raw("POST", &path, &headers, PNG).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }
}

/// The status of a public request and its body, so two refusals can be compared.
async fn refusal(wiki: &Wiki, path: &str) -> (u16, Vec<u8>) {
    let reply = wiki.public(path).await;
    (reply.status, reply.bytes)
}

#[tokio::test]
async fn editors_create_list_and_revoke_shares() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n\nHello\n", None).await;

    let reply = wiki.create(&wiki.bob, &page, json!({})).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let share = &reply.body;
    let token = share["token"].as_str().unwrap();
    assert_eq!(token.len(), 32, "128 random bits in hex");
    assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
    assert_eq!(share["page_id"], page);
    assert_eq!(share["include_children"], false);
    assert_eq!(share["indexable"], false);
    assert_eq!(share["expires_at"], Value::Null);
    assert_eq!(share["created_by"], BOB);
    assert!(share["created_at"].as_i64().unwrap() > 0);

    let later = share["created_at"].as_i64().unwrap() + 3_600_000;
    let body = json!({ "include_children": true, "indexable": true, "expires_at": later });
    let reply = wiki.create(&wiki.ada, &page, body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["include_children"], true);
    assert_eq!(reply.body["indexable"], true);
    assert_eq!(reply.body["expires_at"], later);
    assert_ne!(reply.body["token"], share["token"]);

    let list = wiki
        .api
        .get(&wiki.bob, &format!("/pages/{page}/shares"))
        .await;
    assert_eq!(list.status, 200, "{list:?}");
    let items = list.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0]["id"], share["id"]);
    assert_eq!(items[0]["token"], share["token"]);

    assert_eq!(wiki.public(&format!("/s/{token}")).await.status, 200);
    let path = format!("/shares/{}", share["id"].as_str().unwrap());
    let reply = wiki
        .api
        .raw("DELETE", &path, &wiki.bob.headers(), b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(wiki.public(&format!("/s/{token}")).await.status, 404);
    let reply = wiki
        .api
        .raw("DELETE", &path, &wiki.bob.headers(), b"")
        .await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let list = wiki
        .api
        .get(&wiki.ada, &format!("/pages/{page}/shares"))
        .await;
    assert_eq!(list.body["items"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn readers_and_strangers_cannot_manage_shares() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n", None).await;
    let share = wiki.create(&wiki.ada, &page, json!({})).await;
    let path = format!("/shares/{}", id(&share));

    // A viewer reads the page, so learns it exists: 403. Someone who cannot read it gets 404.
    let reply = wiki.create(&wiki.vic, &page, json!({})).await;
    assert_eq!(reply.status, 403, "{reply:?}");
    let reply = wiki
        .api
        .get(&wiki.vic, &format!("/pages/{page}/shares"))
        .await;
    assert_eq!(reply.status, 403, "{reply:?}");
    let reply = wiki
        .api
        .raw("DELETE", &path, &wiki.vic.headers(), b"")
        .await;
    assert_eq!(reply.status, 403, "{reply:?}");

    let workspace = wiki.api.first_workspace().await;
    let eve = wiki
        .api
        .add_member(
            &workspace,
            "44444444-0000-7000-8000-000000000001",
            "eve@example.org",
            "member",
        )
        .await;
    let reply = wiki.create(&eve, &page, json!({})).await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = wiki.api.raw("DELETE", &path, &eve.headers(), b"").await;
    assert_eq!(reply.status, 404, "{reply:?}");

    // No session, no share.
    let reply = wiki
        .api
        .send("POST", &format!("/pages/{page}/shares"), &[], json!({}))
        .await;
    assert_eq!(reply.status, 401, "{reply:?}");
}

#[tokio::test]
async fn a_share_refuses_fields_it_does_not_know_or_cannot_keep() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n", None).await;
    let reply = wiki
        .create(&wiki.ada, &page, json!({ "token": "mine" }))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    let reply = wiki
        .create(&wiki.ada, &page, json!({ "include_children": "yes" }))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    let reply = wiki
        .create(&wiki.ada, &page, json!({ "expires_at": 1 }))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["expires_at"], "past");
    assert_eq!(wiki.api.number("SELECT count(*) FROM page_shares").await, 0);
}

#[tokio::test]
async fn a_public_page_renders_its_content_without_a_session() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n\nHello **world** & co.\n", None).await;
    let token = wiki.share(&page, json!({})).await;

    let reply = wiki.public(&format!("/s/{token}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        reply.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    let body = text(&reply);
    assert!(body.starts_with("<!doctype html>"), "{body}");
    assert!(body.contains("<title>Guide</title>"), "{body}");
    assert!(
        body.contains("Hello <strong>world</strong> &amp; co."),
        "{body}"
    );
    assert_eq!(reply.header("x-robots-tag"), Some("noindex"));
    assert_eq!(reply.header("cache-control"), Some("no-cache"));
    assert!(reply.header("etag").is_some());
    assert!(reply.header("set-cookie").is_none());
    // The page itself is also reachable by its slug.
    let slug = wiki.slug(&page).await;
    let reply = wiki.public(&format!("/s/{token}/p/{slug}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert!(text(&reply).contains("<strong>world</strong>"));
}

/// The policy of a public page, as a map from directive to its sources.
fn policy(reply: &Reply) -> Vec<(String, String)> {
    let header = reply.header("content-security-policy").expect("a policy");
    header
        .split(';')
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(|d| match d.split_once(' ') {
            Some((name, sources)) => (name.to_owned(), sources.to_owned()),
            None => (d.to_owned(), String::new()),
        })
        .collect()
}

/// Standard base64 with padding, as a hash source of a policy writes it.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::new();
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().fold(0u32, |w, b| (w << 8) | u32::from(*b));
        let word = word << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(ALPHABET[((word >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn directive<'p>(policy: &'p [(String, String)], name: &str) -> Option<&'p str> {
    policy
        .iter()
        .find(|(n, _)| n == name)
        .map(|(_, sources)| sources.as_str())
}

#[tokio::test]
async fn a_script_in_content_cannot_run_on_a_public_page() {
    let wiki = wiki().await;
    let markdown = "# Guide\n\n\
        <script>window.pwned = 1</script>\n\n\
        <img src=x onerror=\"window.pwned = 1\">\n\n\
        [click](javascript:alert(1)) and [also](JaVaScRiPt:alert(1) \"t\\\" onmouseover=\\\"x\")\n\n\
        ```html\n</code></pre><script>window.pwned = 1</script>\n```\n\n\
        <a href=\"javascript:alert(1)\" onclick=\"alert(1)\">raw</a>\n";
    let page = wiki.page(markdown, None).await;
    let token = wiki.share(&page, json!({})).await;

    let reply = wiki.public(&format!("/s/{token}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let body = text(&reply);
    let lower = body.to_ascii_lowercase();
    // Content shows as text: no element or handler of its own reaches the page.
    assert!(!lower.contains("<script"), "{body}");
    assert!(!lower.contains("<img src=x"), "{body}");
    // Raw HTML shows as text, so `javascript:` may appear, but never as an address.
    assert!(!lower.contains(r#"href="javascript"#), "{body}");
    assert!(body.contains("<p>click and also</p>"), "{body}");
    for handler in ["onerror=", "onclick=", "onmouseover="] {
        let as_attribute = lower.match_indices(handler).any(|(at, _)| {
            // Escaped text keeps its `&lt;`, so the handler is never inside a tag.
            let before = &lower[..at];
            before.rfind('<') > before.rfind('>')
        });
        assert!(!as_attribute, "{handler} in {body}");
    }
    assert!(
        body.contains("&lt;script&gt;window.pwned = 1&lt;/script&gt;"),
        "{body}"
    );

    // And were one to slip through, the policy runs no script at all.
    let policy = policy(&reply);
    assert_eq!(directive(&policy, "default-src"), Some("'none'"));
    assert_eq!(directive(&policy, "script-src"), Some("'none'"));
    assert_eq!(directive(&policy, "object-src"), Some("'none'"));
    assert_eq!(directive(&policy, "base-uri"), Some("'none'"));
    assert_eq!(directive(&policy, "form-action"), Some("'none'"));
    assert_eq!(directive(&policy, "frame-ancestors"), Some("'none'"));
    let header = reply.header("content-security-policy").unwrap();
    assert!(!header.contains("unsafe"), "{header}");

    // The one style sheet is allowed by its hash, so no inline style of content applies.
    let start = body.find("<style>").unwrap() + "<style>".len();
    let end = body.find("</style>").unwrap();
    let hash = base64(&Sha256::digest(&body.as_bytes()[start..end]));
    assert_eq!(
        directive(&policy, "style-src"),
        Some(format!("'sha256-{hash}'").as_str())
    );
    assert_eq!(body.matches("<style").count(), 1, "{body}");
    assert!(!lower.contains(" style="), "{body}");
}

#[tokio::test]
async fn a_page_never_edited_shows_as_untitled() {
    let wiki = wiki().await;
    let path = format!("/spaces/{}/pages", wiki.space);
    let reply = wiki
        .api
        .send("POST", &path, &wiki.ada.headers(), json!({}))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let token = wiki.share(&id(&reply), json!({})).await;
    let reply = wiki.public(&format!("/s/{token}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert!(text(&reply).contains("<title>Untitled</title>"));
}

#[tokio::test]
async fn an_indexable_share_drops_the_robots_header() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n", None).await;
    let token = wiki.share(&page, json!({ "indexable": true })).await;
    let reply = wiki.public(&format!("/s/{token}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("x-robots-tag"), None);
}

#[tokio::test]
async fn a_public_page_revalidates_until_it_changes() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n\nHello\n", None).await;
    let token = wiki.share(&page, json!({})).await;
    let path = format!("/s/{token}");

    let first = wiki.public(&path).await;
    let etag = first.header("etag").unwrap().to_owned();
    let reply = wiki
        .api
        .outside("GET", &path, &[("if-none-match", &etag)])
        .await;
    assert_eq!(reply.status, 304, "{reply:?}");
    assert!(reply.bytes.is_empty());
    assert_eq!(reply.header("etag"), Some(etag.as_str()));

    let body = json!({ "title": "Renamed guide" });
    let reply = wiki
        .api
        .send(
            "PATCH",
            &format!("/pages/{page}"),
            &wiki.ada.headers(),
            body,
        )
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let deadline = Instant::now() + PATIENCE;
    loop {
        let reply = wiki
            .api
            .outside("GET", &path, &[("if-none-match", &etag)])
            .await;
        if reply.status == 200 {
            assert_ne!(reply.header("etag"), Some(etag.as_str()));
            assert!(text(&reply).contains("<title>Renamed guide</title>"));
            break;
        }
        assert_eq!(reply.status, 304, "{reply:?}");
        assert!(Instant::now() < deadline, "the page never changed");
        sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn an_edit_not_yet_flushed_is_never_answered_with_304() {
    // No flush during the test: the edit lives in the room only.
    let wiki = wiki_flushing_every("60000").await;
    let page = wiki.import("# Guide\n\nA secret paragraph\n", None).await;
    let token = wiki.share(&page, json!({})).await;
    let path = format!("/s/{token}");
    let first = wiki.public(&path).await;
    assert!(text(&first).contains("A secret paragraph"));
    let etag = first.header("etag").unwrap().to_owned();

    let cleaned = Document {
        blocks: vec![Block::Paragraph(vec![Inline::Text {
            text: "Nothing to see".into(),
            marks: Marks::default(),
        }])],
    };
    wiki.api
        .registry
        .rewrite(&page, "editor", 64, |txn| {
            replace_page(txn, "Guide", &cleaned);
        })
        .await
        .unwrap();
    let reply = wiki
        .api
        .outside("GET", &path, &[("if-none-match", &etag)])
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let body = text(&reply);
    assert!(!body.contains("secret"), "{body}");
    assert_ne!(reply.header("etag"), Some(etag.as_str()));
}

#[tokio::test]
async fn renaming_a_linked_page_changes_the_etag_of_the_page_linking_to_it() {
    let wiki = wiki().await;
    let root = wiki.page("# Root\n", None).await;
    let linked = wiki.page("# Before\n", Some(&root)).await;
    let markdown = format!("# Linking\n\nSee [it](/?page={linked}).\n");
    let page = wiki.page(&markdown, Some(&root)).await;
    let token = wiki.share(&root, json!({ "include_children": true })).await;
    let path = format!("/s/{token}/p/{}", wiki.slug(&page).await);
    let first = wiki.public(&path).await;
    assert!(text(&first).contains(">Before</a>"));
    let etag = first.header("etag").unwrap().to_owned();

    let body = json!({ "title": "After" });
    let reply = wiki
        .api
        .send(
            "PATCH",
            &format!("/pages/{linked}"),
            &wiki.ada.headers(),
            body,
        )
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let deadline = Instant::now() + PATIENCE;
    loop {
        let reply = wiki
            .api
            .outside("GET", &path, &[("if-none-match", &etag)])
            .await;
        if reply.status == 200 {
            assert!(text(&reply).contains(">After</a>"));
            break;
        }
        assert_eq!(reply.status, 304, "{reply:?}");
        assert!(Instant::now() < deadline, "the link kept its old title");
        sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn descendants_open_only_with_include_children() {
    let wiki = wiki().await;
    let root = wiki.page("# Root\n\nTop\n", None).await;
    let child = wiki.page("# Child\n\nMiddle\n", Some(&root)).await;
    let grandchild = wiki.page("# Grandchild\n\nBottom\n", Some(&child)).await;
    let sibling = wiki.page("# Sibling\n\nElsewhere\n", None).await;
    let (child_slug, grandchild_slug, sibling_slug) = (
        wiki.slug(&child).await,
        wiki.slug(&grandchild).await,
        wiki.slug(&sibling).await,
    );

    let alone = wiki.share(&root, json!({})).await;
    let body = text(&wiki.public(&format!("/s/{alone}")).await);
    assert!(!body.contains("Child"), "{body}");
    assert_eq!(
        wiki.public(&format!("/s/{alone}/p/{child_slug}"))
            .await
            .status,
        404
    );

    let tree = wiki.share(&root, json!({ "include_children": true })).await;
    let body = text(&wiki.public(&format!("/s/{tree}")).await);
    assert!(
        body.contains(&format!("href=\"/s/{tree}/p/{child_slug}\"")),
        "{body}"
    );
    assert!(body.contains(">Child</a>"), "{body}");

    let reply = wiki.public(&format!("/s/{tree}/p/{child_slug}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let body = text(&reply);
    assert!(body.contains("Middle"), "{body}");
    // The way back to the shared page, and on to the next level.
    assert!(body.contains(&format!("href=\"/s/{tree}\"")), "{body}");
    assert!(
        body.contains(&format!("href=\"/s/{tree}/p/{grandchild_slug}\"")),
        "{body}"
    );
    let reply = wiki.public(&format!("/s/{tree}/p/{grandchild_slug}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert!(text(&reply).contains("Bottom"));

    // A page outside the shared tree stays closed, as does one moved to the trash.
    let outside = refusal(&wiki, &format!("/s/{tree}/p/{sibling_slug}")).await;
    let unknown = refusal(&wiki, &format!("/s/{tree}/p/nothing")).await;
    assert_eq!(outside.0, 404);
    assert_eq!(outside, unknown);
    let reply = wiki
        .api
        .raw(
            "DELETE",
            &format!("/pages/{child}"),
            &wiki.ada.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(
        refusal(&wiki, &format!("/s/{tree}/p/{child_slug}")).await,
        unknown
    );
    assert_eq!(
        refusal(&wiki, &format!("/s/{tree}/p/{grandchild_slug}")).await,
        unknown
    );
    let body = text(&wiki.public(&format!("/s/{tree}")).await);
    assert!(!body.contains("Child"), "{body}");
}

#[tokio::test]
async fn links_to_pages_outside_a_share_show_no_title() {
    let wiki = wiki().await;
    let secret = wiki.page("# Secret plans\n", None).await;
    let root = wiki.page("# Root\n", None).await;
    let child = wiki.page("# Child\n", Some(&root)).await;
    let markdown = format!(
        "# Linking\n\nSee [a](/?page={secret}), [b](/?page={child}) and [c](/?page={root}).\n"
    );
    let page = wiki.page(&markdown, Some(&root)).await;
    let page_slug = wiki.slug(&page).await;
    let child_slug = wiki.slug(&child).await;

    let token = wiki.share(&root, json!({ "include_children": true })).await;
    let reply = wiki.public(&format!("/s/{token}/p/{page_slug}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let body = text(&reply);
    assert!(!body.contains("Secret"), "{body}");
    assert!(!body.contains(&secret), "{body}");
    assert!(body.contains("page not found"), "{body}");
    assert!(
        body.contains(&format!(
            r#"<a class="page-link" href="/s/{token}/p/{child_slug}">Child</a>"#
        )),
        "{body}"
    );
    assert!(
        body.contains(&format!(
            r#"<a class="page-link" href="/s/{token}">Root</a>"#
        )),
        "{body}"
    );
    // Nothing points into the application.
    assert!(!body.contains("/?page="), "{body}");
    assert!(!body.contains("/api/"), "{body}");
}

/// Waits for a public address to answer `status`: what a page's content holds reaches the files
/// a share opens through the indexing job, a moment after the edit.
async fn until_status(wiki: &Wiki, path: &str, status: u16) -> Reply {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let reply = wiki.public(path).await;
        if reply.status == status {
            break reply;
        }
        assert!(Instant::now() < deadline, "{path} kept {}", reply.status);
        sleep(Duration::from_millis(50)).await;
    }
}

/// Markdown that shows the attachment `id` as an image.
fn picture(title: &str, id: &str) -> String {
    format!("# {title}\n\n![a diagram](/api/v1/attachments/{id})\n")
}

#[tokio::test]
async fn files_open_only_through_the_content_of_a_page_the_share_covers() {
    let wiki = wiki().await;
    let root = wiki.page("# Root\n", None).await;
    let other = wiki.page("# Other\n", None).await;
    let shown = wiki.upload(&root).await;
    let hidden = wiki.upload(&other).await;
    let page = wiki.page(&picture("Pictures", &shown), None).await;
    let token = wiki.share(&page, json!({})).await;

    let path = format!("/s/{token}/files/{shown}");
    let reply = until_status(&wiki, &path, 200).await;
    assert_eq!(reply.bytes, PNG);
    assert_eq!(reply.header("content-type"), Some("image/png"));
    assert!(
        reply
            .header("content-disposition")
            .unwrap()
            .starts_with("inline")
    );
    assert!(
        reply
            .header("content-security-policy")
            .unwrap()
            .contains("sandbox")
    );
    assert_eq!(reply.header("x-robots-tag"), Some("noindex"));
    let etag = reply.header("etag").unwrap().to_owned();
    let reply = wiki
        .api
        .outside("GET", &path, &[("if-none-match", &etag)])
        .await;
    assert_eq!(reply.status, 304, "{reply:?}");
    // The image in the content points to the share's own address for it.
    let body = text(&wiki.public(&format!("/s/{token}")).await);
    assert!(
        body.contains(&format!(
            r#"<img src="/s/{token}/files/{shown}" alt="a diagram">"#
        )),
        "{body}"
    );

    let unknown = refusal(&wiki, &format!("/s/{token}/files/nothing")).await;
    assert_eq!(unknown.0, 404);
    assert_eq!(
        refusal(&wiki, &format!("/s/{token}/files/{hidden}")).await,
        unknown
    );
    // A file uploaded to a shared page, and never placed in its content, stays closed.
    let alone = wiki.share(&root, json!({})).await;
    assert_eq!(
        refusal(&wiki, &format!("/s/{alone}/files/{shown}")).await,
        unknown
    );

    // A file shown below the shared page needs the children.
    let child = wiki.page("# Child\n", Some(&root)).await;
    let below = wiki.upload(&child).await;
    wiki.page(&picture("Grandchild", &below), Some(&child))
        .await;
    let tree = wiki.share(&root, json!({ "include_children": true })).await;
    until_status(&wiki, &format!("/s/{tree}/files/{below}"), 200).await;
    assert_eq!(
        refusal(&wiki, &format!("/s/{alone}/files/{below}")).await,
        unknown
    );
}

#[tokio::test]
async fn a_file_removed_from_the_content_closes_on_the_share() {
    let wiki = wiki().await;
    let root = wiki.page("# Root\n", None).await;
    let secret = wiki.upload(&root).await;
    let page = wiki.page(&picture("Pictures", &secret), None).await;
    let token = wiki.share(&page, json!({})).await;
    let path = format!("/s/{token}/files/{secret}");
    until_status(&wiki, &path, 200).await;

    let empty = Document {
        blocks: vec![Block::Paragraph(vec![Inline::Text {
            text: "Nothing to see".into(),
            marks: Marks::default(),
        }])],
    };
    wiki.api
        .registry
        .rewrite(&page, "editor", 64, |txn| {
            replace_page(txn, "Pictures", &empty);
        })
        .await
        .unwrap();
    until_status(&wiki, &path, 404).await;
    // The file stays for the readers of the space, and for a revision that shows it.
    let reply = wiki
        .api
        .get(&wiki.ada, &format!("/attachments/{secret}"))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

#[tokio::test]
async fn closed_shares_all_answer_the_same_404() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n", None).await;
    let unknown = refusal(&wiki, "/s/00000000000000000000000000000000").await;
    assert_eq!(unknown.0, 404);
    let reply = wiki.public("/s/00000000000000000000000000000000").await;
    assert_eq!(reply.header("x-robots-tag"), Some("noindex"));
    assert!(
        reply
            .header("content-security-policy")
            .unwrap()
            .contains("script-src 'none'")
    );

    // Expired.
    let soon = wiki
        .share(&page, json!({ "expires_at": 4_000_000_000_000_i64 }))
        .await;
    assert_eq!(wiki.public(&format!("/s/{soon}")).await.status, 200);
    wiki.api
        .sql(&format!(
            "UPDATE page_shares SET expires_at = 1 WHERE token = '{soon}'"
        ))
        .await;
    assert_eq!(refusal(&wiki, &format!("/s/{soon}")).await, unknown);

    // The page in the trash, then the space.
    let token = wiki.share(&page, json!({})).await;
    let reply = wiki
        .api
        .raw(
            "DELETE",
            &format!("/pages/{page}"),
            &wiki.ada.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(refusal(&wiki, &format!("/s/{token}")).await, unknown);
    let reply = wiki
        .api
        .raw(
            "POST",
            &format!("/pages/{page}/restore"),
            &wiki.ada.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(wiki.public(&format!("/s/{token}")).await.status, 200);
    let reply = wiki
        .api
        .raw(
            "DELETE",
            &format!("/spaces/{}", wiki.space),
            &wiki.ada.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(refusal(&wiki, &format!("/s/{token}")).await, unknown);
}

/// Step 58: a share answers only while its creator may still manage it (ADR 0030).
#[tokio::test]
async fn a_share_closes_when_its_creator_loses_the_editor_role() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n", None).await;
    let unknown = refusal(&wiki, "/s/00000000000000000000000000000000").await;
    let reply = wiki.create(&wiki.bob, &page, json!({})).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let token = reply.body["token"].as_str().unwrap().to_owned();
    assert_eq!(wiki.public(&format!("/s/{token}")).await.status, 200);

    let path = format!("/spaces/{}/grants", wiki.space);
    let viewer = json!({ "grants": [{ "user_id": BOB, "role": "viewer" }] });
    let reply = wiki
        .api
        .send("PUT", &path, &wiki.ada.headers(), viewer)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(refusal(&wiki, &format!("/s/{token}")).await, unknown);

    // The share follows the creator's rights of now: given back, they reopen it.
    let editor = json!({ "grants": [{ "user_id": BOB, "role": "editor" }] });
    let reply = wiki
        .api
        .send("PUT", &path, &wiki.ada.headers(), editor)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(wiki.public(&format!("/s/{token}")).await.status, 200);
}

#[tokio::test]
async fn a_share_closes_when_its_creator_leaves_the_workspace() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n", None).await;
    let unknown = refusal(&wiki, "/s/00000000000000000000000000000000").await;
    let reply = wiki.create(&wiki.bob, &page, json!({})).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let token = reply.body["token"].as_str().unwrap().to_owned();
    let kept = wiki.share(&page, json!({})).await;
    assert_eq!(wiki.public(&format!("/s/{token}")).await.status, 200);

    let workspace = wiki.api.first_workspace().await;
    let path = format!("/workspaces/{workspace}/members/{BOB}");
    let reply = wiki
        .api
        .raw("DELETE", &path, &wiki.ada.headers(), b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(refusal(&wiki, &format!("/s/{token}")).await, unknown);
    // Another editor's share of the page stays open.
    assert_eq!(wiki.public(&format!("/s/{kept}")).await.status, 200);
}

#[tokio::test]
async fn an_expired_share_leaves_the_list() {
    let wiki = wiki().await;
    let page = wiki.page("# Guide\n", None).await;
    let live = wiki.share(&page, json!({})).await;
    let soon = wiki
        .share(&page, json!({ "expires_at": 4_000_000_000_000_i64 }))
        .await;
    wiki.api
        .sql(&format!(
            "UPDATE page_shares SET expires_at = 1 WHERE token = '{soon}'"
        ))
        .await;
    let list = wiki
        .api
        .get(&wiki.ada, &format!("/pages/{page}/shares"))
        .await;
    assert_eq!(list.status, 200, "{list:?}");
    let items = list.body["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "{items:?}");
    assert_eq!(items[0]["token"], live);
}

#[tokio::test]
async fn public_routes_are_rate_limited_per_client() {
    let wiki = wiki().await;
    let mut limited = None;
    for count in 0..1_000 {
        let reply = wiki.public("/s/00000000000000000000000000000000").await;
        if reply.status == 429 {
            limited = Some((count, reply));
            break;
        }
        assert_eq!(reply.status, 404, "{reply:?}");
    }
    let (count, reply) = limited.expect("a limit");
    assert!((60..=200).contains(&count), "{count} requests passed");
    assert!(reply.header("retry-after").is_some());
    // Files count too.
    let reply = wiki
        .public("/s/00000000000000000000000000000000/files/x")
        .await;
    assert_eq!(reply.status, 429, "{reply:?}");
    // The API keeps its own limits.
    let reply = wiki.api.session(&wiki.ada).await;
    assert_eq!(reply.status, 200, "{reply:?}");
}
