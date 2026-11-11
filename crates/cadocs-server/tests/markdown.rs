//! Markdown export and import of one page, over REST: a file with absolute links, or a zip with
//! the attachments, which imports back as a new page with copies of them.

// async_zip 0.0.19 deprecates its readers for a `read1` module it does not ship yet.
#![allow(deprecated)]

mod common;

use std::collections::HashMap;

use async_zip::base::read::mem::ZipFileReader;
use async_zip::base::write::ZipFileWriter;
use async_zip::{Compression, ZipEntryBuilder};
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
