//! Markdown export and import of one page, over REST: a file with absolute links, or a zip with
//! the attachments, which imports back as a new page with copies of them.

// async_zip 0.0.19 deprecates its readers for a `read1` module it does not ship yet.
#![allow(deprecated)]

mod common;

use std::collections::HashMap;

use async_zip::base::read::mem::ZipFileReader;
use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, StringEncoding, ZipEntryBuilder, ZipString};
use common::{Api, Login, ORIGIN, Reply, start_tuned};
use serde_json::json;
use tokio::time::{Duration, Instant, sleep};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const VIC: &str = "33333333-0000-7000-8000-000000000001";
const EVE: &str = "44444444-0000-7000-8000-000000000001";
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";
const PDF: &[u8] = b"%PDF-1.4 a report";

struct Wiki {
    api: Api,
    ada: Login,
    /// An editor of the space.
    bob: Login,
    /// A viewer of the space.
    vic: Login,
    /// A member who cannot read the space.
    eve: Login,
    space: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

async fn wiki(settings: &[(&str, &str)]) -> Wiki {
    let api = start_tuned(ORIGIN, "", settings).await;
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
    let eve = api
        .add_member(&workspace, EVE, "eve@example.org", "member")
        .await;
    let path = format!("/workspaces/{workspace}/spaces");
    let reply = api
        .send("POST", &path, &ada.headers(), json!({ "name": "Handbook" }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let space = id(&reply);
    let grants = json!({ "grants": [
        { "user_id": BOB, "role": "editor" },
        { "user_id": VIC, "role": "viewer" },
    ] });
    let path = format!("/spaces/{space}/grants");
    let reply = api.send("PUT", &path, &ada.headers(), grants).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    Wiki {
        api,
        ada,
        bob,
        vic,
        eve,
        space,
    }
}

impl Wiki {
    async fn new_page(&self) -> String {
        let path = format!("/spaces/{}/pages", self.space);
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), json!({}))
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

    async fn import(&self, by: &Login, query: &str, kind: &str, body: &[u8]) -> Reply {
        let path = format!("/spaces/{}/import?{query}", self.space);
        let mut headers = by.headers().to_vec();
        headers.push(("content-type", kind));
        self.api.raw("POST", &path, &headers, body).await
    }

    /// A page imported by `ada` from Markdown, by id.
    async fn imported(&self, markdown: &str) -> String {
        let reply = self
            .import(
                &self.ada,
                "name=notes.md",
                "text/markdown",
                markdown.as_bytes(),
            )
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn export(&self, by: &Login, page: &str, query: &str) -> Reply {
        self.api
            .get(by, &format!("/pages/{page}/export?{query}"))
            .await
    }

    async fn markdown(&self, page: &str) -> String {
        let reply = self.export(&self.ada, page, "format=md").await;
        assert_eq!(reply.status, 200, "{reply:?}");
        String::from_utf8(reply.bytes).unwrap()
    }
}

/// The entries of a zip, by name.
async fn unzip(bytes: Vec<u8>) -> HashMap<String, Vec<u8>> {
    let reader = ZipFileReader::new(bytes).await.unwrap();
    let mut entries = HashMap::new();
    for index in 0..reader.file().entries().len() {
        let entry = &reader.file().entries()[index];
        let name = entry.filename().as_str().unwrap().to_owned();
        let mut content = Vec::new();
        let mut file = reader.reader_with_entry(index).await.unwrap();
        file.read_to_end_checked(&mut content).await.unwrap();
        entries.insert(name, content);
    }
    entries
}

async fn zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipFileWriter::new(Vec::new());
    for (name, content) in entries {
        let builder = ZipEntryBuilder::new((*name).into(), Compression::Stored);
        writer.write_entry_whole(builder, content).await.unwrap();
    }
    writer.close().await.unwrap()
}

#[tokio::test]
async fn an_export_imports_back_with_its_content_and_attachments() {
    let wiki = wiki(&[]).await;
    let holder = wiki.new_page().await;
    let image = wiki.attach(&holder, "chart.png", "image/png", PNG).await;
    let file = wiki
        .attach(&holder, "Q3%20report.pdf", "application/pdf", PDF)
        .await;
    let source = format!(
        "# Quarterly review\n\nNumbers went **up**.\n\n![the chart](/api/v1/attachments/{image})\n\n\
         [Q3 report.pdf](/api/v1/attachments/{file})\n"
    );
    let page = wiki.imported(&source).await;

    let reply = wiki
        .export(&wiki.vic, &page, "format=md&attachments=1")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("content-type"), Some("application/zip"));
    let disposition = reply.header("content-disposition").unwrap();
    assert!(
        disposition.contains("Quarterly review.zip"),
        "{disposition}"
    );
    let entries = unzip(reply.bytes.clone()).await;
    let markdown = String::from_utf8(entries["Quarterly review.md"].clone()).unwrap();
    assert_eq!(
        markdown,
        format!(
            "# Quarterly review\n\nNumbers went **up**.\n\n\
             ![the chart](attachments/{image}/chart.png)\n\n\
             [Q3 report.pdf](attachments/{file}/Q3%20report.pdf)\n"
        )
    );
    assert_eq!(entries[&format!("attachments/{image}/chart.png")], PNG);
    assert_eq!(entries[&format!("attachments/{file}/Q3 report.pdf")], PDF);

    let reply = wiki
        .import(
            &wiki.bob,
            "name=export.zip",
            "application/zip",
            &reply.bytes,
        )
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["title"], "Quarterly review");
    let copy = id(&reply);
    assert_ne!(copy, page);

