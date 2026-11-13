//! The tools of the MVP benchmark: accounts, tree and index in the seed, events sockets in the
//! load, request latencies and login peaks, against the real server over real sockets.

use std::sync::Arc;
use std::time::Duration;

use cadocs_bench::http::{http_origin, request};
use cadocs_bench::load::{LoadConfig, LoadReport, run};
use cadocs_bench::login::{LoginConfig, login_peak};
use cadocs_bench::probe::{LatencyConfig, latencies, search};
use cadocs_bench::seed::{
    SESSION_TOKEN, SESSION_TOKEN_HASH, Seed, WORKSPACE, doc_id, parent_of, seed, session_token,
    token_hash,
};
use cadocs_collab::{CollabConfig, DocRegistry, PageAuthorizer};
use cadocs_files::LocalFileStore;
use cadocs_server::client_ip::TrustedProxies;
use cadocs_server::config::{AttachmentLimits, HttpConfig, PublicUrl, SocketConfig};
use cadocs_server::{AppState, serve};
use cadocs_store::{JobStore, PageStore, SqliteStore};
use rust_embed::Embed;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;

#[derive(Embed)]
#[folder = "tests/fixtures"]
struct NoAssets;

const ORIGIN: &str = "http://127.0.0.1:3000";

struct Server {
    url: String,
    store: Arc<SqliteStore>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    _dir: TempDir,
}

impl Server {
    async fn stop(self) {
        self.stop.send(()).unwrap();
        self.task.await.unwrap().unwrap();
    }
}

async fn start(seeded: &Seed, trusted_proxies: &str) -> Server {
    let dir = TempDir::new().unwrap();
    let store = Arc::new(
        SqliteStore::open(&dir.path().join("cadocs.db"))
            .await
            .unwrap(),
    );
    seed(store.as_ref(), seeded).await.unwrap();
    let registry = DocRegistry::new(store.clone(), CollabConfig::default());
    let state = AppState::new(
        registry,
        store.clone(),
        Arc::new(PageAuthorizer::new(store.clone())),
        SocketConfig::default(),
        HttpConfig {
            public_url: PublicUrl::parse(ORIGIN).unwrap(),
            trusted_proxies: TrustedProxies::parse(trusted_proxies).unwrap(),
            session_lifetime: Duration::from_secs(60),
        },
        Arc::new(LocalFileStore::open(dir.path()).unwrap()),
        AttachmentLimits::default(),
    );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("ws://{}", listener.local_addr().unwrap());
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(serve::<_, _, NoAssets>(listener, state, async {
        let _ = stopped.await;
    }));
    Server {
        url,
        store,
        stop,
        task,
        _dir: dir,
    }
}

#[test]
fn the_tree_hangs_each_page_under_an_earlier_one() {
    assert_eq!(parent_of(0, Some(3)), None);
    assert_eq!(parent_of(2, Some(3)), None);
    assert_eq!(parent_of(3, Some(3)), Some(0));
    assert_eq!(parent_of(5, Some(3)), Some(0));
    assert_eq!(parent_of(6, Some(3)), Some(1));
    assert_eq!(parent_of(9, Some(3)), Some(2));
    assert_eq!(parent_of(9, None), None);
    for index in 0..1_000 {
        if let Some(parent) = parent_of(index, Some(20)) {
            assert!(parent < index);
        }
    }
}

#[test]
fn each_user_has_a_session() {
    assert_eq!(session_token(0), SESSION_TOKEN);
    assert_eq!(token_hash(SESSION_TOKEN), SESSION_TOKEN_HASH);
    assert_eq!(
        token_hash("x"),
        cadocs_server::session::token_hash("x"),
        "the server must find the seeded sessions"
    );
    assert_ne!(session_token(1), session_token(2));
}

