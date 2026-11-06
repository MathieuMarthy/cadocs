//! The server over real sockets, with `yrs` standing in for the client. The real `yjs` client is
//! tested in step 4.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use axum::body::Body;
use axum::http::{Request, StatusCode, header};
use cadocs_collab::{Authorizer, CollabConfig, DocRegistry, Permissive};
use cadocs_core::permissions::SocketMode;
use cadocs_server::config::SocketConfig;
use cadocs_server::{AppState, router, serve};
use cadocs_store::{DocStore, Snapshot, SqliteStore, StoreError, StoredDoc, StoredUpdate};
use futures_util::{SinkExt, StreamExt};
use rust_embed::Embed;
use tempfile::TempDir;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};
use tower::ServiceExt;
use yrs::sync::{Message as YMessage, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, ReadTxn, StateVector, Text, Transact, Update};

type Client = WebSocketStream<MaybeTlsStream<TcpStream>>;

#[derive(Embed)]
#[folder = "tests/fixtures"]
struct Fixtures;

/// A SQLite store whose writes or pings can be made to fail.
#[derive(Clone)]
struct FlakyStore {
    inner: SqliteStore,
    fail_writes: Arc<AtomicBool>,
    fail_ping: Arc<AtomicBool>,
}

impl DocStore for FlakyStore {
    async fn ping(&self) -> Result<(), StoreError> {
        if self.fail_ping.load(Ordering::SeqCst) {
            return Err(sqlx::Error::PoolTimedOut.into());
        }
        self.inner.ping().await
    }

    async fn load(&self, doc_id: &str) -> Result<StoredDoc, StoreError> {
        self.inner.load(doc_id).await
    }

    async fn append_updates(
        &self,
        doc_id: &str,
        updates: &[StoredUpdate],
    ) -> Result<(), StoreError> {
        if self.fail_writes.load(Ordering::SeqCst) {
            return Err(sqlx::Error::PoolTimedOut.into());
        }
        self.inner.append_updates(doc_id, updates).await
    }

    async fn compact(&self, doc_id: &str, snapshot: &Snapshot) -> Result<(), StoreError> {
        self.inner.compact(doc_id, snapshot).await
    }
}

#[derive(Clone, Copy)]
struct Fixed(Option<SocketMode>);

impl Authorizer for Fixed {
    async fn authorize(&self, _doc_id: &str) -> Option<SocketMode> {
        self.0
    }
}

fn collab() -> CollabConfig {
    CollabConfig {
        flush_interval: Duration::from_millis(10),
        ..CollabConfig::default()
    }
}

async fn state<A: Authorizer>(
    dir: &TempDir,
    collab: CollabConfig,
    socket: SocketConfig,
    authorizer: A,
) -> AppState<FlakyStore, A> {
    let inner = SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap();
    let store = Arc::new(FlakyStore {
        inner,
        fail_writes: Arc::default(),
        fail_ping: Arc::default(),
    });
    let registry = DocRegistry::new(store.clone(), collab);
    AppState::new(registry, store, Arc::new(authorizer), socket)
}

struct Running {
    addr: std::net::SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<anyhow::Result<()>>,
}

impl Running {
    fn url(&self, doc: &str) -> String {
        format!("ws://{}/ws/{doc}", self.addr)
    }

    async fn client(&self, doc: &str) -> Client {
        connect_async(self.url(doc)).await.unwrap().0
    }

    async fn stop(self) {
        self.stop.send(()).unwrap();
        timeout(Duration::from_secs(10), self.task)
            .await
            .expect("the server must stop")
            .unwrap()
            .unwrap();
    }
}

async fn start<A: Authorizer>(state: AppState<FlakyStore, A>) -> Running {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (stop, stopped) = oneshot::channel();
    let task = tokio::spawn(serve::<_, _, Fixtures>(listener, state, async {
        let _ = stopped.await;
    }));
    Running { addr, stop, task }
}

async fn start_default(dir: &TempDir) -> Running {
    start(state(dir, collab(), SocketConfig::default(), Permissive).await).await
}

async fn get<A: Authorizer>(
    state: AppState<FlakyStore, A>,
    path: &str,
) -> axum::response::Response {
    router::<_, _, Fixtures>(state)
        .oneshot(Request::get(path).body(Body::empty()).unwrap())
        .await
        .unwrap()
}