    // The copy has the same content, with attachments of its own that hold the same bytes.
    let exported = wiki.markdown(&copy).await;
    let ids: Vec<String> = exported
        .split(&format!("{ORIGIN}/api/v1/attachments/"))
        .skip(1)
        .map(|rest| rest[..36].to_owned())
        .collect();
    assert_eq!(ids.len(), 2, "{exported}");
    assert!(!ids.contains(&image) && !ids.contains(&file), "{exported}");
    assert_eq!(
        exported,
        format!(
            "# Quarterly review\n\nNumbers went **up**.\n\n\
             ![the chart]({ORIGIN}/api/v1/attachments/{})\n\n\
             [Q3 report.pdf]({ORIGIN}/api/v1/attachments/{})\n",
            ids[0], ids[1]
        )
    );
    let reply = wiki
        .api
        .get(&wiki.vic, &format!("/attachments/{}", ids[0]))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.bytes, PNG);
    assert_eq!(reply.header("content-type"), Some("image/png"));
    let reply = wiki
        .api
        .get(&wiki.vic, &format!("/attachments/{}", ids[1]))
        .await;
    assert_eq!(reply.bytes, PDF);
    assert_eq!(reply.header("content-type"), Some("application/pdf"));
    assert!(
        reply
            .header("content-disposition")
            .unwrap()
            .contains("Q3%20report.pdf")
    );
}

#[tokio::test]
async fn a_markdown_export_is_one_file_with_absolute_links() {
    let wiki = wiki(&[("CADOCS_FLUSH_INTERVAL_MS", "50")]).await;
    let other = wiki.imported("# Other page\n").await;
    let page = wiki
        .imported(&format!(
            "# Guide\n\nSee [x](/?page={other}).\n\n# A heading\n"
        ))
        .await;
    // A link shows the title of its page, which `pages` holds from the next flush.
    let deadline = Instant::now() + Duration::from_secs(5);
    while wiki
        .api
        .number(&format!(
            "SELECT COUNT(*) FROM pages WHERE id = '{other}' AND title = 'Other page'"
        ))
        .await
        == 0
    {
        assert!(Instant::now() < deadline, "the title was never flushed");
        sleep(Duration::from_millis(20)).await;
    }
    let reply = wiki.export(&wiki.vic, &page, "").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        reply.header("content-type"),
        Some("text/markdown; charset=utf-8")
    );
    assert!(
        reply
            .header("content-disposition")
            .unwrap()
            .starts_with("attachment; filename=\"Guide.md\"")
    );
    assert_eq!(
        String::from_utf8(reply.bytes).unwrap(),
        format!("# Guide\n\nSee [Other page]({ORIGIN}/?page={other}).\n\n# A heading\n")
    );
}

#[tokio::test]
async fn an_import_takes_its_title_from_a_first_heading_or_the_file_name() {
    let wiki = wiki(&[]).await;
    let reply = wiki
        .import(
            &wiki.ada,
            "name=Meeting%20notes.md",
            "text/markdown",
            b"Just text.\n",
        )
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["title"], "Meeting notes");
    let page = id(&reply);
    assert_eq!(
        wiki.markdown(&page).await,
        "# Meeting notes\n\nJust text.\n"
    );

    // Under a parent, last among its children.
    let parent = wiki.imported("# Parent\n").await;
    let reply = wiki
        .import(
            &wiki.ada,
            &format!("parent_id={parent}"),
            "text/markdown",
            b"## Not a title\n",
        )
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["title"], "");
    assert_eq!(reply.body["parent_id"], parent.as_str());
    let reply = wiki
        .import(&wiki.ada, "parent_id=nope", "text/markdown", b"x")
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["parent_id"], "unknown");
}

#[tokio::test]
async fn roles_decide_who_exports_and_who_imports() {
    let wiki = wiki(&[]).await;
    let page = wiki.imported("# Secret\n").await;
    assert_eq!(wiki.export(&wiki.vic, &page, "").await.status, 200);
    let reply = wiki.export(&wiki.eve, &page, "").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = wiki.export(&wiki.eve, &page, "attachments=1").await;
    assert_eq!(reply.status, 404, "{reply:?}");

    let reply = wiki.import(&wiki.vic, "", "text/markdown", b"x").await;
    assert_eq!(reply.status, 403, "{reply:?}");
    let reply = wiki.import(&wiki.eve, "", "text/markdown", b"x").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = wiki.import(&wiki.bob, "", "text/markdown", b"x").await;
    assert_eq!(reply.status, 201, "{reply:?}");
}