#[test]
fn an_origin_follows_the_socket_address() {
    assert_eq!(http_origin("ws://127.0.0.1:3917"), "http://127.0.0.1:3917");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_seed_builds_a_tree_with_titles_and_empty_pages() {
    let seeded = Seed {
        pages: 7,
        fanout: Some(3),
        ..Seed::docs(2)
    };
    let server = start(&seeded, "").await;
    let owner = "bench-user";

    let child = server.store.page(&doc_id(6), owner).await.unwrap().unwrap();
    assert_eq!(child.parent_id.as_deref(), Some(doc_id(1).as_str()));
    assert_eq!(child.title, "", "a page past `docs` is empty");
    let first = server.store.page(&doc_id(0), owner).await.unwrap().unwrap();
    assert_eq!(first.parent_id, None);
    assert_eq!(first.title, "Bench page 1", "the title a flush would copy");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn seeded_users_edit_with_their_events_socket_open() {
    let seeded = Seed {
        users: 3,
        ..Seed::docs(3)
    };
    let server = start(&seeded, "").await;
    let config = LoadConfig {
        url: server.url.clone(),
        docs: 3,
        clients_per_doc: 1,
        users: 3,
        events: true,
        updates_per_sec: 10,
        awareness_per_sec: 5,
        duration: Duration::from_secs(1),
        hold: Duration::from_millis(200),
    };

    let report = run(&config, async { Ok(()) }).await.unwrap();

    assert_eq!(report.clients, 3);
    assert_eq!(report.events, 3, "{report:?}");
    server.stop().await;
}

#[test]
fn a_run_short_of_events_sockets_is_refused() {
    let config = LoadConfig {
        url: "ws://unused".to_owned(),
        docs: 2,
        clients_per_doc: 1,
        users: 2,
        events: true,
        updates_per_sec: 2,
        awareness_per_sec: 1,
        duration: Duration::from_secs(10),
        hold: Duration::ZERO,
    };
    let full = LoadReport {
        clients: 2,
        updates_sent: 40,
        awareness_sent: 20,
        updates_received: 0,
        events: 2,
    };
    full.check(&config).unwrap();
    let lost = LoadReport { events: 1, ..full };
    assert!(lost.check(&config).is_err(), "an events socket is missing");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_login_peak_needs_one_address_per_user() {
    let seeded = Seed {
        users: 12,
        ..Seed::docs(0)
    };
    let server = start(&seeded, "127.0.0.1").await;
    let config = |forwarded| LoginConfig {
        url: server.url.clone(),
        origin: ORIGIN.to_owned(),
        users: 12,
        forwarded,
    };

    let peak = login_peak(&config(true)).await.unwrap();
    assert_eq!(peak.ok, 12, "{peak:?}");
    assert_eq!(peak.latencies.len(), 12);

    // From one address, the burst of password guesses ends at 10.
    let shared = login_peak(&config(false)).await.unwrap();
    assert_eq!(shared.ok, 10, "{shared:?}");
    assert_eq!(shared.refused, 2, "{shared:?}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_seed_indexes_pages_for_search() {
    let seeded = Seed {
        index: true,
        ..Seed::docs(3)
    };
    let server = start(&seeded, "").await;
    let cookie = format!("session={SESSION_TOKEN}");
    let search = |q: &str| format!("/api/v1/workspaces/{WORKSPACE}/search?q={q}");

    // Titles are `Bench page <seed + index>`, and the seed starts at 1.
    let found = request(
        &server.url,
        "GET",
        &search("2"),
        &[("cookie", &cookie)],
        b"",
    )
    .await
    .unwrap();
    assert_eq!(found.status, 200);
    let body = String::from_utf8_lossy(&found.body);
    assert!(body.contains(&doc_id(1)), "{body}");
    assert!(!body.contains(&doc_id(2)), "{body}");
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn latencies_cover_each_request_of_a_page_view_and_a_search() {
    let seeded = Seed {
        pages: 12,
        fanout: Some(3),
        index: true,
        ..Seed::docs(12)
    };
    let server = start(&seeded, "").await;
    let config = LatencyConfig {
        url: server.url.clone(),
        pages: 12,
        fanout: 3,
        samples: 5,
        seed: 1,
    };

    let measured = latencies(&config).await.unwrap();

    let names: Vec<_> = measured.iter().map(|latency| latency.name).collect();
    assert_eq!(
        names,
        [
            "page",
            "open",
            "tree-root",
            "tree-children",
            "search-common",
            "search-rare"
        ]
    );
    for latency in &measured {
        assert_eq!(latency.samples.len(), 5, "{}", latency.name);
        assert!(latency.quantile(0.95) <= latency.max());
    }
    server.stop().await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_probe_search_matches_whole_words_only() {
    let seeded = Seed {
        index: true,
        ..Seed::docs(2)
    };
    let server = start(&seeded, "").await;

    let (_, whole) = search(&server.url, "server").await.unwrap();
    assert_eq!(whole, 2, "every page holds the word");
    // As typed, the last word would be a prefix of `server`.
    let (_, part) = search(&server.url, "serv").await.unwrap();
    assert_eq!(part, 0);
    server.stop().await;
}

/// Each stored edit queues a reindex and a revision. Left in the database, they would run on
/// every server the benchmark starts, during what it measures as idle.
#[tokio::test(flavor = "multi_thread")]
async fn the_seed_leaves_no_job_behind() {
    let dir = TempDir::new().unwrap();
    let store = SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap();
    seed(&store, &Seed::docs(2)).await.unwrap();

    assert!(store.claim_job(i64::MAX).await.unwrap().is_none());
}
