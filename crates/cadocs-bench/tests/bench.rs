//! The generator, the seed and the load client, against the real server over a real socket.

use std::sync::Arc;
use std::time::Duration;

use cadocs_bench::document::{EDITS, body_text, generate};
use cadocs_bench::load::{LoadConfig, LoadReport, peers, run};
use cadocs_bench::seed::{COMPACT_UPDATES, doc_id, seed};
use cadocs_collab::{CollabConfig, DocRegistry, Permissive};
use cadocs_server::client_ip::TrustedProxies;
use cadocs_server::config::{HttpConfig, PublicUrl, SocketConfig};
use cadocs_server::{AppState, serve};
use cadocs_store::{DocStore, SqliteStore};
use rust_embed::Embed;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::oneshot;
use yrs::updates::decoder::Decode;
use yrs::{Doc, GetString, Transact, Update};

#[derive(Embed)]
#[folder = "tests/fixtures"]
struct NoAssets;

fn replay(updates: &[Vec<u8>]) -> Doc {
    let doc = Doc::new();
    let mut txn = doc.transact_mut();
    for update in updates {
        txn.apply_update(Update::decode_v1(update).unwrap())
            .unwrap();
    }
    drop(txn);
    doc
}

#[test]
fn the_document_matches_the_scenario() {
    let bench = generate(1);
    assert_eq!(bench.updates.len(), EDITS);

    let text = body_text(&bench.doc);
    let bytes = text.len();
    let words = text.split_whitespace().count();
    assert!((19_000..=21_000).contains(&bytes), "{bytes} bytes");
    assert!((2_800..=3_200).contains(&words), "{words} words");
    assert!(text.lines().count() > 10, "a page has paragraphs");
}

#[test]
fn the_updates_rebuild_the_document() {
    let bench = generate(2);
    let rebuilt = replay(&bench.updates);
    assert_eq!(body_text(&rebuilt), body_text(&bench.doc));
    assert!(
        !rebuilt
            .get_or_insert_text("title")
            .get_string(&rebuilt.transact())
            .is_empty()
    );
}

#[test]
fn a_seed_gives_one_document() {
    assert_eq!(generate(3).updates, generate(3).updates);
    assert_ne!(generate(3).updates, generate(4).updates);
}

async fn store(dir: &TempDir) -> SqliteStore {
    SqliteStore::open(&dir.path().join("cadocs.db"))
        .await
        .unwrap()
}

fn stored_text(stored: &cadocs_store::StoredDoc) -> String {
    let mut updates: Vec<Vec<u8>> = stored.snapshot.iter().map(|s| s.state.clone()).collect();
    updates.extend(stored.updates.iter().map(|u| u.data.clone()));
    body_text(&replay(&updates))
}

#[tokio::test]
async fn seeding_follows_the_compaction_of_a_room() {
    let dir = TempDir::new().unwrap();
    let store = store(&dir).await;
    seed(&store, 2, 7, Some(COMPACT_UPDATES)).await.unwrap();

    for index in 0..2 {
        let stored = store.load(&doc_id(index)).await.unwrap();
        // A room compacts once it stores more than the threshold: at 501, 1002 and 1503 here.
        assert_eq!(stored.snapshot.as_ref().unwrap().upto_seq, 1_503);
        assert_eq!(stored.updates.len(), EDITS - 1_503);
        assert_eq!(stored.last_seq(), EDITS as i64);
        assert_eq!(
            stored_text(&stored),
            body_text(&generate(7 + index as u64).doc)
        );
    }
}

#[tokio::test]
async fn seeding_can_keep_the_whole_log() {
    let dir = TempDir::new().unwrap();
    let store = store(&dir).await;
    seed(&store, 1, 7, None).await.unwrap();

    let stored = store.load(&doc_id(0)).await.unwrap();
    assert!(stored.snapshot.is_none());
    assert_eq!(stored.updates.len(), EDITS);
}

struct Server {
    url: String,
    store: Arc<SqliteStore>,
    stop: oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
}

async fn start(dir: &TempDir) -> Server {
    let store = Arc::new(self::store(dir).await);
    seed(store.as_ref(), 2, 7, Some(COMPACT_UPDATES))
        .await
        .unwrap();
    let collab = CollabConfig {
        flush_interval: Duration::from_millis(10),
        ..Default::default()
    };
    let registry = DocRegistry::new(store.clone(), collab);
    let state = AppState::new(
        registry,
        store.clone(),
        Arc::new(Permissive),
        SocketConfig::default(),
        HttpConfig {
            public_url: PublicUrl::parse("http://127.0.0.1:3000").unwrap(),
            trusted_proxies: TrustedProxies::default(),
        },
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
    }
}