#[tokio::test]
async fn an_export_leaves_out_attachments_the_reader_cannot_download() {
    let wiki = wiki(&[]).await;
    // Only `ada` reads this space.
    let workspace = wiki.api.first_workspace().await;
    let path = format!("/workspaces/{workspace}/spaces");
    let reply = wiki
        .api
        .send(
            "POST",
            &path,
            &wiki.ada.headers(),
            json!({ "name": "Private" }),
        )
        .await;
    let private = id(&reply);
    let path = format!("/spaces/{private}/pages");
    let reply = wiki
        .api
        .send("POST", &path, &wiki.ada.headers(), json!({}))
        .await;
    let hidden = wiki
        .attach(&id(&reply), "hidden.png", "image/png", PNG)
        .await;
    let page = wiki
        .imported(&format!("![x](/api/v1/attachments/{hidden})\n"))
        .await;

    let reply = wiki.export(&wiki.vic, &page, "attachments=1").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let entries = unzip(reply.bytes).await;
    assert_eq!(entries.len(), 1, "{:?}", entries.keys());
    let markdown = String::from_utf8(entries["notes.md"].clone()).unwrap();
    assert_eq!(
        markdown,
        format!("# notes\n\n![x]({ORIGIN}/api/v1/attachments/{hidden})\n")
    );
}

#[tokio::test]
async fn an_import_refuses_what_it_cannot_read() {
    let wiki = wiki(&[("CADOCS_MAX_IMPORT_MB", "1")]).await;
    let bad = |reply: &Reply, problem: &str| {
        assert_eq!(reply.status, 400, "{reply:?}");
        assert_eq!(reply.body["error"]["fields"]["file"], problem, "{reply:?}");
    };
    let reply = wiki
        .import(&wiki.ada, "", "text/markdown", b"\xff\xfe")
        .await;
    bad(&reply, "not_utf8");
    let reply = wiki
        .import(&wiki.ada, "", "application/zip", b"not a zip")
        .await;
    bad(&reply, "invalid");
    let none = zip(&[("readme.txt", b"hello")]).await;
    let reply = wiki.import(&wiki.ada, "", "application/zip", &none).await;
    bad(&reply, "no_markdown");
    let two = zip(&[("a.md", b"a"), ("b.md", b"b")]).await;
    let reply = wiki.import(&wiki.ada, "", "application/zip", &two).await;
    bad(&reply, "several_markdown");
    let names: Vec<String> = (0..=1000).map(|n| format!("f{n}")).collect();
    let many: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    let many = zip(&many).await;
    let reply = wiki.import(&wiki.ada, "", "application/zip", &many).await;
    bad(&reply, "too_many_entries");

    let big = vec![b'a'; 1024 * 1024 + 1];
    let reply = wiki.import(&wiki.ada, "", "application/zip", &big).await;
    assert_eq!(reply.status, 413, "{reply:?}");
    let reply = wiki.import(&wiki.ada, "", "text/markdown", &big).await;
    assert_eq!(reply.status, 413, "{reply:?}");

    // Nothing was made.
    let pages = wiki
        .api
        .number("SELECT COUNT(*) FROM pages WHERE deleted_at IS NULL")
        .await;
    assert_eq!(pages, 0);
}

#[tokio::test]
async fn an_import_over_the_quota_leaves_no_page() {
    let wiki = wiki(&[("CADOCS_WORKSPACE_QUOTA_MB", "1")]).await;
    let big = vec![7u8; 600 * 1024];
    let markdown = "![a](attachments/a/a.png)\n\n![b](attachments/b/b.png)\n";
    let archive = zip(&[
        ("page.md", markdown.as_bytes()),
        ("attachments/a/a.png", &big),
        ("attachments/b/b.png", &big),
    ])
    .await;
    let reply = wiki
        .import(&wiki.ada, "", "application/zip", &archive)
        .await;
    assert_eq!(reply.status, 413, "{reply:?}");
    let live = wiki
        .api
        .number("SELECT COUNT(*) FROM pages WHERE deleted_at IS NULL")
        .await;
    assert_eq!(live, 0);
}

/// Points the last central directory record at the data of the one before, with its size, so
/// two entries share one copy of the bytes.
fn overlap(mut archive: Vec<u8>) -> Vec<u8> {
    let records: Vec<usize> = archive
        .windows(4)
        .enumerate()
        .filter(|(_, w)| *w == [0x50, 0x4b, 0x01, 0x02])
        .map(|(at, _)| at)
        .collect();
    let (first, last) = (records[records.len() - 2], records[records.len() - 1]);
    let copied: Vec<u8> = archive[first + 20..first + 28].to_vec();
    archive[last + 20..last + 28].copy_from_slice(&copied);
    let offset: Vec<u8> = archive[first + 42..first + 46].to_vec();
    archive[last + 42..last + 46].copy_from_slice(&offset);
    archive
}