fn sync_step1(doc: &Doc) -> Vec<u8> {
    let state_vector = doc.transact().state_vector();
    YMessage::Sync(SyncMessage::SyncStep1(state_vector)).encode_v1()
}

fn update_frame(text: &str) -> Vec<u8> {
    let doc = Doc::new();
    let title = doc.get_or_insert_text("title");
    let mut txn = doc.transact_mut();
    title.push(&mut txn, text);
    let update = txn.encode_state_as_update_v1(&StateVector::default());
    YMessage::Sync(SyncMessage::Update(update)).encode_v1()
}

/// Applies binary frames until `title` reads `expected`.
async fn wait_for_title(client: &mut Client, doc: &Doc, expected: &str) {
    let title = doc.get_or_insert_text("title");
    let result = timeout(Duration::from_secs(5), async {
        loop {
            let Message::Binary(frame) = client.next().await.unwrap().unwrap() else {
                continue;
            };
            if let Ok(YMessage::Sync(
                SyncMessage::SyncStep2(update) | SyncMessage::Update(update),
            )) = YMessage::decode_v1(&frame)
            {
                doc.transact_mut()
                    .apply_update(Update::decode_v1(&update).unwrap())
                    .unwrap();
            }
            if title.get_string(&doc.transact()) == expected {
                return;
            }
        }
    })
    .await;
    assert!(result.is_ok(), "title never became {expected:?}");
}

/// Reads until the socket closes, and returns the close code.
async fn close_code(client: &mut Client) -> Option<CloseCode> {
    timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.code),
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return None,
            }
        }
    })
    .await
    .expect("the socket must close")
}

async fn refused_status(request: impl IntoClientRequest + Unpin) -> StatusCode {
    match connect_async(request).await {
        Err(WsError::Http(response)) => response.status(),
        Err(error) => panic!("unexpected error: {error}"),
        Ok(_) => panic!("the upgrade must be refused"),
    }
}

#[tokio::test]
async fn health_is_ok_on_a_working_server() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Permissive).await;
    assert_eq!(get(state, "/health").await.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_fails_when_the_database_does_not_answer() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Permissive).await;
    state.store.fail_ping.store(true, Ordering::SeqCst);
    assert_eq!(
        get(state, "/health").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
}

#[tokio::test]
async fn sqlite_ping_reads_the_database() {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cadocs.db");
    let store = SqliteStore::open(&path).await.unwrap();
    store.ping().await.unwrap();

    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cadocs.db");
    let store = SqliteStore::open(&path).await.unwrap();
    // The first ping is what opens the read connection, which now finds no database.
    for suffix in ["", "-wal", "-shm"] {
        let _ = std::fs::remove_file(format!("{}{suffix}", path.display()));
    }
    assert!(store.ping().await.is_err());
}

#[tokio::test]
async fn health_fails_while_flushes_fail() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Permissive).await;
    state.store.fail_writes.store(true, Ordering::SeqCst);
    let server = start(state.clone()).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(update_frame("lost")))
        .await
        .unwrap();

    let failing = timeout(Duration::from_secs(5), async {
        while state.registry.healthy() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(failing.is_ok(), "the flush failure must be reported");
    assert_eq!(
        get(state.clone(), "/health").await.status(),
        StatusCode::SERVICE_UNAVAILABLE
    );
    state.store.fail_writes.store(false, Ordering::SeqCst);
    server.stop().await;
}

#[tokio::test]
async fn the_root_serves_the_page_with_its_security_headers() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Permissive).await;
    let response = get(state, "/").await;
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers();
    assert_eq!(headers[header::CONTENT_TYPE], "text/html; charset=utf-8");
    assert_eq!(headers[header::CACHE_CONTROL], "no-cache");
    assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff");
    assert_eq!(headers[header::REFERRER_POLICY], "same-origin");
    assert_eq!(
        headers[header::CONTENT_SECURITY_POLICY],
        "default-src 'self'; img-src 'self' data:; style-src 'self' 'unsafe-inline'; \
         connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'"
    );
}

#[tokio::test]
async fn hashed_assets_are_cached_for_good() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Permissive).await;

    let script = get(state.clone(), "/assets/app-abc123.js").await;
    assert_eq!(script.status(), StatusCode::OK);
    assert_eq!(
        script.headers()[header::CONTENT_TYPE],
        "text/javascript; charset=utf-8"
    );
    assert_eq!(
        script.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );

    let font = get(state, "/assets/font-abc123.woff2").await;
    assert_eq!(font.headers()[header::CONTENT_TYPE], "font/woff2");
}