fn config(url: &str) -> LoadConfig {
    LoadConfig {
        url: url.to_owned(),
        docs: 2,
        clients_per_doc: 2,
        updates_per_sec: 20,
        awareness_per_sec: 10,
        duration: Duration::from_secs(1),
        hold: Duration::from_millis(200),
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn the_load_reaches_the_server_and_the_other_client() {
    let dir = TempDir::new().unwrap();
    let server = start(&dir).await;
    let before = server.store.load(&doc_id(0)).await.unwrap().last_seq();

    let report = run(&config(&server.url), async { Ok(()) }).await.unwrap();

    assert_eq!(report.clients, 4);
    // Four clients for one second at 20 updates and 10 awareness updates per second.
    assert!((60..=84).contains(&report.updates_sent), "{report:?}");
    assert!((30..=44).contains(&report.awareness_sent), "{report:?}");
    // Each update reaches the one other client of its room.
    assert!(
        report.updates_received >= report.updates_sent * 9 / 10,
        "{report:?}"
    );

    server.stop.send(()).unwrap();
    server.task.await.unwrap().unwrap();
    let after = server.store.load(&doc_id(0)).await.unwrap();
    assert!(after.last_seq() > before, "the edits must be stored");
    let text = stored_text(&after);
    assert!(
        text.len() > body_text(&generate(7).doc).len(),
        "the edits must add text"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn then_runs_while_clients_are_connected_and_after_the_hold() {
    let dir = TempDir::new().unwrap();
    let server = start(&dir).await;
    let started = std::time::Instant::now();
    let probe = server.url.clone();

    run(&config(&server.url), async move {
        assert!(started.elapsed() >= Duration::from_millis(1_200));
        // The sockets are open, so a new client of the same room still finds it loaded and sees
        // the awareness of the others at once.
        let others = peers(&probe, &doc_id(0)).await?;
        assert_eq!(others, 2);
        Ok(())
    })
    .await
    .unwrap();

    server.stop.send(()).unwrap();
    server.task.await.unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lost_socket_fails_the_run() {
    let dir = TempDir::new().unwrap();
    let server = start(&dir).await;
    let mut config = config(&server.url);
    config.duration = Duration::from_secs(30);

    let probe = server.url.clone();
    let load = tokio::spawn(async move { run(&config, async { Ok(()) }).await });
    tokio::time::sleep(Duration::from_millis(500)).await;
    // Clients start together once all are connected, so this is a loss during the load.
    let running = peers(&probe, &doc_id(0)).await.unwrap();
    assert!(running >= 1);
    server.stop.send(()).unwrap();

    let result = tokio::time::timeout(Duration::from_secs(5), load)
        .await
        .unwrap()
        .unwrap();
    assert!(result.is_err(), "{result:?}");
    server.task.await.unwrap().unwrap();
}

#[test]
fn a_run_short_of_the_scenario_is_refused() {
    let config = LoadConfig {
        docs: 100,
        duration: Duration::from_secs(60),
        updates_per_sec: 2,
        awareness_per_sec: 1,
        ..config("ws://unused")
    };
    let full = LoadReport {
        clients: 200,
        updates_sent: 24_000,
        awareness_sent: 12_000,
        updates_received: 24_000,
    };
    full.check(&config).unwrap();

    let slow = LoadReport {
        updates_sent: 20_000,
        updates_received: 20_000,
        ..full
    };
    assert!(slow.check(&config).is_err(), "too few updates sent");
    let quiet = LoadReport {
        awareness_sent: 9_000,
        ..full
    };
    assert!(
        quiet.check(&config).is_err(),
        "too few awareness updates sent"
    );
    let dropped = LoadReport {
        updates_received: 20_000,
        ..full
    };
    assert!(
        dropped.check(&config).is_err(),
        "updates did not reach the other clients"
    );
    let few = LoadReport {
        clients: 199,
        ..full
    };
    assert!(few.check(&config).is_err(), "a client is missing");
}

#[test]
fn clients_do_not_share_an_id_with_an_author() {
    let authors: Vec<_> = (1..50).map(|seed| generate(seed).doc.client_id()).collect();
    for index in 0..50 {
        assert!(!authors.contains(&cadocs_bench::load::load_client_id(index)));
    }
}