#[tokio::test]
async fn entries_sharing_their_bytes_are_refused() {
    let wiki = wiki(&[]).await;
    let big = vec![7u8; 300 * 1024];
    let markdown = "![a](attachments/a/a.png)\n\n![b](attachments/b/b.png)\n";
    let archive = zip(&[
        ("page.md", markdown.as_bytes()),
        ("attachments/a/a.png", &big),
        ("attachments/b/b.png", b"x"),
    ])
    .await;
    let reply = wiki
        .import(&wiki.ada, "", "application/zip", &overlap(archive))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["file"], "invalid");
    let stored = wiki.api.number("SELECT COUNT(*) FROM attachments").await;
    assert_eq!(stored, 0);
}

#[tokio::test]
async fn an_empty_first_heading_leaves_the_title_to_the_file_name() {
    let wiki = wiki(&[]).await;
    let reply = wiki
        .import(&wiki.ada, "name=Plan.md", "text/markdown", b"#\n\nText.\n")
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["title"], "Plan");
}

#[tokio::test]
async fn an_html_export_is_one_escaped_file_or_a_zip_with_its_attachments() {
    let wiki = wiki(&[("CADOCS_FLUSH_INTERVAL_MS", "50")]).await;
    let page = wiki
        .imported("# Guide\n\nSome **bold** text and <script>alert(1)</script>.\n")
        .await;
    let reply = wiki.export(&wiki.vic, &page, "format=html").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(
        reply.header("content-type"),
        Some("text/html; charset=utf-8")
    );
    assert!(
        reply
            .header("content-disposition")
            .unwrap()
            .starts_with("attachment; filename=\"Guide.html\"")
    );
    let html = String::from_utf8(reply.bytes).unwrap();
    assert!(html.starts_with("<!doctype html>"), "{html}");
    assert!(html.contains("<title>Guide</title>"), "{html}");
    assert!(html.contains("<h1>Guide</h1>"), "{html}");
    assert!(html.contains("<strong>bold</strong>"), "{html}");
    assert!(!html.contains("<script>"), "{html}");

    let image = wiki.attach(&page, "a.png", "image/png", b"PNG").await;
    let content = format!("# Guide\n\n![a](/api/v1/attachments/{image})\n");
    let reply = wiki
        .import(&wiki.ada, "name=g.md", "text/markdown", content.as_bytes())
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let page = id(&reply);
    let reply = wiki
        .export(&wiki.ada, &page, "format=html&attachments=1")
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("content-type"), Some("application/zip"));
    let entries = unzip(reply.bytes).await;
    let html = String::from_utf8(entries["Guide.html"].clone()).unwrap();
    assert!(
        html.contains(&format!("src=\"attachments/{image}/a.png\"")),
        "{html}"
    );

    let reply = wiki.export(&wiki.ada, &page, "format=pdf").await;
    assert_eq!(reply.status, 400, "{reply:?}");
}

impl Wiki {
    /// Starts a space export and waits for it to end: the job's final status.
    async fn space_export(&self, by: &Login, query: &str) -> (String, Reply) {
        let path = format!("/spaces/{}/export?{query}", self.space);
        let reply = self.api.raw("POST", &path, &by.headers(), b"").await;
        assert_eq!(reply.status, 202, "{reply:?}");
        let job = id(&reply);
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let reply = self.api.get(by, &format!("/jobs/{job}")).await;
            assert_eq!(reply.status, 200, "{reply:?}");
            if reply.body["status"] != "running" {
                return (job, reply);
            }
            assert!(Instant::now() < deadline, "the export never ended");
            sleep(Duration::from_millis(20)).await;
        }
    }
}

