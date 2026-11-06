//! The `cadocs` binary itself: what can only go wrong between `main` and the library.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Output, Stdio};

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
