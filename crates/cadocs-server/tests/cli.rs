//! The `cadocs` binary itself: what can only go wrong between `main` and the library.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

use cadocs_mail::fake::{Behavior, FakeRelay};
use cadocs_store::{AccountStore, FirstUser, NewSession, SqliteStore};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use tempfile::TempDir;

fn cadocs(data_dir: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_cadocs"));
    command
        .env_clear()
        .env("CADOCS_DATA_DIR", data_dir)
        .env("CADOCS_BIND", "127.0.0.1:0")
        .env("CADOCS_PUBLIC_URL", "http://127.0.0.1:3000");
    command
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// Kills the server even when an assertion fails first.
struct Server(Child);

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// Starts a server and returns once it listens.
fn start(mut command: Command) -> Server {
    let mut server = Server(command.stdout(Stdio::piped()).spawn().unwrap());
    let stdout = server.0.stdout.take().unwrap();
    let listening = BufReader::new(stdout)
        .lines()
        .any(|line| line.unwrap().contains("listening"));
    assert!(listening, "the server stopped before it listened");
    server
}

#[test]
fn an_unknown_command_is_refused_before_anything_else() {
    let dir = TempDir::new().unwrap();
    let output = cadocs(dir.path())
        .env_remove("CADOCS_PUBLIC_URL")
        .arg("nonsense")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("usage: cadocs [serve]"));
    assert!(!dir.path().join("cadocs.lock").exists());
}

#[test]
fn the_public_url_is_required_to_serve() {
    let dir = TempDir::new().unwrap();
    let output = cadocs(dir.path())
        .env_remove("CADOCS_PUBLIC_URL")
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("CADOCS_PUBLIC_URL"));
}

#[test]
fn serve_runs_and_holds_the_data_folder() {
    let dir = TempDir::new().unwrap();
    let mut command = cadocs(dir.path());
    command.arg("serve");
    let _server = start(command);

    let output = cadocs(dir.path()).output().unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("in use by another server"));
}

#[test]
fn no_command_also_serves() {
    let dir = TempDir::new().unwrap();
    let _server = start(cadocs(dir.path()));
}

/// An account in a database of its own, as a first run leaves it.
fn seed(dir: &Path) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let store = SqliteStore::open(&dir.join("cadocs.db")).await.unwrap();
            let first = FirstUser {
                id: "user-1".to_owned(),
                email: "ada@example.org".to_owned(),
                name: "Ada".to_owned(),
                password_hash: "hash".to_owned(),
                workspace_id: "workspace-1".to_owned(),
                workspace_slug: "acme".to_owned(),
                workspace_name: "Acme".to_owned(),
            };
            let session = NewSession {
                token_hash: "session".to_owned(),
                user_id: "user-1".to_owned(),
                now: 1,
                expires_at: 2,
            };
            store.create_first_user(&first, &session).await.unwrap();
        });
}

#[test]
fn reset_password_prints_a_link_beside_a_running_server() {
    let dir = TempDir::new().unwrap();
    seed(dir.path());
    let _server = start(cadocs(dir.path()));

    let output = cadocs(dir.path())
        .args(["reset-password", " Ada@Example.org "])
        .output()
        .unwrap();
    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8(output.stdout).unwrap();
    let token = stdout
        .trim()
        .strip_prefix("http://127.0.0.1:3000/reset-password/")
        .unwrap_or_else(|| panic!("{stdout}"));
    assert_eq!(token.len(), 64);
}

#[test]
fn reset_password_refuses_an_unknown_email() {
    let dir = TempDir::new().unwrap();
    seed(dir.path());
    let output = cadocs(dir.path())
        .args(["reset-password", "nobody@example.org"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(output.stdout.is_empty());
    assert!(
        stderr(&output).contains("no account"),
        "{}",
        stderr(&output)
    );
}

/// A database at another version, as after an upgrade or a downgrade.
fn run_sql(dir: &Path, statement: &'static str) {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let options = SqliteConnectOptions::new().filename(dir.join("cadocs.db"));
            let db = SqlitePool::connect_with(options).await.unwrap();
            sqlx::query(statement).execute(&db).await.unwrap();
            db.close().await;
        });
}