#[tokio::test]
async fn a_space_export_is_a_zip_of_folders_and_pages_only_its_starter_downloads() {
    let wiki = wiki(&[("CADOCS_FLUSH_INTERVAL_MS", "50")]).await;
    let image = wiki
        .attach(&wiki.new_page().await, "a.png", "image/png", PNG)
        .await;
    let guide = wiki
        .imported(&format!("# Guide\n\n![a](/api/v1/attachments/{image})\n"))
        .await;
    let query = format!("name=c.md&parent_id={guide}");
    let reply = wiki
        .import(
            &wiki.ada,
            &query,
            "text/markdown",
            b"# Child\n\nHello **you**.\n",
        )
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    // The same title twice at the root, so the second one is told apart.
    wiki.imported("# Guide\n\nOther.\n").await;
    wiki.titles_stored(3).await;

    let (job, status) = wiki.space_export(&wiki.vic, "format=md").await;
    assert_eq!(status.body["status"], "done", "{status:?}");
    let file = format!("/jobs/{job}/file");
    let reply = wiki.api.get(&wiki.vic, &file).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.header("content-type"), Some("application/zip"));
    assert!(
        reply
            .header("content-disposition")
            .unwrap()
            .starts_with("attachment; filename=\"Handbook.zip\"")
    );
    let entries = unzip(reply.bytes).await;
    let mut names: Vec<_> = entries
        .keys()
        .filter(|name| !name.starts_with("attachments/"))
        .collect();
    names.sort();
    assert_eq!(
        names,
        ["Guide (2).md", "Guide.md", "Guide/Child.md", "Untitled.md"],
        "{:?}",
        entries.keys()
    );
    let child = String::from_utf8(entries["Guide/Child.md"].clone()).unwrap();
    assert_eq!(child, "# Child\n\nHello **you**.\n");
    let guide = String::from_utf8(entries["Guide.md"].clone()).unwrap();
    assert!(
        guide.contains(&format!("(attachments/{image}/a.png)")),
        "{guide}"
    );
    assert_eq!(entries[&format!("attachments/{image}/a.png")], PNG);

    // Only whoever started it sees it, and only while they can read the space.
    for other in [&wiki.bob, &wiki.eve] {
        assert_eq!(
            wiki.api.get(other, &format!("/jobs/{job}")).await.status,
            404
        );
        assert_eq!(wiki.api.get(other, &file).await.status, 404);
    }
    let path = format!("/spaces/{}/export?format=md", wiki.space);
    let reply = wiki.api.raw("POST", &path, &wiki.eve.headers(), b"").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    let reply = wiki
        .api
        .raw("POST", &format!("{path}x"), &wiki.vic.headers(), b"")
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");

    let (job, _) = wiki.space_export(&wiki.vic, "format=html").await;
    let reply = wiki.api.get(&wiki.vic, &format!("/jobs/{job}/file")).await;
    let entries = unzip(reply.bytes).await;
    let html = String::from_utf8(entries["Guide/Child.html"].clone()).unwrap();
    assert!(
        html.contains("<h1>Child</h1>") && html.contains("<strong>you</strong>"),
        "{html}"
    );

    // Held jobs count until an hour after their end: a third one of the same user is refused.
    let path = format!("/spaces/{}/export", wiki.space);
    let reply = wiki.api.raw("POST", &path, &wiki.vic.headers(), b"").await;
    assert_eq!(reply.status, 429, "{reply:?}");
}

impl Wiki {
    /// Starts a bulk import and waits for its job to end.
    async fn bulk(&self, query: &str, body: &[u8]) -> Reply {
        let path = format!("/spaces/{}/bulk-import?{query}", self.space);
        let mut headers = self.bob.headers().to_vec();
        headers.push(("content-type", "application/zip"));
        let reply = self.api.raw("POST", &path, &headers, body).await;
        assert_eq!(reply.status, 202, "{reply:?}");
        assert_eq!(reply.body["kind"], "space_import");
        let job = format!("/jobs/{}", id(&reply));
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let reply = self.api.get(&self.bob, &job).await;
            assert_eq!(reply.status, 200, "{reply:?}");
            if reply.body["status"] != "running" {
                return reply;
            }
            assert!(Instant::now() < deadline, "the import never ended");
            sleep(Duration::from_millis(20)).await;
        }
    }

    /// The pages under `parent`, or at the root of the space, in order: id and exported Markdown.
    async fn children(&self, parent: Option<&str>) -> Vec<(String, String)> {
        let query = parent.map_or(String::new(), |id| format!("?parent={id}"));
        let path = format!("/spaces/{}/tree{query}", self.space);
        let reply = self.api.get(&self.ada, &path).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        let mut pages = Vec::new();
        for item in reply.body["items"].as_array().unwrap() {
            let id = item["id"].as_str().unwrap().to_owned();
            let markdown = self.markdown(&id).await;
            pages.push((id, markdown));
        }
        pages
    }

    async fn live_pages(&self) -> i64 {
        self.api
            .number("SELECT COUNT(*) FROM pages WHERE deleted_at IS NULL")
            .await
    }

    /// Waits for a flush to store `count` titles, which name the files of a space export.
    async fn titles_stored(&self, count: i64) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while self
            .api
            .number("SELECT COUNT(*) FROM pages WHERE title <> ''")
            .await
            < count
        {
            assert!(Instant::now() < deadline, "the titles were never stored");
            sleep(Duration::from_millis(20)).await;
        }
    }
}

