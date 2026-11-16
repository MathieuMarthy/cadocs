//! Upgrades from every release: each folder of `tests/upgrade` is the data folder a published
//! release left (`scripts/make-upgrade-fixture.sh`), which the current build restores, migrates and
//! serves with all its data.

mod common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{Api, Login, PASSWORD, Reply, start_in_tuned};
use serde_json::{Value, json};
use tempfile::TempDir;

/// The releases with a fixture. A new release adds its own, which the test then requires.
const RELEASES: &[&str] = &["v0.1.0", "v0.2.0"];
/// What `make-upgrade-fixture.sh` uploaded.
const PNG: &[u8] = b"\x89PNG\r\n\x1a\nupgrade fixture";

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/upgrade")
}

/// The fixture of `release`, restored by the current `cadocs restore` into a new data folder, which
/// is never the fixture itself: opening a database migrates it.
fn restore(release: &str) -> TempDir {
    let data = TempDir::new().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_cadocs"))
        .env_clear()
        .env("CADOCS_DATA_DIR", data.path())
        .env("CADOCS_PUBLIC_URL", common::ORIGIN)
        .arg("restore")
        .arg(fixtures().join(release))
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{release}: {stderr}");
    data
}

/// The fixtures' dates never move, so nothing may expire while the test reads them.
async fn serve(release: &str) -> Api {
    let settings = [
        ("CADOCS_TRASH_DAYS", "3650"),
        ("CADOCS_REVISION_KEEP_DAYS", "3650"),
    ];
    start_in_tuned(restore(release), &settings).await
}

async fn login(api: &Api, release: &str, email: &str) -> (Login, Value) {
    let reply = api.login(email, PASSWORD).await;
    assert_eq!(reply.status, 200, "{release}: {reply:?}");
    (Login::from(&reply), reply.body)
}

async fn get(api: &Api, release: &str, by: &Login, path: &str) -> Reply {
    let reply = api.get(by, path).await;
    assert_eq!(reply.status, 200, "{release} {path}: {reply:?}");
    reply
}

fn items(reply: &Reply) -> &Vec<Value> {
    reply.body["items"].as_array().unwrap()
}

fn find<'a>(list: &'a [Value], field: &str, value: &str) -> &'a Value {
    let found = list.iter().find(|item| item[field] == value);
    found.unwrap_or_else(|| panic!("no {field} {value} in {list:?}"))
}

fn id(value: &Value) -> &str {
    value["id"].as_str().unwrap()
}

#[test]
fn every_fixture_is_listed() {
    let mut found: Vec<String> = std::fs::read_dir(fixtures())
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    found.sort();
    assert_eq!(found, RELEASES);
}

#[tokio::test(flavor = "multi_thread")]
async fn every_release_upgrades_with_its_data() {
    for release in RELEASES {
        check(release).await;
    }
}

async fn check(release: &str) {
    let api = serve(release).await;
    let migrations = std::fs::read_dir(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../cadocs-store/migrations/sqlite"),
    )
    .unwrap()
    .count();
    let applied = api
        .number("SELECT count(*) FROM _sqlx_migrations WHERE success")
        .await;
    assert_eq!(applied, migrations as i64, "{release}");
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(reply.body, json!({ "required": false }), "{release}");

    // Both accounts log in with their password, in their roles.
    let (ada, session) = login(&api, release, "ada@example.org").await;
    assert_eq!(session["user"]["is_instance_admin"], true, "{release}");
    assert_eq!(session["user"]["email_2fa"], false, "{release}");
    let workspace = &session["workspaces"][0];
    assert_eq!(workspace["name"], "Acme", "{release}");
    assert_eq!(workspace["role"], "owner", "{release}");
    let workspace = id(workspace).to_owned();
    let (bob, session) = login(&api, release, "bob@example.org").await;
    assert_eq!(session["user"]["is_instance_admin"], false, "{release}");
    assert_eq!(session["workspaces"][0]["role"], "member", "{release}");
    let bob_id = session["user"]["id"].as_str().unwrap().to_owned();

    // The space, its default role and Bob's grant.
    let reply = get(
        &api,
        release,
        &bob,
        &format!("/workspaces/{workspace}/spaces"),
    )
    .await;
    let space = find(items(&reply), "name", "Handbook");
    assert_eq!(space["description"], "How we work", "{release}");
    assert_eq!(space["default_role"], "viewer", "{release}");
    let space = id(space).to_owned();
    let reply = get(&api, release, &ada, &format!("/spaces/{space}/grants")).await;
    let grant = find(items(&reply), "user_id", &bob_id);
    assert_eq!(grant["role"], "editor", "{release}");

    // The tree, with the child under its parent.
    let reply = get(&api, release, &bob, &format!("/spaces/{space}/tree")).await;
    let roots = items(&reply);
    assert_eq!(roots.len(), 1, "{release}: {roots:?}");
    let welcome = find(roots, "title", "Welcome");
    assert_eq!(welcome["has_children"], true, "{release}");
    let welcome = id(welcome).to_owned();
    let path = format!("/spaces/{space}/tree?parent={welcome}");
    let reply = get(&api, release, &bob, &path).await;
    let onboarding = id(find(items(&reply), "title", "Onboarding")).to_owned();

    // Content, from the stored updates and snapshots.
    let path = format!("/pages/{welcome}/export?format=md");
    let markdown = String::from_utf8(get(&api, release, &bob, &path).await.bytes).unwrap();
    for part in ["# Welcome", "**quokka**", "- one", "```rust\nfn main() {}"] {
        assert!(
            markdown.contains(part),
            "{release}: no {part} in {markdown}"
        );
    }
    let path = format!("/pages/{onboarding}/export?format=md");
    let markdown = String::from_utf8(get(&api, release, &bob, &path).await.bytes).unwrap();
    assert!(markdown.contains(&welcome), "{release}: {markdown}");
    let (_, image) = markdown
        .split_once("![the chart](")
        .unwrap_or_else(|| panic!("{release}: no image in {markdown}"));
    let image = image.split(')').next().unwrap();
    let image = image.rsplit('/').next().unwrap();
    let reply = get(&api, release, &bob, &format!("/attachments/{image}")).await;
    assert_eq!(reply.bytes, PNG, "{release}");

    // The search index and the revisions came through too.
    let path = format!("/workspaces/{workspace}/search?q=quokka");
    let reply = get(&api, release, &bob, &path).await;
    find(items(&reply), "id", &welcome);
    let reply = get(&api, release, &bob, &format!("/pages/{welcome}/revisions")).await;
    let number = items(&reply)[0]["number"].as_i64().unwrap();
    let path = format!("/pages/{welcome}/revisions/{number}");
    let html = &get(&api, release, &bob, &path).await.body["html"];
    assert!(
        html.as_str().unwrap().contains("quokka"),
        "{release}: {html}"
    );

    // The trashed page waits in the trash.
    let reply = get(&api, release, &ada, &format!("/spaces/{space}/trash")).await;
    find(items(&reply), "title", "Old notes");

    // And the upgraded database takes writes.
    let path = format!("/spaces/{space}/pages");
    let reply = api
        .send(
            "POST",
            &path,
            &bob.headers(),
            json!({ "parent_id": welcome }),
        )
        .await;
    assert_eq!(reply.status, 201, "{release}: {reply:?}");
}
