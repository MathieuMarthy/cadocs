//! Attachments over real sockets: the rules of `docs/DATA_MODEL.md#attachments`, the limits, and
//! a full disk.

mod common;

use std::io;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use cadocs_files::{
    BoxFuture, ByteStream, Download, FileBody, FileError, FileList, FileStore, LocalFileStore,
    Stored,
};
use cadocs_jobs::PURGE_ORPHAN_FILES;
use cadocs_store::now_ms;
use common::{Api, Login, Reply, logged_in, start_tuned, start_with_files};
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::time::{sleep, timeout};

const BOB: &str = "11111111-0000-7000-8000-000000000001";
const EVE: &str = "22222222-0000-7000-8000-000000000001";
const MIB: usize = 1024 * 1024;

struct Wiki {
    api: Api,
    ada: Login,
    /// A member, with whatever the default role of a space gives.
    bob: Login,
    /// Not a member.
    eve: Login,
    workspace: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

async fn wiki_on(api: Api, ada: Login) -> Wiki {
    let workspace = api.first_workspace().await;
    let bob = api
        .add_member(&workspace, BOB, "bob@example.org", "member")
        .await;
    let eve = api.add_and_login(EVE, "eve@example.org").await;
    Wiki {
        api,
        ada,
        bob,
        eve,
        workspace,
    }
}

async fn wiki() -> Wiki {
    let (api, ada) = logged_in().await;
    wiki_on(api, ada).await
}

async fn wiki_tuned(settings: &[(&str, &str)]) -> Wiki {
    let api = start_tuned(common::ORIGIN, "", settings).await;
    let ada = setup(&api).await;
    wiki_on(api, ada).await
}

async fn setup(api: &Api) -> Login {
    let reply = api.setup().await;
    assert_eq!(reply.status, 201, "{reply:?}");
    Login::from(&reply)
}

impl Wiki {
    async fn space(&self, default_role: Value) -> String {
        let body = json!({ "name": "Handbook", "default_role": default_role });
        let path = format!("/workspaces/{}/spaces", self.workspace);
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), body)
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn page(&self, space: &str) -> String {
        let path = format!("/spaces/{space}/pages");
        let reply = self
            .api
            .send("POST", &path, &self.ada.headers(), json!({}))
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn upload(&self, by: &Login, page: &str, query: &str, kind: &str, body: &[u8]) -> Reply {
        let path = format!("/pages/{page}/attachments?{query}");
        let mut headers = by.headers().to_vec();
        if !kind.is_empty() {
            headers.push(("content-type", kind));
        }
        self.api.raw("POST", &path, &headers, body).await
    }

    /// An attachment `ada` uploads, by id.
    async fn attach(&self, page: &str, name: &str, kind: &str, body: &[u8]) -> String {
        let reply = self
            .upload(&self.ada, page, &format!("name={name}"), kind, body)
            .await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn download(&self, by: &Login, attachment: &str) -> Reply {
        self.api
            .get(by, &format!("/attachments/{attachment}"))
            .await
    }

    async fn delete(&self, by: &Login, attachment: &str) -> Reply {
        let path = format!("/attachments/{attachment}");
        self.api.raw("DELETE", &path, &by.headers(), b"").await
    }

    async fn attachments(&self) -> i64 {
        self.api.number("SELECT count(*) FROM attachments").await
    }

    fn file(&self, attachment: &str) -> PathBuf {
        let end = attachment.len();
        self.api
            .dir()
            .join("files")
            .join(&attachment[end - 2..])
            .join(&attachment[end - 4..end - 2])
            .join(attachment)
    }

    fn temporary_files(&self) -> usize {
        std::fs::read_dir(self.api.dir().join("tmp"))
            .unwrap()
            .count()
    }

    /// Runs a job and waits for it to end.
    async fn run(&self, kind: &str) {
        let queue = &self.api.queue;
        queue
            .enqueue(kind, Some(kind), "{}", now_ms())
            .await
            .unwrap();
        let ended = async {
            while self.api.number("SELECT count(*) FROM jobs").await != 0 {
                sleep(Duration::from_millis(5)).await;
            }
        };
        timeout(Duration::from_secs(5), ended)
            .await
            .expect("the job never ended");
    }
}

#[tokio::test]
async fn an_editor_uploads_and_a_reader_downloads() {
    let wiki = wiki().await;
    let page = wiki.page(&wiki.space(json!("viewer")).await).await;

    let reply = wiki
        .upload(
            &wiki.ada,
            &page,
            "name=notes.txt",
            "text/plain",
            b"hello world",
        )
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["file_name"], "notes.txt");
    assert_eq!(reply.body["content_type"], "text/plain");
    assert_eq!(reply.body["size"], 11);
    let attachment = id(&reply);
    assert!(wiki.file(&attachment).is_file());
    assert_eq!(wiki.temporary_files(), 0);

    let reply = wiki.download(&wiki.bob, &attachment).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.bytes, b"hello world");
    assert_eq!(reply.header("content-type"), Some("text/plain"));
    assert_eq!(reply.header("content-length"), Some("11"));
    assert_eq!(
        reply.header("content-disposition"),
        Some("attachment; filename=\"notes.txt\"; filename*=UTF-8''notes.txt")
    );
    let policy = reply.header("content-security-policy").unwrap();
    assert!(policy.contains("sandbox"), "{policy}");
    assert!(policy.contains("frame-ancestors 'none'"), "{policy}");
    // sha256("hello world")
    let etag = "\"b94d27b9934d3e08a52e52d7da7dabfac484efe37a5380ee9088f7ace2efcde9\"";
    assert_eq!(reply.header("etag"), Some(etag));
    assert_eq!(reply.header("cache-control"), Some("private, no-cache"));