#[tokio::test]
async fn a_bulk_import_makes_a_tree_of_pages_with_their_attachments() {
    let wiki = wiki(&[]).await;
    let body = zip(&[
        (
            "Guide.md",
            b"# The guide\n\n![a](attachments/x/a%20b.png)\n\n[child](Guide/Child.md#top)\n",
        ),
        (
            "Guide/Child.md",
            b"![a](../attachments/x/a%20b.png)\n\n[back](../Guide.md)\n",
        ),
        ("notes/Deep.markdown", b"Deep.\n"),
        ("attachments/x/a b.png", PNG),
        (
            "attachments/01890000-0000-7000-8000-000000000001/attached.md",
            b"not a page",
        ),
        ("__MACOSX/Guide.md", b"junk"),
        (".obsidian/x.md", b"junk"),
    ])
    .await;
    let status = wiki.bulk("", &body).await;
    assert_eq!(status.body["status"], "done", "{status:?}");
    assert_eq!(status.body["pages"], 4);
    let file = format!("/jobs/{}/file", id(&status));
    assert_eq!(wiki.api.get(&wiki.bob, &file).await.status, 404);

    let root = wiki.children(None).await;
    assert_eq!(root.len(), 2, "{root:?}");
    let (guide, text) = &root[0];
    let under_guide = wiki.children(Some(guide)).await;
    let (child, child_text) = &under_guide[0];
    let image = text
        .split(&format!("{ORIGIN}/api/v1/attachments/"))
        .nth(1)
        .unwrap()[..36]
        .to_owned();
    assert!(text.starts_with("# The guide\n\n![a]("), "{text}");
    assert!(text.contains(&format!("{ORIGIN}/?page={child}")), "{text}");
    // The same file, stored once, and a link back by page id.
    assert!(child_text.starts_with("# Child\n"), "{child_text}");
    assert!(child_text.contains(&image), "{child_text}");
    assert!(
        child_text.contains(&format!("{ORIGIN}/?page={guide}")),
        "{child_text}"
    );
    let stored = wiki.api.number("SELECT COUNT(*) FROM attachments").await;
    assert_eq!(stored, 1);
    let reply = wiki
        .api
        .get(&wiki.vic, &format!("/attachments/{image}"))
        .await;
    assert_eq!(reply.bytes, PNG);

    // A folder without a Markdown file is an empty page named after it.
    let (notes, text) = &root[1];
    assert_eq!(text, "# notes\n");
    assert_eq!(wiki.children(Some(notes)).await[0].1, "# Deep\n\nDeep.\n");

    // Under a parent.
    let body = zip(&[("Sub.md", b"x")]).await;
    let status = wiki.bulk(&format!("parent_id={notes}"), &body).await;
    assert_eq!(status.body["status"], "done", "{status:?}");
    assert_eq!(wiki.children(Some(notes)).await.len(), 2);
}

#[tokio::test]
async fn a_bulk_import_refuses_or_fails_whole() {
    let wiki = wiki(&[("CADOCS_MAX_UPLOAD_MB", "1")]).await;
    let send = async |by: &Login, query: &str, body: &[u8]| {
        let path = format!("/spaces/{}/bulk-import?{query}", wiki.space);
        let mut headers = by.headers().to_vec();
        headers.push(("content-type", "application/zip"));
        wiki.api.raw("POST", &path, &headers, body).await
    };
    let one = zip(&[("a.md", b"a")]).await;
    assert_eq!(send(&wiki.vic, "", &one).await.status, 403);
    assert_eq!(send(&wiki.eve, "", &one).await.status, 404);
    let reply = send(&wiki.bob, "parent_id=nope", &one).await;
    assert_eq!(reply.body["error"]["fields"]["parent_id"], "unknown");

    let bad = |reply: Reply, problem: &str| {
        assert_eq!(reply.status, 400, "{reply:?}");
        assert_eq!(reply.body["error"]["fields"]["file"], problem, "{reply:?}");
    };
    bad(send(&wiki.bob, "", b"not a zip").await, "invalid");
    let none = zip(&[("readme.txt", b"x")]).await;
    bad(send(&wiki.bob, "", &none).await, "no_markdown");
    let deep = format!("{}a.md", "d/".repeat(100));
    let deep = zip(&[(deep.as_str(), b"x")]).await;
    bad(send(&wiki.bob, "", &deep).await, "too_deep");
    let names: Vec<String> = (0..=1000).map(|n| format!("p{n}.md")).collect();
    let many: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), &b""[..])).collect();
    let many = zip(&many).await;
    bad(send(&wiki.bob, "", &many).await, "too_many_pages");
    let big = vec![b'a'; 2 * 1024 * 1024 + 1];
    let big_page = zip(&[("a.md", &big)]).await;
    assert_eq!(send(&wiki.bob, "", &big_page).await.status, 413);
    assert_eq!(wiki.live_pages().await, 0);

    // What only shows once the pages exist fails the job, and leaves none of them.
    let not_utf8 = zip(&[("a.md", b"a"), ("b.md", b"\xff")]).await;
    let status = wiki.bulk("", &not_utf8).await;
    assert_eq!(status.body["status"], "failed", "{status:?}");
    let message = status.body["message"].as_str().unwrap();
    assert!(message.contains("b.md"), "{message}");
    assert_eq!(wiki.live_pages().await, 0);
    let heavy = zip(&[("a.md", b"[f](f.bin)"), ("f.bin", &big)]).await;
    let status = wiki.bulk("", &heavy).await;
    assert_eq!(status.body["status"], "failed", "{status:?}");
    assert_eq!(wiki.live_pages().await, 0);
}

