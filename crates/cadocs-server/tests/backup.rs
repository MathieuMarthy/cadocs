//! `cadocs backup` beside a running server, and `cadocs restore` into a new data folder, which a
//! server then serves as it was at the backup.

mod common;

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use common::{Api, Login, ORIGIN, Reply, logged_in, start_in};
use serde_json::json;
use tempfile::TempDir;
use tokio::task::spawn_blocking;
use tokio::time::{Duration, Instant, sleep};

const PNG: &[u8] = b"\x89PNG\r\n\x1a\nnot really an image";

/// Runs the `cadocs` binary on `data_dir`, off the runtime that serves.
async fn cadocs(data_dir: &Path, args: &[&str]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cadocs"));
    command
        .env_clear()
        .env("CADOCS_DATA_DIR", data_dir)
        .env("CADOCS_PUBLIC_URL", ORIGIN)
        .args(args);
    spawn_blocking(move || command.output().unwrap())
        .await
        .unwrap()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

fn arg(path: &Path) -> &str {
    path.to_str().unwrap()
}

/// A space holding a page whose content shows an image attachment, flushed to the database.
async fn wiki() -> (Api, Login, String, String) {
    let (api, ada) = logged_in().await;
    let workspace = api.first_workspace().await;
    let path = format!("/workspaces/{workspace}/spaces");
    let reply = api
        .send("POST", &path, &ada.headers(), json!({ "name": "Handbook" }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let space = id(&reply);

    let path = format!("/spaces/{space}/pages");
    let reply = api.send("POST", &path, &ada.headers(), json!({})).await;
    let holder = id(&reply);
    let path = format!("/pages/{holder}/attachments?name=chart.png");
    let mut headers = ada.headers().to_vec();
    headers.push(("content-type", "image/png"));
    let reply = api.raw("POST", &path, &headers, PNG).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let image = id(&reply);

    let markdown = format!("# Quarterly review\n\n![the chart](/api/v1/attachments/{image})\n");
    let path = format!("/spaces/{space}/import?name=notes.md");
    let mut headers = ada.headers().to_vec();
    headers.push(("content-type", "text/markdown"));
    let reply = api.raw("POST", &path, &headers, markdown.as_bytes()).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let page = id(&reply);

    // The title follows the content at the next flush, so the content is stored once it shows.
    let deadline = Instant::now() + Duration::from_secs(10);
    let flushed =
        format!("SELECT count(*) FROM pages WHERE id = '{page}' AND title = 'Quarterly review'");
    while api.number(&flushed).await == 0 {
        assert!(Instant::now() < deadline, "the import was never flushed");
        sleep(Duration::from_millis(50)).await;
    }
    (api, ada, page, image)
}

fn files_under(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return found;
    };
    for entry in entries {
        let path = entry.unwrap().path();
        if path.is_dir() {
            found.extend(files_under(&path));
        } else {
            found.push(path);
        }
    }
    found
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_of_a_running_server_restores_into_a_new_data_folder() {
    let (api, ada, page, image) = wiki().await;
    let backup = TempDir::new().unwrap();

    let output = cadocs(api.dir(), &["backup", arg(backup.path())]).await;
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(backup.path().join("cadocs.db").is_file());
    assert_eq!(files_under(&backup.path().join("files")).len(), 1);

    // Made after the backup, so the restored server must not have it.
    let workspace = api.first_workspace().await;
    let path = format!("/workspaces/{workspace}/spaces");
    let reply = api
        .send("POST", &path, &ada.headers(), json!({ "name": "Later" }))
        .await;
    let later = id(&reply);

    let restored = TempDir::new().unwrap();
    let output = cadocs(restored.path(), &["restore", arg(backup.path())]).await;
    assert!(output.status.success(), "{}", stderr(&output));

    let copy = start_in(restored).await;
    // The session was in the database too.
    let reply = copy
        .get(&ada, &format!("/pages/{page}/export?format=md"))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let markdown = String::from_utf8(reply.bytes).unwrap();
    assert!(markdown.starts_with("# Quarterly review"), "{markdown}");
    assert!(markdown.contains(&image), "{markdown}");
    let reply = copy.get(&ada, &format!("/attachments/{image}")).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.bytes, PNG);
    let reply = copy.get(&ada, &format!("/spaces/{later}")).await;
    assert_eq!(reply.status, 404, "{reply:?}");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_can_run_again_into_the_same_folder() {
    let (api, _ada, _page, _image) = wiki().await;
    let backup = TempDir::new().unwrap();
    for _ in 0..2 {
        let output = cadocs(api.dir(), &["backup", arg(backup.path())]).await;
        assert!(output.status.success(), "{}", stderr(&output));
    }
    let mut names: Vec<_> = std::fs::read_dir(backup.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, ["cadocs.db", "files"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_backup_never_writes_into_the_data_folder() {
    let (api, _ada, _page, _image) = wiki().await;
    let before = std::fs::metadata(api.dir().join("cadocs.db"))
        .unwrap()
        .len();
    let output = cadocs(api.dir(), &["backup", arg(api.dir())]).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("data folder"),
        "{}",
        stderr(&output)
    );
    let inside = api.dir().join("files");
    let output = cadocs(api.dir(), &["backup", arg(&inside)]).await;
    assert!(!output.status.success());
    assert_eq!(
        std::fs::metadata(api.dir().join("cadocs.db"))
            .unwrap()
            .len(),
        before
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_refuses_a_running_server_or_a_folder_with_a_database() {
    let (api, _ada, _page, _image) = wiki().await;
    let backup = TempDir::new().unwrap();
    let output = cadocs(api.dir(), &["backup", arg(backup.path())]).await;
    assert!(output.status.success(), "{}", stderr(&output));

    // The in-process server of the tests holds no lock, so a real one is not needed to hold it:
    // the lock is what `serve` takes.
    let held = cadocs_server::data_dir::lock(api.dir()).unwrap();
    let output = cadocs(api.dir(), &["restore", arg(backup.path())]).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("in use by another server"),
        "{}",
        stderr(&output)
    );
    drop(held);

    let output = cadocs(api.dir(), &["restore", arg(backup.path())]).await;
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("already holds a database"),
        "{}",
        stderr(&output)
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_restore_refuses_a_folder_that_is_no_backup() {
    let empty = TempDir::new().unwrap();
    let data = TempDir::new().unwrap();
    let output = cadocs(data.path(), &["restore", arg(empty.path())]).await;
    assert!(!output.status.success());
    assert!(stderr(&output).contains("cadocs.db"), "{}", stderr(&output));
    assert!(!data.path().join("cadocs.db").exists());
}