#[tokio::test]
async fn unknown_paths_and_methods_are_refused() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Permissive).await;
    assert_eq!(
        get(state.clone(), "/missing.js").await.status(),
        StatusCode::NOT_FOUND
    );
    let post = router::<_, _, Fixtures>(state)
        .oneshot(Request::post("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
}

#[tokio::test]
async fn two_clients_edit_one_document() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut alice = server.client("doc").await;
    let mut bob = server.client("doc").await;

    alice
        .send(Message::binary(update_frame("hello")))
        .await
        .unwrap();

    wait_for_title(&mut bob, &Doc::new(), "hello").await;
    server.stop().await;
}

#[tokio::test]
async fn edits_survive_a_server_restart() {
    let dir = TempDir::new().unwrap();
    // No periodic flush will run, so only the flush at shutdown can save the edit.
    let collab = CollabConfig {
        flush_interval: Duration::from_secs(3600),
        ..collab()
    };
    let server = start(state(&dir, collab, SocketConfig::default(), Permissive).await).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(update_frame("kept")))
        .await
        .unwrap();
    // The frame must reach the room before the shutdown does.
    client
        .send(Message::binary(sync_step1(&Doc::new())))
        .await
        .unwrap();
    wait_for_title(&mut client, &Doc::new(), "kept").await;
    server.stop().await;

    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    let doc = Doc::new();
    client
        .send(Message::binary(sync_step1(&doc)))
        .await
        .unwrap();
    wait_for_title(&mut client, &doc, "kept").await;
    server.stop().await;
}

#[tokio::test]
async fn shutdown_closes_sockets_with_going_away() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    server.stop().await;
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Away));
}

#[tokio::test]
async fn a_stalled_http_client_does_not_block_shutdown() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    let mut stalled = TcpStream::connect(server.addr).await.unwrap();
    stalled.write_all(b"GET / HTTP/1.1\r\nHo").await.unwrap();
    sleep(Duration::from_millis(50)).await;

    server.stop().await;

    // The sockets must hear about it, whatever the stalled connection does.
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Away));
    drop(stalled);
}

#[tokio::test]
async fn a_close_from_the_client_is_answered() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    client.send(Message::Close(None)).await.unwrap();

    let answered = timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Close(_))) => return true,
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return false,
            }
        }
    })
    .await
    .expect("the socket must close");
    assert!(answered, "the close handshake must complete");
    server.stop().await;
}

#[tokio::test]
async fn a_text_frame_closes_the_socket() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    client.send(Message::text("hello")).await.unwrap();
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Protocol));
    server.stop().await;
}

#[tokio::test]
async fn a_malformed_frame_closes_the_socket() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(vec![0xff, 0xff, 0xff]))
        .await
        .unwrap();
    assert_eq!(close_code(&mut client).await, Some(CloseCode::Protocol));
    server.stop().await;
}