#[tokio::test]
async fn an_ended_import_takes_no_slot_and_only_export_files_are_not_pages() {
    let wiki = wiki(&[]).await;
    // A page named `attachments` and its folder are the user's: only `attachments/<id>/` is not.
    let body = zip(&[("attachments.md", b"a"), ("attachments/Mine.md", b"b")]).await;
    // More than the jobs one user may hold.
    for _ in 0..3 {
        let status = wiki.bulk("", &body).await;
        assert_eq!(status.body["status"], "done", "{status:?}");
        assert_eq!(status.body["pages"], 2);
    }
    let failing = zip(&[("a.md", b"\xff")]).await;
    for _ in 0..3 {
        assert_eq!(wiki.bulk("", &failing).await.body["status"], "failed");
    }

    let parent = wiki.imported("# Gone\n").await;
    let path = format!("/pages/{parent}");
    let reply = wiki
        .api
        .send("DELETE", &path, &wiki.ada.headers(), json!({}))
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let path = format!("/spaces/{}/bulk-import?parent_id={parent}", wiki.space);
    let reply = wiki
        .api
        .raw("POST", &path, &wiki.bob.headers(), &body)
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["parent_id"], "unknown");
}

#[tokio::test]
async fn ended_jobs_and_their_messages_stay_bounded() {
    let wiki = wiki(&[]).await;
    let name = format!("{}.md", "n".repeat(2000));
    let failing = zip(&[(name.as_str(), b"\xff")]).await;
    let first = wiki.bulk("", &failing).await;
    let message = first.body["message"].as_str().unwrap();
    assert!(message.chars().count() <= 300, "{message}");
    for _ in 0..8 {
        wiki.bulk("", &failing).await;
    }
    // The oldest ended jobs of a user make room for the new ones.
    let path = format!("/jobs/{}", id(&first));
    assert_eq!(wiki.api.get(&wiki.bob, &path).await.status, 404);
}

/// A zip as desktop tools make it: deflated, with UTF-8 names that no flag declares.
async fn deflated(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = ZipFileWriter::new(Vec::new());
    for (name, content) in entries {
        let name = ZipString::new(name.as_bytes().to_vec(), StringEncoding::Raw);
        let builder = ZipEntryBuilder::new(name, Compression::Deflate);
        writer.write_entry_whole(builder, content).await.unwrap();
    }
    writer.close().await.unwrap()
}

#[tokio::test]
async fn a_compressed_zip_imports_as_one_page_or_as_a_tree() {
    let wiki = wiki(&[]).await;
    let text = format!("![a](r\u{e9}sum\u{e9}.png)\n\n{}\n", "word ".repeat(500));
    let one = deflated(&[
        ("Caf\u{e9}.md", text.as_bytes()),
        ("r\u{e9}sum\u{e9}.png", PNG),
    ])
    .await;
    assert!(one.len() < text.len(), "the zip was not compressed");
    let reply = wiki.import(&wiki.ada, "", "application/zip", &one).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let markdown = wiki.markdown(&id(&reply)).await;
    assert!(markdown.contains("word word"), "{markdown}");
    let image = &markdown
        .split(&format!("{ORIGIN}/api/v1/attachments/"))
        .nth(1)
        .unwrap()[..36];
    let reply = wiki
        .api
        .get(&wiki.ada, &format!("/attachments/{image}"))
        .await;
    assert_eq!(reply.bytes, PNG);

    let status = wiki.bulk("", &one).await;
    assert_eq!(status.body["status"], "done", "{status:?}");
    let root = wiki.children(None).await;
    assert!(root[1].1.starts_with("# Caf\u{e9}\n"), "{}", root[1].1);
}

#[tokio::test]
async fn what_a_zip_unpacks_to_counts_against_the_import_limit() {
    let wiki = wiki(&[("CADOCS_MAX_IMPORT_MB", "1")]).await;
    // Far under the limit as sent, over it once unpacked.
    let zeros = vec![0; 600 * 1024];
    let bomb = deflated(&[
        ("a.md", b"[b](b.bin) [c](c.bin)"),
        ("b.bin", &zeros),
        ("c.bin", &zeros),
    ])
    .await;
    assert!(bomb.len() < 16 * 1024);
    let reply = wiki.import(&wiki.ada, "", "application/zip", &bomb).await;
    assert_eq!(reply.status, 413, "{reply:?}");
    let path = format!("/spaces/{}/bulk-import", wiki.space);
    let reply = wiki
        .api
        .raw("POST", &path, &wiki.ada.headers(), &bomb)
        .await;
    assert_eq!(reply.status, 413, "{reply:?}");
    assert_eq!(wiki.live_pages().await, 0);
}

