//! A real server over real sockets, for the tests of the REST API. Each test binary uses part of it.
#![allow(dead_code)]

use std::sync::Arc;

use cadocs_collab::{DocRegistry, PageAuthorizer};
use cadocs_jobs::JobQueue;
use cadocs_server::config::Config;
use cadocs_server::{AppState, serve, start_jobs};
use cadocs_store::SqliteStore;
use rust_embed::Embed;
use serde_json::{Value, json};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{AssertSqlSafe, SqlitePool};
use tempfile::TempDir;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;

pub const ORIGIN: &str = "https://wiki.example.org";
pub const PASSWORD: &str = "correct horse battery";

#[derive(Embed)]
#[folder = "tests/fixtures"]
pub struct Fixtures;

pub struct Api {
    pub addr: std::net::SocketAddr,
    pub db: SqlitePool,
    pub store: Arc<SqliteStore>,
    pub registry: DocRegistry<SqliteStore>,
    pub queue: JobQueue<SqliteStore>,
    _stop: oneshot::Sender<()>,
    _dir: TempDir,
}

#[derive(Debug)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Value,
}

impl Reply {
    pub fn header(&self, name: &str) -> Option<&str> {
        let found = self.headers.iter().find(|(key, _)| key == name);
        found.map(|(_, value)| value.as_str())
    }

    pub fn code(&self) -> &str {
        self.body["error"]["code"].as_str().unwrap_or_default()
    }

    /// The `name=value` part of `Set-Cookie`.
    pub fn cookie(&self) -> String {
        let cookie = self.header("set-cookie").expect("a session cookie");
        cookie.split(';').next().unwrap().to_owned()
    }
}

/// A logged-in client.
#[derive(Clone)]
pub struct Login {
    pub cookie: String,
    pub csrf: String,
}

impl Login {
    pub fn from(reply: &Reply) -> Self {
        Self {
            cookie: reply.cookie(),
            csrf: reply.body["csrf_token"].as_str().unwrap().to_owned(),
        }
    }

    pub fn headers(&self) -> [(&str, &str); 2] {
        [("cookie", &self.cookie), ("x-csrf-token", &self.csrf)]
    }
}

pub async fn start_with(public_url: &str, proxies: &str) -> Api {
    start_tuned(public_url, proxies, &[]).await
}

/// `settings` are more environment variables.
pub async fn start_tuned(public_url: &str, proxies: &str, settings: &[(&str, &str)]) -> Api {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cadocs.db");
    let store = Arc::new(SqliteStore::open(&path).await.unwrap());
    let config = Config::from_lookup(|name| match name {
        "CADOCS_PUBLIC_URL" => Some(public_url.to_owned()),
        "CADOCS_TRUSTED_PROXIES" => Some(proxies.to_owned()),
        _ => settings
            .iter()
            .find(|(key, _)| *key == name)
            .map(|(_, value)| (*value).to_owned()),
    })
    .unwrap();
    let registry = DocRegistry::new(store.clone(), config.collab);
    let state = AppState::new(
        registry.clone(),
        store.clone(),
        Arc::new(PageAuthorizer::new(store.clone())),
        config.socket,
        config.http,
    );
    let queue = start_jobs(&state, config.jobs);

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel::<()>();
    tokio::spawn(serve::<_, _, Fixtures>(listener, state, async {
        let _ = stopped.await;
    }));
    let db = SqlitePool::connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    Api {
        addr,
        db,
        store,
        registry,
        queue,
        _stop: stop,
        _dir: dir,
    }
}

pub async fn start() -> Api {
    start_with(ORIGIN, "").await
}

impl Api {
    pub async fn raw(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: &[u8],
    ) -> Reply {
        let mut request = format!(
            "{method} /api/v1{path} HTTP/1.1\r\nhost: test\r\nconnection: close\r\n\
             content-length: {}\r\n",
            body.len()
        );
        for (name, value) in headers {
            request.push_str(&format!("{name}: {value}\r\n"));
        }
        request.push_str("\r\n");

        let mut stream = TcpStream::connect(self.addr).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.write_all(body).await.unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).await.unwrap();

        let split = answer.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&answer[..split]).into_owned();
        let mut lines = head.lines();
        let status = lines.next().unwrap().split(' ').nth(1).unwrap();
        let headers = lines
            .map(|line| line.split_once(": ").unwrap())
            .map(|(name, value)| (name.to_ascii_lowercase(), value.to_owned()))
            .collect();
        Reply {
            status: status.parse().unwrap(),
            headers,
            body: serde_json::from_slice(&answer[split + 4..]).unwrap_or(Value::Null),
        }
    }

    pub async fn send(
        &self,
        method: &str,
        path: &str,
        headers: &[(&str, &str)],
        body: Value,
    ) -> Reply {
        let mut headers = headers.to_vec();
        headers.push(("content-type", "application/json"));
        self.raw(method, path, &headers, body.to_string().as_bytes())
            .await
    }

    pub async fn setup(&self) -> Reply {
        let body = json!({
            "email": "Ada@Example.org",
            "name": "Ada",
            "password": PASSWORD,
            "workspace_name": "Acme Docs",
        });
        self.send("POST", "/auth/setup", &[], body).await
    }

    pub async fn login(&self, email: &str, password: &str) -> Reply {
        let body = json!({ "email": email, "password": password });
        self.send("POST", "/auth/login", &[], body).await
    }

    pub async fn session(&self, login: &Login) -> Reply {
        self.raw("GET", "/session", &[("cookie", &login.cookie)], b"")
            .await
    }

    pub async fn sql(&self, statement: &str) {
        sqlx::query(AssertSqlSafe(statement.to_owned()))
            .execute(&self.db)
            .await
            .unwrap();
    }

    pub async fn number(&self, statement: &str) -> i64 {
        sqlx::query_scalar(AssertSqlSafe(statement.to_owned()))
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    /// Another account with the password of the first one, and no workspace.
    pub async fn add_user(&self, id: &str, email: &str) {
        self.sql(&format!(
            "INSERT INTO users (id, email, name, password_hash, created_at, updated_at)
             SELECT '{id}', '{email}', 'Other', password_hash, 1, 1 FROM users LIMIT 1"
        ))
        .await;
    }

    pub async fn add_and_login(&self, id: &str, email: &str) -> Login {
        self.add_user(id, email).await;
        let reply = self.login(email, PASSWORD).await;
        assert_eq!(reply.status, 200, "{reply:?}");
        Login::from(&reply)
    }

    pub async fn get(&self, by: &Login, path: &str) -> Reply {
        self.raw("GET", path, &[("cookie", &by.cookie)], b"").await
    }

    pub async fn first_workspace(&self) -> String {
        sqlx::query_scalar("SELECT id FROM workspaces ORDER BY id LIMIT 1")
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    pub async fn add_member(&self, workspace: &str, id: &str, email: &str, role: &str) -> Login {
        let login = self.add_and_login(id, email).await;
        self.sql(&format!(
            "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
             VALUES ('{workspace}', '{id}', '{role}', 1)"
        ))
        .await;
        login
    }
}

pub async fn logged_in() -> (Api, Login) {
    let api = start().await;
    let reply = api.setup().await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let login = Login::from(&reply);
    (api, login)
}