#[tokio::test]
async fn a_frame_over_the_limit_closes_the_socket_with_1009() {
    let dir = TempDir::new().unwrap();
    let collab = CollabConfig {
        max_frame_bytes: 1024,
        ..collab()
    };
    let server = start(state(&dir, collab, SocketConfig::default(), Permissive).await).await;

    // Just past the limit, and far past it: both must end the same way.
    for size in [1025, 1024 * 1024] {
        let mut client = server.client("doc").await;
        client.send(Message::binary(vec![0; size])).await.unwrap();
        assert_eq!(
            close_code(&mut client).await,
            Some(CloseCode::Size),
            "{size}"
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn sockets_past_the_limit_get_503_before_the_upgrade() {
    let dir = TempDir::new().unwrap();
    let collab = CollabConfig {
        max_sockets: 1,
        ..collab()
    };
    let server = start(state(&dir, collab, SocketConfig::default(), Permissive).await).await;
    let _client = server.client("doc").await;
    assert_eq!(
        refused_status(server.url("doc")).await,
        StatusCode::SERVICE_UNAVAILABLE
    );
    server.stop().await;
}

#[tokio::test]
async fn a_foreign_origin_is_refused() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;

    let mut foreign = server.url("doc").into_client_request().unwrap();
    foreign
        .headers_mut()
        .insert(header::ORIGIN, "https://evil.example".parse().unwrap());
    assert_eq!(refused_status(foreign).await, StatusCode::FORBIDDEN);

    // DNS rebinding: a hostile name pointed at this server, so Origin and Host agree.
    let mut rebound = server.url("doc").into_client_request().unwrap();
    let headers = rebound.headers_mut();
    headers.insert(header::ORIGIN, "http://evil.example".parse().unwrap());
    headers.insert(header::HOST, "evil.example".parse().unwrap());
    assert_eq!(refused_status(rebound).await, StatusCode::FORBIDDEN);

    // Loopback names work with any port.
    let mut named = server.url("doc").into_client_request().unwrap();
    let port = server.addr.port();
    let headers = named.headers_mut();
    let localhost = format!("localhost:{port}");
    headers.insert(header::HOST, localhost.parse().unwrap());
    let origin = format!("http://localhost:{port}");
    headers.insert(header::ORIGIN, origin.parse().unwrap());
    connect_async(named).await.expect("localhost is allowed");

    let mut same = server.url("doc").into_client_request().unwrap();
    let origin = format!("http://{}", server.addr);
    same.headers_mut()
        .insert(header::ORIGIN, origin.parse().unwrap());
    connect_async(same).await.expect("same origin is allowed");

    server.stop().await;
}

#[tokio::test]
async fn an_invalid_document_id_is_not_found() {
    let dir = TempDir::new().unwrap();
    let server = start_default(&dir).await;
    let long = "a".repeat(65);
    for doc in ["bad%20id", "bad.id", long.as_str()] {
        assert_eq!(
            refused_status(server.url(doc)).await,
            StatusCode::NOT_FOUND,
            "{doc}"
        );
    }
    server.stop().await;
}

#[tokio::test]
async fn a_refused_document_is_not_found() {
    let dir = TempDir::new().unwrap();
    let state = state(&dir, collab(), SocketConfig::default(), Fixed(None)).await;
    let server = start(state).await;
    assert_eq!(
        refused_status(server.url("doc")).await,
        StatusCode::NOT_FOUND
    );
    server.stop().await;
}

#[tokio::test]
async fn the_authorizer_sets_the_mode_of_the_socket() {
    let dir = TempDir::new().unwrap();
    let authorizer = Fixed(Some(SocketMode::ReadOnly));
    let server = start(state(&dir, collab(), SocketConfig::default(), authorizer).await).await;
    let mut client = server.client("doc").await;
    client
        .send(Message::binary(update_frame("refused")))
        .await
        .unwrap();

    let auth_frame = timeout(Duration::from_secs(5), async {
        loop {
            if let Message::Binary(frame) = client.next().await.unwrap().unwrap()
                && frame[0] == 2
            {
                return;
            }
        }
    })
    .await;
    assert!(auth_frame.is_ok(), "a read-only socket gets an Auth frame");
    server.stop().await;
}

#[tokio::test]
async fn the_server_pings_and_a_silent_client_is_closed() {
    let dir = TempDir::new().unwrap();
    let socket = SocketConfig {
        ping: Duration::from_millis(50),
        ping_timeout: Duration::from_millis(200),
    };
    let server = start(state(&dir, collab(), socket, Permissive).await).await;
    let mut client = server.client("doc").await;

    // Not polling the client means no pong goes out, so the server hears silence.
    sleep(Duration::from_millis(500)).await;

    let mut pinged = false;
    let code = timeout(Duration::from_secs(5), async {
        loop {
            match client.next().await {
                Some(Ok(Message::Ping(_))) => pinged = true,
                Some(Ok(Message::Close(frame))) => return frame.map(|frame| frame.code),
                Some(Ok(_)) => {}
                Some(Err(_)) | None => return None,
            }
        }
    })
    .await
    .expect("the socket must close");
    assert!(pinged);
    assert_eq!(code, Some(CloseCode::from(4408)));
    server.stop().await;
}

#[tokio::test]
async fn a_client_that_answers_pings_stays_connected() {
    let dir = TempDir::new().unwrap();
    let socket = SocketConfig {
        ping: Duration::from_millis(20),
        ping_timeout: Duration::from_millis(100),
    };
    let server = start(state(&dir, collab(), socket, Permissive).await).await;
    let mut client = server.client("doc").await;

    // Reading is what makes tungstenite answer the pings.
    let closed = timeout(Duration::from_millis(400), async {
        while let Some(Ok(message)) = client.next().await {
            if message.is_close() {
                return;
            }
        }
    })
    .await;
    assert!(closed.is_err(), "the socket must stay open");
    server.stop().await;
}