    // The editor reads a file's size this way.
    let path = format!("/attachments/{attachment}");
    let cookie = [("cookie", wiki.bob.cookie.as_str())];
    let head = wiki.api.raw("HEAD", &path, &cookie, b"").await;
    assert_eq!(head.status, 200, "{head:?}");
    assert_eq!(head.header("content-length"), Some("11"));
    assert!(head.bytes.is_empty());

    let headers = [
        ("cookie", wiki.bob.cookie.as_str()),
        ("if-none-match", etag),
    ];
    let again = wiki.api.raw("GET", &path, &headers, b"").await;
    assert_eq!(again.status, 304, "{again:?}");
    assert!(again.bytes.is_empty());
}

#[tokio::test]
async fn only_safe_images_show_in_the_page() {
    let wiki = wiki().await;
    let page = wiki.page(&wiki.space(json!("viewer")).await).await;

    for (kind, disposition) in [
        ("image/png", "inline"),
        ("image/jpeg", "inline"),
        ("image/gif", "inline"),
        ("image/webp", "inline"),
        ("image/svg+xml", "attachment"),
        ("text/html", "attachment"),
    ] {
        let attachment = wiki
            .attach(&page, "file", kind, b"<svg onload=alert(1)>")
            .await;
        let reply = wiki.download(&wiki.bob, &attachment).await;
        let found = reply.header("content-disposition").unwrap();
        assert!(found.starts_with(disposition), "{kind}: {found}");
        assert_eq!(reply.header("x-content-type-options"), Some("nosniff"));
    }
}

#[tokio::test]
async fn names_and_types_are_cleaned_up() {
    let wiki = wiki().await;
    let page = wiki.page(&wiki.space(json!("viewer")).await).await;

    let attachment = wiki
        .attach(
            &page,
            "r%C3%A9sum%C3%A9+%22final%22.pdf",
            "application/pdf",
            b"%PDF",
        )
        .await;
    let reply = wiki.download(&wiki.ada, &attachment).await;
    assert_eq!(
        reply.header("content-disposition"),
        Some(
            "attachment; filename=\"r_sum_ _final_.pdf\"; filename*=UTF-8''r%C3%A9sum%C3%A9%20%22final%22.pdf"
        )
    );

    for (kind, stored) in [
        ("Text/HTML; charset=utf-8", "text/html"),
        ("not a type", "application/octet-stream"),
        ("", "application/octet-stream"),
    ] {
        let reply = wiki.upload(&wiki.ada, &page, "name=a", kind, b"x").await;
        assert_eq!(reply.status, 201, "{reply:?}");
        assert_eq!(reply.body["content_type"], stored, "{kind}");
    }

    let long = "a".repeat(256);
    for query in [
        "",
        "name=",
        "name=%20",
        "name=a%0Ab",
        &format!("name={long}"),
        "name=%FF",
    ] {
        let reply = wiki
            .upload(&wiki.ada, &page, query, "text/plain", b"x")
            .await;
        assert_eq!(reply.status, 400, "{query}: {reply:?}");
        assert!(
            reply.body["error"]["fields"]["name"].is_string(),
            "{reply:?}"
        );
    }
    assert_eq!(wiki.attachments().await, 4);
}