#[tokio::test]
async fn a_page_that_unpacks_to_more_than_it_declares_is_refused() {
    let wiki = wiki(&[]).await;
    let long = vec![b'a'; 64 * 1024];
    let mut lying = deflated(&[("a.md", &long)]).await;
    // The unpacked size in the directory record.
    let record = lying.windows(4).rposition(|w| w == b"PK\x01\x02").unwrap();
    lying[record + 24..record + 28].copy_from_slice(&1u32.to_le_bytes());
    let reply = wiki.import(&wiki.ada, "", "application/zip", &lying).await;
    assert_eq!(reply.status, 400, "{reply:?}");
    assert_eq!(reply.body["error"]["fields"]["file"], "invalid");
    let status = wiki.bulk("", &lying).await;
    assert_eq!(status.body["status"], "failed", "{status:?}");
    assert_eq!(wiki.live_pages().await, 0);
}

#[tokio::test]
async fn backslashes_in_a_zip_separate_folders() {
    let wiki = wiki(&[]).await;
    let body = zip(&[("folder\\page.md", b"x")]).await;
    let status = wiki.bulk("", &body).await;
    assert_eq!(status.body["pages"], 2, "{status:?}");
    let root = wiki.children(None).await;
    assert_eq!(root[0].1, "# folder\n");
}

/// The page links of an exported file, as title and address.
fn page_links(text: &str, html: bool) -> Vec<(String, String)> {
    let (open, middle, close) = if html {
        (r#"<a class="page-link" href=""#, r#"">"#, "</a>")
    } else {
        ("[", "](", ")")
    };
    let mut links = Vec::new();
    for part in text.split(open).skip(1) {
        let (first, rest) = part.split_once(middle).unwrap();
        let (second, _) = rest.split_once(close).unwrap();
        links.push(if html {
            (second.to_owned(), first.to_owned())
        } else {
            (first.to_owned(), second.to_owned())
        });
    }
    links
}

#[tokio::test]
async fn a_space_export_links_its_pages_to_their_files() {
    let wiki = wiki(&[("CADOCS_FLUSH_INTERVAL_MS", "50")]).await;
    let body = zip(&[
        (
            "Guide.md",
            b"# Guide\n\n[x](Guide/Child%20one.md) [x](Notes.md) [x](Other.md)\n",
        ),
        (
            "Guide/Child one.md",
            b"# Child one\n\n[x](../Guide.md) [x](Child%20one/Leaf.md)\n",
        ),
        (
            "Guide/Child one/Leaf.md",
            b"# Leaf\n\n[x](../../Notes.md) [x](../../Guide.md) [x](../Child%20one.md)\n",
        ),
        ("Notes.md", b"# Notes\n\n[x](Guide/Child%20one/Leaf.md)\n"),
        // The same title as `Notes.md`, so one of them is written as `Notes (2)`.
        ("Other.md", b"# Notes\n\n[x](Notes.md) [x](Other.md)\n"),
    ])
    .await;
    let status = wiki.bulk("", &body).await;
    assert_eq!(status.body["pages"], 5, "{status:?}");
    wiki.titles_stored(5).await;

    for (format, html) in [("md", false), ("html", true)] {
        let (job, status) = wiki
            .space_export(&wiki.ada, &format!("format={format}"))
            .await;
        assert_eq!(status.body["status"], "done", "{status:?}");
        let reply = wiki.api.get(&wiki.ada, &format!("/jobs/{job}/file")).await;
        let entries = unzip(reply.bytes).await;
        let mut followed = 0;
        for (name, bytes) in &entries {
            let text = String::from_utf8(bytes.clone()).unwrap();
            let dir = name.rsplit_once('/').map_or("", |(dir, _)| dir);
            for (title, href) in page_links(&text, html) {
                // Relative to the file, each segment percent-encoded.
                let mut path: Vec<&str> = dir.split('/').filter(|s| !s.is_empty()).collect();
                let decoded = percent_encoding::percent_decode_str(&href)
                    .decode_utf8()
                    .unwrap()
                    .into_owned();
                for segment in decoded.split('/') {
                    if segment == ".." {
                        assert!(path.pop().is_some(), "{href} leaves the zip from {name}");
                    } else {
                        path.push(segment);
                    }
                }
                let target = path.join("/");
                let Some(linked) = entries.get(&target) else {
                    panic!("{href} in {name} leads to no entry: {:?}", entries.keys());
                };
                let linked = String::from_utf8(linked.clone()).unwrap();
                let heading = if html {
                    format!("<h1>{title}</h1>")
                } else {
                    format!("# {title}\n")
                };
                assert!(linked.contains(&heading), "{href} in {name}: {linked}");
                followed += 1;
            }
        }
        assert_eq!(followed, 11, "{format}: {:?}", entries.keys());
    }
}
