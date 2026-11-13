//! Reading a document outside a room (`docs/ARCHITECTURE.md`).

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread;
use std::time::Duration;

use cadocs_collab::{CollabConfig, CollabError, DocRegistry, ROOM_MIN_BYTES};
use cadocs_core::permissions::SocketMode::ReadWrite;
use cadocs_store::{DocEdit, DocStore, StoredUpdate};
use common::{DOC, MemStore, Peer, config, title};
use tokio::task::JoinSet;
use tokio::time::sleep;
use yrs::{Doc, Text, Transact};

const FLUSH: Duration = Duration::from_millis(600);

fn registry(store: &Arc<MemStore>, doc_reads: usize) -> DocRegistry<MemStore> {
    let config = CollabConfig {
        doc_reads,
        ..config()
    };
    DocRegistry::new(store.clone(), config)
}

async fn store_title(store: &MemStore, text: &str) {
    let doc = Doc::new();
    let field = doc.get_or_insert_text("title");
    let mut txn = doc.transact_mut();
    field.push(&mut txn, text);
    let data = txn.encode_update_v1();
    let update = StoredUpdate { seq: 1, data };
    let edit = DocEdit::default();
    store.append_updates(DOC, &[update], &edit).await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn a_document_without_a_room_is_read_from_the_store_and_leaves_nothing_behind() {
    let store = MemStore::new();
    store_title(&store, "stored").await;
    let appends = store.appends();
    let registry = registry(&store, 2);

    let read = registry.read(DOC, title).await.unwrap();

    assert_eq!(read, "stored");
    assert_eq!(registry.loaded_rooms(), 0, "a read registers no room");
    assert_eq!(store.appends(), appends, "a read writes nothing");
}

#[tokio::test(start_paused = true)]
async fn a_loaded_room_gives_its_unflushed_edits() {
    let store = MemStore::new();
    let registry = registry(&store, 2);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("not flushed").unwrap();
    let loads = store.loads();

    let read = registry.read(DOC, title).await.unwrap();

    assert_eq!(read, "not flushed");
    assert_eq!(store.appends(), 0, "the edit is still in the room");
    assert_eq!(store.loads(), loads, "the room answers, not the store");
}

#[tokio::test(start_paused = true)]
async fn a_read_during_an_unload_waits_for_what_it_stores() {
    let store = MemStore::new();
    let config = CollabConfig {
        room_idle: Duration::from_millis(100),
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    store.append_gate.hold();
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("last edit").unwrap();
    drop(alice);
    store.append_gate.entered().await;

    let read = tokio::spawn({
        let registry = registry.clone();
        async move { registry.read(DOC, title).await }
    });
    sleep(FLUSH).await;
    assert!(!read.is_finished(), "the edit is not stored yet");

    store.append_gate.release();

    assert_eq!(read.await.unwrap().unwrap(), "last edit");
}

#[tokio::test(start_paused = true)]
async fn a_read_after_an_unload_finds_what_it_stored() {
    let store = MemStore::new();
    let registry = registry(&store, 2);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("kept").unwrap();
    drop(alice);
    sleep(Duration::from_secs(62)).await;
    assert_eq!(registry.loaded_rooms(), 0);

    assert_eq!(registry.read(DOC, title).await.unwrap(), "kept");
}

#[tokio::test(start_paused = true)]
async fn a_full_registry_still_reads() {
    let store = MemStore::new();
    store_title(&store, "stored").await;
    let config = CollabConfig {
        room_memory: ROOM_MIN_BYTES,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let _other = registry
        .connect("other", ReadWrite, "ada", "s")
        .await
        .unwrap();

    assert_eq!(registry.read(DOC, title).await.unwrap(), "stored");
    assert_eq!(registry.loaded_rooms(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_load_failure_is_the_caller_s() {
    let store = MemStore::new();
    store.fail_load.store(true, Ordering::SeqCst);
    let registry = registry(&store, 2);

    let result = registry.read(DOC, title).await;

    assert!(matches!(result, Err(CollabError::Store(_))), "{result:?}");
}

#[tokio::test(start_paused = true)]
async fn the_bound_covers_the_load() {
    let store = MemStore::new();
    let registry = registry(&store, 1);
    store.load_gate.hold();
    let first = tokio::spawn({
        let registry = registry.clone();
        async move { registry.read(DOC, title).await }
    });
    store.load_gate.entered().await;

    let second = tokio::spawn({
        let registry = registry.clone();
        async move { registry.read(DOC, title).await }
    });
    sleep(FLUSH).await;
    assert_eq!(store.loads(), 1, "the second read waits before loading");

    store.load_gate.release();
    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    assert_eq!(store.loads(), 2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reads_at_once_never_pass_the_bound() {
    const BOUND: usize = 2;
    let store = MemStore::new();
    store_title(&store, "stored").await;
    let registry = registry(&store, BOUND);
    let running = Arc::new(AtomicUsize::new(0));
    let highest = Arc::new(AtomicUsize::new(0));

    let mut reads = JoinSet::new();
    for _ in 0..3 * BOUND {
        let registry = registry.clone();
        let running = running.clone();
        let highest = highest.clone();
        reads.spawn(async move {
            registry
                .read(DOC, move |_| {
                    let now = running.fetch_add(1, Ordering::SeqCst) + 1;
                    highest.fetch_max(now, Ordering::SeqCst);
                    thread::sleep(Duration::from_millis(20));
                    running.fetch_sub(1, Ordering::SeqCst);
                })
                .await
        });
    }
    while let Some(read) = reads.join_next().await {
        read.unwrap().unwrap();
    }

    assert_eq!(highest.load(Ordering::SeqCst), BOUND);
}

#[tokio::test(start_paused = true)]
async fn a_read_is_refused_once_the_server_closes() {
    let store = MemStore::new();
    let registry = registry(&store, 2);
    registry.shutdown().await.unwrap();

    let result = registry.read(DOC, title).await;

    assert!(
        matches!(result, Err(CollabError::ShuttingDown)),
        "{result:?}"
    );
}

/// Decoding a revision weighs as much as a read, so it takes a turn of the same bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bounded_work_shares_the_bound_of_reads() {
    const BOUND: usize = 2;
    let store = MemStore::new();
    store_title(&store, "stored").await;
    let registry = registry(&store, BOUND);
    let running = Arc::new(AtomicUsize::new(0));
    let highest = Arc::new(AtomicUsize::new(0));
    let work = |running: Arc<AtomicUsize>, highest: Arc<AtomicUsize>| {
        move || {
            let now = running.fetch_add(1, Ordering::SeqCst) + 1;
            highest.fetch_max(now, Ordering::SeqCst);
            thread::sleep(Duration::from_millis(20));
            running.fetch_sub(1, Ordering::SeqCst);
        }
    };

    let mut tasks = JoinSet::new();
    for i in 0..3 * BOUND {
        let registry = registry.clone();
        let job = work(running.clone(), highest.clone());
        tasks.spawn(async move {
            if i % 2 == 0 {
                registry.bounded(job).await
            } else {
                registry.read(DOC, move |_| job()).await
            }
        });
    }
    while let Some(task) = tasks.join_next().await {
        task.unwrap().unwrap();
    }

    assert_eq!(highest.load(Ordering::SeqCst), BOUND);
}