#[tokio::test]
async fn only_an_editor_of_a_live_page_uploads() {
    let wiki = wiki().await;
    let readable = wiki.page(&wiki.space(json!("viewer")).await).await;
    let trashed = wiki.page(&wiki.space(json!("editor")).await).await;
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

    let reply = wiki
        .upload(&wiki.bob, &readable, "name=a", "text/plain", b"x")
        .await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let reply = wiki
        .upload(&wiki.eve, &readable, "name=a", "text/plain", b"x")
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = wiki
        .upload(&wiki.ada, &trashed, "name=a", "text/plain", b"x")
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let unknown = "99999999-0000-7000-8000-000000000001";
    let reply = wiki
        .upload(&wiki.ada, unknown, "name=a", "text/plain", b"x")
        .await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));

    let path = format!("/pages/{readable}/attachments?name=a");
    let no_csrf = [("cookie", wiki.ada.cookie.as_str())];
    let reply = wiki.api.raw("POST", &path, &no_csrf, b"x").await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
    let reply = wiki.api.raw("POST", &path, &[], b"x").await;
    assert_eq!((reply.status, reply.code()), (401, "unauthenticated"));

    assert_eq!(wiki.attachments().await, 0);
    assert_eq!(wiki.temporary_files(), 0);
}

#[tokio::test]
async fn a_download_needs_one_readable_page_that_references_the_file() {
    let wiki = wiki().await;
    let closed = wiki.page(&wiki.space(Value::Null).await).await;
    let open_space = wiki.space(json!("viewer")).await;
    let open = wiki.page(&open_space).await;
    let attachment = wiki.attach(&closed, "a.txt", "text/plain", b"x").await;

    assert_eq!(wiki.download(&wiki.ada, &attachment).await.status, 200);
    assert_eq!(wiki.download(&wiki.bob, &attachment).await.status, 404);
    assert_eq!(wiki.download(&wiki.eve, &attachment).await.status, 404);
    let reply = wiki
        .api
        .raw("GET", &format!("/attachments/{attachment}"), &[], b"")
        .await;
    assert_eq!(reply.status, 401);

    // What the indexing job of step 18 will write when the file is pasted into another page.
    wiki.api
        .sql(&format!(
            "INSERT INTO attachment_refs (attachment_id, page_id) VALUES ('{attachment}', '{open}')"
        ))
        .await;
    assert_eq!(wiki.download(&wiki.bob, &attachment).await.status, 200);

    let reply = wiki
        .api
        .raw(
            "DELETE",
            &format!("/pages/{open}"),
            &wiki.ada.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(wiki.download(&wiki.bob, &attachment).await.status, 404);

    let unknown = "99999999-0000-7000-8000-000000000001";
    assert_eq!(wiki.download(&wiki.ada, unknown).await.status, 404);
}

#[tokio::test]
async fn a_delete_needs_an_editor_of_every_page_that_references_the_file() {
    let wiki = wiki().await;
    let closed = wiki.page(&wiki.space(Value::Null).await).await;
    let read_only = wiki.page(&wiki.space(json!("viewer")).await).await;
    let editable_space = wiki.space(json!("editor")).await;
    let editable = wiki.page(&editable_space).await;
    let attachment = wiki.attach(&closed, "a.txt", "text/plain", b"x").await;
    let reference = |page: &str| {
        format!(
            "INSERT INTO attachment_refs (attachment_id, page_id) VALUES ('{attachment}', '{page}')"
        )
    };

    assert_eq!(wiki.delete(&wiki.bob, &attachment).await.status, 404);
    wiki.api.sql(&reference(&read_only)).await;
    let reply = wiki.delete(&wiki.bob, &attachment).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    assert_eq!(wiki.delete(&wiki.eve, &attachment).await.status, 404);

    // Pasted into a page Bob edits, the file stays out of his reach in the pages he only reads
    // or cannot see.
    wiki.api.sql(&reference(&editable)).await;
    let reply = wiki.delete(&wiki.bob, &attachment).await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    assert!(wiki.file(&attachment).exists());

    // A page in the trash counts: restoring it must find its file.
    let mine = wiki.attach(&editable, "b.txt", "text/plain", b"y").await;
    let trashed = wiki.page(&editable_space).await;
    wiki.api
        .sql(&format!(
            "INSERT INTO attachment_refs (attachment_id, page_id) VALUES ('{mine}', '{trashed}')"
        ))
        .await;
    let path = format!("/pages/{trashed}");
    let reply = wiki
        .api
        .raw("DELETE", &path, &wiki.ada.headers(), b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = wiki.delete(&wiki.bob, &mine).await;
    assert_eq!(reply.status, 204, "{reply:?}");

    let reply = wiki.delete(&wiki.ada, &attachment).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    assert_eq!(wiki.attachments().await, 0);
    assert_eq!(
        wiki.api
            .number("SELECT count(*) FROM attachment_refs")
            .await,
        0
    );
    assert!(!wiki.file(&attachment).exists());
    assert_eq!(wiki.download(&wiki.ada, &attachment).await.status, 404);
    assert_eq!(wiki.delete(&wiki.ada, &attachment).await.status, 404);
}

#[tokio::test]
async fn a_file_over_the_upload_limit_is_refused() {
    let wiki = wiki_tuned(&[("CADOCS_MAX_UPLOAD_MB", "1")]).await;
    let page = wiki.page(&wiki.space(json!("editor")).await).await;

    let path = format!("/api/v1/pages/{page}/attachments?name=big");
    let reply = announced(&wiki.api, &path, &wiki.ada, MIB + 1).await;
    assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
    wiki.attach(&page, "fits", "", &vec![1; MIB]).await;

    // Without a length, the limit applies while the body streams in.
    let reply = chunked(&wiki.api, &path, &wiki.ada, MIB + 1).await;
    assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");

    assert_eq!(wiki.attachments().await, 1);
    assert_eq!(wiki.temporary_files(), 0);
}

/// Announces `size` bytes and sends none, so that a refusal read no body. A refusal with a body
/// left unread can reach the client as a reset instead.
async fn announced(api: &Api, path: &str, by: &Login, size: usize) -> String {
    let mut stream = TcpStream::connect(api.addr).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nhost: test\r\nconnection: close\r\ncookie: {}\r\n\
         x-csrf-token: {}\r\ncontent-length: {size}\r\n\r\n",
        by.cookie, by.csrf
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let mut answer = Vec::new();
    let _ = stream.read_to_end(&mut answer).await;
    String::from_utf8_lossy(&answer).into_owned()
}

/// Sends `size` bytes in chunks of 64 KiB, with no `content-length` and no last chunk, so the
/// server reads all that was sent, and answers what the server sent back.
async fn chunked(api: &Api, path: &str, by: &Login, size: usize) -> String {
    let mut stream = TcpStream::connect(api.addr).await.unwrap();
    let head = format!(
        "POST {path} HTTP/1.1\r\nhost: test\r\nconnection: close\r\ncookie: {}\r\n\
         x-csrf-token: {}\r\ntransfer-encoding: chunked\r\n\r\n",
        by.cookie, by.csrf
    );
    stream.write_all(head.as_bytes()).await.unwrap();
    let (mut read, mut write) = stream.into_split();
    let writer = tokio::spawn(async move {
        let chunk = vec![1u8; 64 * 1024];
        let mut left = size;
        while left > 0 {
            let part = &chunk[..left.min(chunk.len())];
            let frame = [format!("{:x}\r\n", part.len()).as_bytes(), part, b"\r\n"].concat();
            if write.write_all(&frame).await.is_err() {
                return;
            }
            left -= part.len();
        }
        // Held open: the server answers before the body ends.
        std::future::pending::<()>().await;
        drop(write);
    });
    let mut answer = Vec::new();
    let _ = read.read_to_end(&mut answer).await;
    writer.abort();
    String::from_utf8_lossy(&answer).into_owned()
}

#[tokio::test]
async fn a_workspace_stops_at_its_quota() {
    let wiki = wiki_tuned(&[("CADOCS_WORKSPACE_QUOTA_MB", "1")]).await;
    let page = wiki.page(&wiki.space(json!("editor")).await).await;

    wiki.attach(&page, "first", "", &vec![1; 600 * 1024]).await;
    let path = format!("/api/v1/pages/{page}/attachments?name=second");
    let reply = announced(&wiki.api, &path, &wiki.ada, 600 * 1024).await;
    assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
    // A file the client does not announce is cut at what is left.
    let reply = chunked(&wiki.api, &path, &wiki.ada, 424 * 1024 + 1).await;
    assert!(reply.starts_with("HTTP/1.1 413"), "{reply}");
    wiki.attach(&page, "rest", "", &vec![1; 424 * 1024]).await;

    // Full: even an empty file is refused before its body is read.
    let reply = wiki.upload(&wiki.ada, &page, "name=more", "", b"").await;
    assert_eq!((reply.status, reply.code()), (413, "too_large"));
    assert_eq!(wiki.attachments().await, 2);
    assert_eq!(wiki.temporary_files(), 0);

    // Deleting gives the room back.
    let first: String = sqlx::query_scalar("SELECT id FROM attachments WHERE file_name = 'first'")
        .fetch_one(&wiki.api.db)
        .await
        .unwrap();
    assert_eq!(wiki.delete(&wiki.ada, &first).await.status, 204);
    wiki.attach(&page, "again", "", &vec![1; 600 * 1024]).await;
}

/// Reads the body, then fails as a full disk does, as `LocalFileStore` would after
/// removing its temporary file.
struct FullDisk;

impl FileStore for FullDisk {
    fn put<'a>(
        &'a self,
        _id: &'a str,
        mut body: ByteStream<'a>,
        _max_bytes: u64,
    ) -> BoxFuture<'a, Result<Stored, FileError>> {
        Box::pin(async move {
            while body.next().await.is_some() {}
            Err(FileError::Io(io::Error::from(io::ErrorKind::StorageFull)))
        })
    }

    fn get<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<FileBody, FileError>> {
        Box::pin(async { Err(FileError::NotFound) })
    }

    fn delete<'a>(&'a self, _id: &'a str) -> BoxFuture<'a, Result<(), FileError>> {
        Box::pin(async { Ok(()) })
    }

    fn list(&self) -> FileList {
        Box::pin(futures_util::stream::empty())
    }
}