#[test]
fn reset_password_refuses_a_database_of_another_version() {
    for statement in [
        "DELETE FROM _sqlx_migrations
         WHERE version = (SELECT max(version) FROM _sqlx_migrations)",
        "INSERT INTO _sqlx_migrations
             (version, description, success, checksum, execution_time)
         VALUES (9999, 'from the future', 1, x'00', 0)",
    ] {
        let dir = TempDir::new().unwrap();
        seed(dir.path());
        run_sql(dir.path(), statement);
        let output = cadocs(dir.path())
            .args(["reset-password", "ada@example.org"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(
            stderr(&output).contains("another version"),
            "{}",
            stderr(&output)
        );
    }
}

#[test]
fn reset_password_does_not_make_a_database() {
    let dir = TempDir::new().unwrap();
    let output = cadocs(dir.path())
        .args(["reset-password", "ada@example.org"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(stderr(&output).contains("cadocs.db"), "{}", stderr(&output));
    assert!(!dir.path().join("cadocs.db").exists());
}

/// A relay on its own runtime, which lives as long as what this returns.
fn fake_relay(behavior: Behavior) -> (tokio::runtime::Runtime, FakeRelay) {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .enable_all()
        .build()
        .unwrap();
    let relay = runtime.block_on(FakeRelay::start(behavior));
    (runtime, relay)
}

fn with_relay(command: &mut Command, relay: &FakeRelay) {
    command
        .env("CADOCS_SMTP_HOST", "127.0.0.1")
        .env("CADOCS_SMTP_PORT", relay.port().to_string())
        .env("CADOCS_SMTP_TLS", "none")
        .env("CADOCS_SMTP_FROM", "Team wiki <wiki@example.org>");
}

#[test]
fn test_mail_sends_one_message_through_the_relay() {
    let dir = TempDir::new().unwrap();
    let (_runtime, relay) = fake_relay(Behavior::default());
    let mut command = cadocs(dir.path());
    with_relay(&mut command, &relay);

    let output = command
        .args(["test-mail", "ada@example.net"])
        .output()
        .unwrap();

    assert!(output.status.success(), "{}", stderr(&output));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("ada@example.net"), "{stdout}");
    let messages = relay.messages();
    assert_eq!(messages.len(), 1);
    assert!(messages[0].to.starts_with("<ada@example.net>"));
    // It names the instance, so whoever receives it knows where it comes from.
    assert!(messages[0].data.contains("http://127.0.0.1:3000"));
    // It needs no database, so it runs before the first start.
    assert!(!dir.path().join("cadocs.db").exists());
}

#[test]
fn test_mail_reports_what_the_relay_answered() {
    let dir = TempDir::new().unwrap();
    let (_runtime, relay) = fake_relay(Behavior {
        refuse_recipients: true,
        ..Behavior::default()
    });
    let mut command = cadocs(dir.path());
    with_relay(&mut command, &relay);

    let output = command
        .args(["test-mail", "ada@example.net"])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("no such user"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn test_mail_says_when_mail_is_off() {
    let dir = TempDir::new().unwrap();
    let output = cadocs(dir.path())
        .args(["test-mail", "ada@example.net"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(
        stderr(&output).contains("CADOCS_SMTP_HOST"),
        "{}",
        stderr(&output)
    );
}

/// Reads the log until a line about the relay, which the server writes once it has tried it.
fn relay_line(mut command: Command) -> String {
    let mut server = Server(command.stdout(Stdio::piped()).spawn().unwrap());
    let stdout = server.0.stdout.take().unwrap();
    BufReader::new(stdout)
        .lines()
        .map(|line| line.unwrap())
        .find(|line| line.contains("mail relay"))
        .expect("a line about the mail relay")
}

#[test]
fn the_server_says_whether_its_relay_answers_and_starts_either_way() {
    let dir = TempDir::new().unwrap();
    let (_runtime, relay) = fake_relay(Behavior::default());
    let mut command = cadocs(dir.path());
    with_relay(&mut command, &relay);
    let line = relay_line(command);
    assert!(line.contains("INFO") && line.contains("answers"), "{line}");

    // Nothing listens on port 1. Mail is down, the wiki is not.
    let dir = TempDir::new().unwrap();
    let mut command = cadocs(dir.path());
    with_relay(&mut command, &relay);
    command.env("CADOCS_SMTP_PORT", "1");
    let line = relay_line(command);
    assert!(
        line.contains("WARN") && line.contains("127.0.0.1:1"),
        "{line}"
    );
    let mut command = cadocs(dir.path());
    command.env("CADOCS_DATA_DIR", TempDir::new().unwrap().path());
    with_relay(&mut command, &relay);
    command.env("CADOCS_SMTP_PORT", "1");
    start(command);
}

/// Reads the log until the server listens, and returns every line before.
fn lines_until_listening(command: &mut Command) -> (Server, Vec<String>) {
    let mut server = Server(command.stdout(Stdio::piped()).spawn().unwrap());
    let stdout = server.0.stdout.take().unwrap();
    let mut lines = Vec::new();
    for line in BufReader::new(stdout).lines() {
        let line = line.unwrap();
        let done = line.contains("listening");
        lines.push(line);
        if done {
            return (server, lines);
        }
    }
    panic!("the server stopped before it listened: {lines:?}");
}

#[test]
fn the_server_says_when_mail_is_off_and_a_second_factor_is_on() {
    let dir = TempDir::new().unwrap();
    seed(dir.path());
    let (_server, lines) = lines_until_listening(&mut cadocs(dir.path()));
    assert!(!lines.iter().any(|line| line.contains("second factor")));
    drop(_server);

    run_sql(dir.path(), "UPDATE users SET email_2fa = 1");
    let (_server, lines) = lines_until_listening(&mut cadocs(dir.path()));
    let line = lines.iter().find(|line| line.contains("second factor"));
    let line = line.expect("a line about the second factor");
    assert!(
        line.contains("WARN") && line.contains("1 account"),
        "{line}"
    );
}