#[tokio::test]
async fn an_upload_on_a_full_disk_fails_and_records_nothing() {
    let api = start_with_files(Arc::new(FullDisk), &[]).await;
    let ada = setup(&api).await;
    let wiki = wiki_on(api, ada).await;
    let page = wiki.page(&wiki.space(json!("editor")).await).await;

    let reply = wiki
        .upload(&wiki.ada, &page, "name=a", "", &vec![1; 256 * 1024])
        .await;
    assert_eq!((reply.status, reply.code()), (500, "internal"));
    assert_eq!(wiki.attachments().await, 0);
    assert_eq!(
        wiki.api
            .number("SELECT count(*) FROM attachment_refs")
            .await,
        0
    );
}

#[tokio::test]
async fn files_no_page_references_go_with_their_rows() {
    let wiki = wiki().await;
    let space = wiki.space(json!("editor")).await;
    let purged = wiki.page(&space).await;
    let kept = wiki.page(&space).await;
    let orphan = wiki.attach(&purged, "orphan", "", b"x").await;
    let shared = wiki.attach(&purged, "shared", "", b"x").await;
    let untouched = wiki.attach(&kept, "untouched", "", b"x").await;
    wiki.api
        .sql(&format!(
            "INSERT INTO attachment_refs (attachment_id, page_id) VALUES ('{shared}', '{kept}')"
        ))
        .await;

    let headers = wiki.ada.headers();
    let reply = wiki
        .api
        .raw("DELETE", &format!("/pages/{purged}"), &headers, b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = wiki
        .api
        .raw("DELETE", &format!("/pages/{purged}/purge"), &headers, b"")
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");

    // Files with no row: one a crash left after its rename, one from a backup restored older.
    let store = LocalFileStore::open(wiki.api.dir()).unwrap();
    let stray_old = "0190a1b2-c3d4-7e5f-8a9b-00000000aaaa";
    let stray_new = "0190a1b2-c3d4-7e5f-8a9b-00000000bbbb";
    for stray in [stray_old, stray_new] {
        let body: ByteStream = Box::pin(futures_util::stream::iter([Ok(bytes(b"x"))]));
        store.put(stray, body, 10).await.unwrap();
    }
    let old = SystemTime::now() - Duration::from_secs(2 * 3600);
    let file = std::fs::File::options()
        .write(true)
        .open(wiki.file(stray_old))
        .unwrap();
    file.set_modified(old).unwrap();

    wiki.run(PURGE_ORPHAN_FILES).await;

    let left: Vec<String> = sqlx::query_scalar("SELECT id FROM attachments ORDER BY file_name")
        .fetch_all(&wiki.api.db)
        .await
        .unwrap();
    assert_eq!(left, [shared.clone(), untouched.clone()]);
    assert!(!wiki.file(&orphan).exists());
    assert!(wiki.file(&shared).exists());
    assert!(wiki.file(&untouched).exists());
    assert!(!wiki.file(stray_old).exists());
    assert!(
        wiki.file(stray_new).exists(),
        "an upload may be about to record it"
    );
}

fn bytes(data: &'static [u8]) -> bytes::Bytes {
    bytes::Bytes::from_static(data)
}

const FILES_ORIGIN: &str = "https://files.example.org";

/// Keeps files on disk, and sends browsers to another origin for them, as the S3 store does in
/// `redirect` mode (ADR 0020).
struct Redirecting {
    local: LocalFileStore,
    _dir: tempfile::TempDir,
}

impl Redirecting {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        Self {
            local: LocalFileStore::open(dir.path()).unwrap(),
            _dir: dir,
        }
    }
}

impl FileStore for Redirecting {
    fn put<'a>(
        &'a self,
        id: &'a str,
        body: ByteStream<'a>,
        max_bytes: u64,
    ) -> BoxFuture<'a, Result<Stored, FileError>> {
        self.local.put(id, body, max_bytes)
    }

    fn get<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<FileBody, FileError>> {
        self.local.get(id)
    }

    fn delete<'a>(&'a self, id: &'a str) -> BoxFuture<'a, Result<(), FileError>> {
        self.local.delete(id)
    }

    fn list(&self) -> FileList {
        self.local.list()
    }

    fn download_url(&self, id: &str, download: &Download<'_>) -> Option<String> {
        Some(format!(
            "{FILES_ORIGIN}/{id}?type={}&disposition={}",
            download.content_type, download.disposition
        ))
    }

    fn download_origin(&self) -> Option<String> {
        Some(FILES_ORIGIN.to_owned())
    }
}

#[tokio::test]
async fn a_store_that_links_gets_a_redirect_after_the_access_check() {
    let api = start_with_files(Arc::new(Redirecting::new()), &[]).await;
    let ada = setup(&api).await;
    let wiki = wiki_on(api, ada).await;
    let page = wiki.page(&wiki.space(json!("viewer")).await).await;
    let image = wiki.attach(&page, "a.png", "image/png", b"png").await;
    let html = wiki.attach(&page, "a.html", "text/html", b"<b>").await;

    let reply = wiki.download(&wiki.bob, &image).await;
    assert_eq!(reply.status, 302, "{reply:?}");
    assert_eq!(
        reply.header("location").unwrap(),
        format!(
            "{FILES_ORIGIN}/{image}?type=image/png\
             &disposition=inline; filename=\"a.png\"; filename*=UTF-8''a.png"
        )
    );
    // Each view goes through the check again, so a revoked reader stops getting links.
    assert_eq!(reply.header("cache-control"), Some("private, no-store"));

    // Anything but a safe image is streamed by the server, with its sandbox and `nosniff`: the
    // storage's origin, maybe shared with other applications, sends neither.
    let reply = wiki.download(&wiki.bob, &html).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.bytes, b"<b>");
    let policy = reply.header("content-security-policy").unwrap();
    assert!(policy.contains("sandbox"), "{policy}");

    assert_eq!(wiki.download(&wiki.eve, &image).await.status, 404);
}

#[tokio::test]
async fn images_may_come_from_the_origin_of_the_links() {
    let api = start_with_files(Arc::new(Redirecting::new()), &[]).await;
    let reply = api.raw("GET", "/health", &[], b"").await;
    let policy = reply.header("content-security-policy").unwrap();
    assert!(
        policy.contains(&format!("img-src 'self' data: {FILES_ORIGIN};")),
        "{policy}"
    );

    let (api, _) = logged_in().await;
    let reply = api.raw("GET", "/health", &[], b"").await;
    let policy = reply.header("content-security-policy").unwrap();
    assert!(policy.contains("img-src 'self' data:;"), "{policy}");
}
