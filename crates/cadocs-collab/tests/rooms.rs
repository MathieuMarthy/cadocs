mod common;

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use cadocs_collab::{
    CLOSE_FRAME_TOO_LARGE, CLOSE_GOING_AWAY, CLOSE_PAGE_GONE, CLOSE_PROTOCOL_ERROR,
    CLOSE_STORAGE_FAILING, CLOSE_TOO_SLOW, Close, CollabConfig, CollabError, DocRegistry, Outbound,
};
use cadocs_core::permissions::SocketMode::{ReadOnly, ReadWrite};
use cadocs_store::{DocEdit, DocStore, MAX_TITLE_CHARS, SqliteStore};
use common::{AUTHOR, DOC, MemStore, Peer, config, frame, rebuild, state_vector, title};
use sqlx::SqlitePool;
use sqlx::sqlite::SqliteConnectOptions;
use tokio::time::sleep;
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{Awareness, AwarenessUpdate, Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::{Array, Doc, ReadTxn, Text, Transact, XmlFragment, XmlTextPrelim};

const FLUSH: Duration = Duration::from_millis(600);
const IDLE: Duration = Duration::from_secs(61);

fn registry(store: &Arc<MemStore>) -> DocRegistry<MemStore> {
    DocRegistry::new(store.clone(), config())
}

async fn connect_title(registry: &DocRegistry<MemStore>) -> String {
    title(&Peer::join(registry, ReadWrite).await.doc)
}

// Sync

#[tokio::test(start_paused = true)]
async fn an_edit_reaches_the_other_client() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;
    let mut bob = Peer::join(&registry, ReadWrite).await;

    alice.write("hello").unwrap();
    bob.pump().await;
    alice.pump().await;

    assert_eq!(title(&bob.doc), "hello");
    assert_eq!(alice.updates, 0, "the sender does not get its own update");
}

#[tokio::test(start_paused = true)]
async fn a_late_client_receives_the_document() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("hello").unwrap();

    let bob = Peer::join(&registry, ReadWrite).await;

    assert_eq!(title(&bob.doc), "hello");
}

#[tokio::test(start_paused = true)]
async fn offline_edits_arrive_through_sync_step2() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;

    let conn = registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap();
    let mut bob = Peer::new(conn);
    bob.edit("offline");
    bob.sync().await;
    alice.pump().await;
    sleep(FLUSH).await;

    assert_eq!(title(&alice.doc), "offline");
    assert_eq!(title(&rebuild(&store.stored(DOC))), "offline");
}

#[tokio::test(start_paused = true)]
async fn a_repeated_update_is_neither_stored_nor_broadcast() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    let mut bob = Peer::join(&registry, ReadWrite).await;
    let update = alice.edit("hello");

    for _ in 0..3 {
        alice
            .send(Message::Sync(SyncMessage::Update(update.clone())))
            .unwrap();
    }
    bob.pump().await;
    sleep(FLUSH).await;

    assert_eq!(bob.updates, 1);
    assert_eq!(store.stored(DOC).updates.len(), 1);
}

// Persistence and replay

#[tokio::test(start_paused = true)]
async fn a_flush_copies_the_title_and_the_last_writer() {
    let store = MemStore::new();
    let registry = registry(&store);
    let ada = Peer::new(registry.connect(DOC, ReadWrite, "ada").await.unwrap());
    let mut bob = Peer::new(registry.connect(DOC, ReadWrite, "bob").await.unwrap());

    ada.write("Plans").unwrap();
    sleep(FLUSH).await;
    bob.pump().await;
    bob.write(" for 2027").unwrap();
    sleep(FLUSH).await;

    let edits = store.edits();
    assert_eq!(edits[0].title.as_deref(), Some("Plans"));
    assert_eq!(edits[0].author.as_deref(), Some("ada"));
    assert_eq!(edits[1].title.as_deref(), Some("Plans for 2027"));
    assert_eq!(edits[1].author.as_deref(), Some("bob"));
}

#[tokio::test(start_paused = true)]
async fn a_document_without_a_title_copies_none() {
    let store = MemStore::new();
    let registry = registry(&store);
    let peer = Peer::join(&registry, ReadWrite).await;
    let body = peer.doc.get_or_insert_xml_fragment("default");
    let update = {
        let mut txn = peer.doc.transact_mut();
        body.push_back(&mut txn, XmlTextPrelim::new("text"));
        txn.encode_update_v1()
    };

    peer.send(Message::Sync(SyncMessage::Update(update)))
        .unwrap();
    sleep(FLUSH).await;

    assert_eq!(store.edits()[0].title, None);
}

#[tokio::test(start_paused = true)]
async fn a_long_title_is_clipped_in_the_copy_only() {
    let store = MemStore::new();
    let registry = registry(&store);
    let peer = Peer::join(&registry, ReadWrite).await;
    let long = "x".repeat(MAX_TITLE_CHARS + 10);

    peer.write(&long).unwrap();
    sleep(FLUSH).await;

    let copied = store.edits()[0].title.clone().unwrap();
    assert_eq!(copied.chars().count(), MAX_TITLE_CHARS);
    assert_eq!(title(&rebuild(&store.stored(DOC))), long);
}

#[tokio::test(start_paused = true)]
async fn a_failed_flush_keeps_its_author_for_the_retry() {
    let store = MemStore::new();
    store.fail_next_appends.store(1, Ordering::SeqCst);
    let registry = registry(&store);
    let peer = Peer::join(&registry, ReadWrite).await;

    peer.write("kept").unwrap();
    sleep(FLUSH * 4).await;

    let edits = store.edits();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].author.as_deref(), Some(AUTHOR));
    assert_eq!(edits[0].title.as_deref(), Some("kept"));
}

// Writes from the server

#[tokio::test(start_paused = true)]
async fn the_server_writes_a_title_like_any_other_edit() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut peer = Peer::join(&registry, ReadWrite).await;
    peer.write("Old 🐢 title").unwrap();

    registry.set_title(DOC, "Nouveau été", "bob").await.unwrap();
    peer.pump().await;
    sleep(FLUSH).await;

    assert_eq!(title(&peer.doc), "Nouveau été");
    assert_eq!(title(&rebuild(&store.stored(DOC))), "Nouveau été");
    let edit = store.edits().pop().unwrap();
    assert_eq!(edit.title.as_deref(), Some("Nouveau été"));
    assert_eq!(edit.author.as_deref(), Some("bob"));
}

#[tokio::test(start_paused = true)]
async fn the_server_writes_to_a_document_nobody_has_open() {
    let store = MemStore::new();
    let registry = registry(&store);

    registry.set_title(DOC, "Written", "bob").await.unwrap();

    assert_eq!(registry.loaded_rooms(), 1);
    sleep(IDLE).await;
    assert_eq!(registry.loaded_rooms(), 0, "the room unloads as usual");
    assert_eq!(title(&rebuild(&store.stored(DOC))), "Written");
}

#[tokio::test(start_paused = true)]
async fn the_same_title_writes_nothing() {
    let store = MemStore::new();
    let registry = registry(&store);
    registry.set_title(DOC, "Same", "ada").await.unwrap();
    sleep(FLUSH).await;

    registry.set_title(DOC, "Same", "bob").await.unwrap();
    sleep(FLUSH).await;

    assert_eq!(store.edits().len(), 1);
}

/// yrs counts these items in the length but only removes text, and panics halfway through.
#[tokio::test(start_paused = true)]
async fn a_title_with_more_than_text_is_left_as_it_is() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut peer = Peer::join(&registry, ReadWrite).await;
    peer.write("kept").unwrap();
    let array = peer.doc.get_or_insert_array("title");
    let update = {
        let mut txn = peer.doc.transact_mut();
        array.push_back(&mut txn, 1);
        txn.encode_update_v1()
    };
    peer.send(Message::Sync(SyncMessage::Update(update)))
        .unwrap();

    let refused = registry.set_title(DOC, "Replaced", "ada").await;

    assert!(matches!(refused, Err(CollabError::CorruptDocument { .. })));
    sleep(FLUSH).await;
    peer.pump().await;
    assert_eq!(peer.updates, 0, "nothing was deleted");
    assert_eq!(title(&rebuild(&store.stored(DOC))), "kept");
}

#[tokio::test(start_paused = true)]
async fn a_server_write_is_refused_while_the_queue_is_full_of_failed_flushes() {
    let store = MemStore::new();
    store.fail_append.store(true, Ordering::SeqCst);
    let registry = DocRegistry::new(
        store.clone(),
        CollabConfig {
            room_queue_bytes: 16,
            ..config()
        },
    );
    let peer = Peer::join(&registry, ReadWrite).await;
    peer.write(&"x".repeat(64)).unwrap();
    sleep(FLUSH).await;

    let refused = registry.set_title(DOC, "Refused", "ada").await;

    assert!(matches!(refused, Err(CollabError::StorageFailing)));
}

// A page deleted under its room

#[tokio::test(start_paused = true)]
async fn a_flush_on_a_deleted_page_closes_the_room_and_stays_healthy() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;
    let mut bob = Peer::join(&registry, ReadOnly).await;
    store.page_gone.store(true, Ordering::SeqCst);

    alice.write("lost").unwrap();
    sleep(FLUSH).await;
    alice.pump().await;
    bob.pump().await;

    assert_eq!(alice.closed, Some(CLOSE_PAGE_GONE));
    assert_eq!(bob.closed, Some(CLOSE_PAGE_GONE));
    assert!(registry.healthy());
    let appends = store.appends();
    drop((alice, bob));
    sleep(IDLE).await;
    assert_eq!(registry.loaded_rooms(), 0);
    assert_eq!(store.appends(), appends, "nothing is left to retry");
}

#[tokio::test(start_paused = true)]
async fn updates_are_flushed_in_one_batch() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;

    for _ in 0..20 {
        alice.write("x").unwrap();
    }
    sleep(FLUSH).await;

    assert_eq!(store.appends(), 1);
    let seqs: Vec<_> = store.stored(DOC).updates.iter().map(|u| u.seq).collect();
    assert_eq!(seqs, (1..=20).collect::<Vec<_>>());
}

#[tokio::test(start_paused = true)]
async fn nothing_is_written_without_an_edit() {
    let store = MemStore::new();
    let registry = registry(&store);
    let _alice = Peer::join(&registry, ReadWrite).await;

    sleep(FLUSH * 5).await;

    assert_eq!(store.appends(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_new_registry_replays_the_stored_updates() {
    let store = MemStore::new();
    let first = registry(&store);
    let alice = Peer::join(&first, ReadWrite).await;
    alice.write("hello").unwrap();
    alice.write(" world").unwrap();
    sleep(FLUSH).await;

    let second = registry(&store);

    assert_eq!(connect_title(&second).await, "hello world");
}

#[tokio::test(start_paused = true)]
async fn seq_continues_after_a_reload() {
    let store = MemStore::new();
    let registry = registry(&store);
    {
        let alice = Peer::join(&registry, ReadWrite).await;
        alice.write("one").unwrap();
        sleep(FLUSH).await;
    }
    sleep(IDLE).await;
    assert_eq!(registry.loaded_rooms(), 0);

    let bob = Peer::join(&registry, ReadWrite).await;
    bob.write(" two").unwrap();
    sleep(FLUSH).await;

    assert!(registry.healthy());
    let stored = store.stored(DOC);
    assert_eq!(stored.last_seq(), 2);
    assert_eq!(title(&rebuild(&stored)), "one two");
}

/// yrs holds an update whose dependency is missing outside the document, and reports no change.
#[tokio::test(start_paused = true)]
async fn an_update_waiting_for_its_dependency_survives_a_restart() {
    let store = MemStore::new();
    let first = registry(&store);
    let alice = Peer::join(&first, ReadWrite).await;
    let earlier = alice.edit("one ");
    let later = alice.edit("two");
    alice
        .send(Message::Sync(SyncMessage::Update(later)))
        .unwrap();
    sleep(FLUSH).await;

    let second = registry(&store);
    let bob = Peer::join(&second, ReadWrite).await;
    bob.send(Message::Sync(SyncMessage::Update(earlier)))
        .unwrap();

    let carol = Peer::join(&second, ReadWrite).await;
    assert_eq!(title(&carol.doc), "one two");
}

#[tokio::test]
async fn sqlite_keeps_a_document_across_registries() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cadocs.db");
    let fast = CollabConfig {
        flush_interval: Duration::from_millis(5),
        compact_updates: 3,
        ..CollabConfig::default()
    };

    let store = Arc::new(SqliteStore::open(&path).await.unwrap());
    let pool = SqlitePool::connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    sqlx::raw_sql(
        "INSERT INTO workspaces (id, slug, name, created_at, updated_at)
             VALUES ('ws', 'ws', 'Workspace', 1, 1);
         INSERT INTO spaces (id, workspace_id, slug, name, created_at, updated_at)
             VALUES ('space', 'ws', 'space', 'Space', 1, 1);
         INSERT INTO pages (id, workspace_id, space_id, position, slug, created_at, updated_at)
             VALUES ('doc', 'ws', 'space', 'a', 'doc', 1, 1);",
    )
    .execute(&pool)
    .await
    .unwrap();
    pool.close().await;
    let first = DocRegistry::new(store, fast.clone());
    let alice = Peer::join(&first, ReadWrite).await;
    for word in ["a", "b", "c", "d", "e"] {
        alice.write(word).unwrap();
        sleep(Duration::from_millis(20)).await;
    }
    drop(alice);
    first.shutdown().await.unwrap();

    let store = Arc::new(SqliteStore::open(&path).await.unwrap());
    assert!(store.load(DOC).await.unwrap().snapshot.is_some());
    let second = DocRegistry::new(store, fast);
    let bob = Peer::join(&second, ReadWrite).await;
    assert_eq!(title(&bob.doc), "abcde");
}

// Compaction

fn compacting(store: &Arc<MemStore>, threshold: usize) -> DocRegistry<MemStore> {
    let config = CollabConfig {
        compact_updates: threshold,
        ..config()
    };
    DocRegistry::new(store.clone(), config)
}

#[tokio::test(start_paused = true)]
async fn compaction_starts_past_the_threshold() {
    let store = MemStore::new();
    let registry = compacting(&store, 3);
    let alice = Peer::join(&registry, ReadWrite).await;

    for _ in 0..3 {
        alice.write("x").unwrap();
    }
    sleep(FLUSH).await;
    assert_eq!(store.compacts(), 0, "the threshold itself does not compact");

    alice.write("y").unwrap();
    sleep(FLUSH).await;

    let stored = store.stored(DOC);
    assert_eq!(stored.snapshot.as_ref().unwrap().upto_seq, 4);
    assert!(stored.updates.is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_compacted_document_equals_the_replayed_log() {
    let store = MemStore::new();
    let plain = MemStore::new();
    let compacted = compacting(&store, 2);
    let logged = registry(&plain);
    let alice = Peer::join(&compacted, ReadWrite).await;
    let mirror = Peer::join(&logged, ReadWrite).await;

    for word in ["one ", "two ", "three ", "four ", "five"] {
        let update = alice.edit(word);
        for peer in [&alice, &mirror] {
            peer.send(Message::Sync(SyncMessage::Update(update.clone())))
                .unwrap();
        }
        sleep(FLUSH).await;
    }

    assert!(store.stored(DOC).snapshot.is_some());
    assert!(plain.stored(DOC).snapshot.is_none());
    let from_snapshot = rebuild(&store.stored(DOC));
    let from_log = rebuild(&plain.stored(DOC));
    assert_eq!(title(&from_snapshot), "one two three four five");
    assert_eq!(title(&from_snapshot), title(&from_log));
    assert_eq!(state_vector(&from_snapshot), state_vector(&from_log));
}

#[tokio::test(start_paused = true)]
async fn an_edit_during_compaction_is_kept() {
    let store = MemStore::new();
    let registry = compacting(&store, 1);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.compact_gate.hold();

    alice.write("a").unwrap();
    alice.write("b").unwrap();
    store.compact_gate.entered().await;
    alice.write("c").unwrap();
    store.compact_gate.release();
    sleep(FLUSH).await;

    assert!(registry.healthy());
    assert_eq!(title(&rebuild(&store.stored(DOC))), "abc");
}

#[tokio::test(start_paused = true)]
async fn an_edit_queued_when_compaction_starts_is_stored_after_it() {
    let store = MemStore::new();
    let registry = compacting(&store, 1);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.append_gate.hold();

    alice.write("a").unwrap();
    alice.write("b").unwrap();
    store.append_gate.entered().await;
    alice.write("c").unwrap();
    store.append_gate.release();
    sleep(FLUSH * 2).await;

    assert!(registry.healthy());
    assert!(store.compacts() >= 1);
    assert_eq!(title(&rebuild(&store.stored(DOC))), "abc");
}

#[tokio::test(start_paused = true)]
async fn a_failed_compaction_loses_nothing() {
    let store = MemStore::new();
    let registry = compacting(&store, 1);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.fail_compact.store(true, Ordering::SeqCst);

    alice.write("a").unwrap();
    alice.write("b").unwrap();
    sleep(FLUSH).await;

    assert!(store.stored(DOC).snapshot.is_none());
    assert_eq!(store.stored(DOC).updates.len(), 2);

    store.fail_compact.store(false, Ordering::SeqCst);
    sleep(Duration::from_secs(31)).await;
    alice.write("c").unwrap();
    sleep(FLUSH).await;

    assert_eq!(store.stored(DOC).snapshot.unwrap().upto_seq, 3);
}

// Unload and reload

#[tokio::test(start_paused = true)]
async fn an_empty_room_unloads_after_the_delay() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("hello").unwrap();
    drop(alice);

    sleep(Duration::from_secs(59)).await;
    assert_eq!(registry.loaded_rooms(), 1);

    sleep(Duration::from_secs(2)).await;
    assert_eq!(registry.loaded_rooms(), 0);
    let stored = store.stored(DOC);
    assert!(stored.updates.is_empty(), "unloading compacts");
    assert_eq!(title(&rebuild(&stored)), "hello");
}

#[tokio::test(start_paused = true)]
async fn a_reloaded_document_is_identical() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("hello").unwrap();
    let before = state_vector(&alice.doc);
    drop(alice);
    sleep(IDLE).await;

    let bob = Peer::join(&registry, ReadWrite).await;

    assert_eq!(registry.loaded_rooms(), 1);
    assert_eq!(title(&bob.doc), "hello");
    assert_eq!(state_vector(&bob.doc), before);
}

#[tokio::test(start_paused = true)]
async fn a_returning_client_keeps_the_room() {
    let store = MemStore::new();
    let registry = registry(&store);
    drop(Peer::join(&registry, ReadWrite).await);
    sleep(Duration::from_secs(50)).await;

    let _bob = Peer::join(&registry, ReadWrite).await;
    sleep(IDLE * 2).await;

    assert_eq!(registry.loaded_rooms(), 1);
    assert_eq!(store.loads(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_unload_is_announced() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    let announced = tokio::spawn({
        let registry = registry.clone();
        async move { registry.room_unloaded().await }
    });
    drop(alice);

    sleep(Duration::from_secs(59)).await;
    assert!(!announced.is_finished());

    sleep(Duration::from_secs(2)).await;
    assert!(announced.is_finished());
}

#[tokio::test(start_paused = true)]
async fn an_untouched_room_unloads_without_writing() {
    let store = MemStore::new();
    let registry = registry(&store);
    drop(Peer::join(&registry, ReadWrite).await);

    sleep(IDLE).await;

    assert_eq!(registry.loaded_rooms(), 0);
    assert_eq!(store.appends(), 0);
    assert_eq!(store.compacts(), 0);
}

#[tokio::test(start_paused = true)]
async fn a_room_nobody_joined_still_unloads() {
    let store = MemStore::new();
    let registry = registry(&store);
    store.load_gate.hold();
    let waiting = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.connect(DOC, ReadWrite, AUTHOR).await.map(drop) })
    };
    store.load_gate.entered().await;
    waiting.abort();
    let _ = waiting.await;
    store.load_gate.release();

    sleep(IDLE * 2).await;

    assert_eq!(registry.loaded_rooms(), 0);
}

// Load and unload concurrency

#[tokio::test(start_paused = true)]
async fn concurrent_clients_share_one_load() {
    let store = MemStore::new();
    let registry = registry(&store);
    store.load_gate.hold();
    let mut joins = Vec::new();
    for _ in 0..5 {
        let registry = registry.clone();
        joins.push(tokio::spawn(async move {
            registry.connect(DOC, ReadWrite, AUTHOR).await.is_ok()
        }));
    }
    store.load_gate.entered().await;
    sleep(Duration::from_millis(1)).await;
    store.load_gate.release();

    for join in joins {
        assert!(join.await.unwrap());
    }
    assert_eq!(store.loads(), 1);
    assert_eq!(registry.loaded_rooms(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_client_arriving_during_an_unload_waits_for_the_persisted_state() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("hello").unwrap();
    drop(alice);
    store.compact_gate.hold();
    store.compact_gate.entered().await;

    let bob = {
        let registry = registry.clone();
        tokio::spawn(async move { connect_title(&registry).await })
    };
    sleep(Duration::from_secs(5)).await;
    assert!(!bob.is_finished(), "the client waits for the unload");
    assert_eq!(store.loads(), 1);

    store.compact_gate.release();

    assert_eq!(bob.await.unwrap(), "hello");
    assert_eq!(store.loads(), 2, "it reads what the unload stored");
}

#[tokio::test(start_paused = true)]
async fn a_failed_load_is_reported_and_can_be_retried() {
    let store = MemStore::new();
    let registry = registry(&store);
    store.fail_load.store(true, Ordering::SeqCst);

    let refused = registry.connect(DOC, ReadWrite, AUTHOR).await;
    assert!(matches!(refused, Err(CollabError::Store(_))));
    assert_eq!(registry.loaded_rooms(), 0);

    store.fail_load.store(false, Ordering::SeqCst);
    assert!(registry.connect(DOC, ReadWrite, AUTHOR).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn a_corrupt_document_is_refused() {
    let store = MemStore::new();
    store
        .append_updates(
            DOC,
            &[cadocs_store::StoredUpdate {
                seq: 1,
                data: vec![0xff, 0xff],
            }],
            &DocEdit::default(),
        )
        .await
        .unwrap();
    let registry = registry(&store);

    let refused = registry.connect(DOC, ReadWrite, AUTHOR).await;

    assert!(matches!(refused, Err(CollabError::CorruptDocument { .. })));
    assert_eq!(registry.loaded_rooms(), 0);
}

// Read-only

#[tokio::test(start_paused = true)]
async fn a_read_only_update_is_dropped_and_answered_once() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut writer = Peer::join(&registry, ReadWrite).await;
    let mut reader = Peer::join(&registry, ReadOnly).await;
    reader.denied.clear();

    reader.write("sneaky").unwrap();
    reader.write("sneaky").unwrap();
    writer.pump().await;
    reader.pump().await;
    sleep(FLUSH).await;

    assert_eq!(title(&writer.doc), "");
    assert!(store.stored(DOC).updates.is_empty());
    let late = Peer::join(&registry, ReadOnly).await;
    assert_eq!(title(&late.doc), "");
}

#[tokio::test(start_paused = true)]
async fn a_read_only_client_is_told_why() {
    let store = MemStore::new();
    let registry = registry(&store);
    let conn = registry.connect(DOC, ReadOnly, AUTHOR).await.unwrap();
    let mut reader = Peer::new(conn);

    reader.write("a").unwrap();
    reader.write("b").unwrap();
    let step2 = Message::Sync(SyncMessage::SyncStep2(reader.edit("c")));
    reader.send(step2).unwrap();
    reader.pump().await;

    assert_eq!(reader.denied, vec![Some("read-only".to_owned())]);
}

#[tokio::test(start_paused = true)]
async fn a_read_only_client_follows_edits_and_shows_its_presence() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut writer = Peer::join(&registry, ReadWrite).await;
    let mut reader = Peer::join(&registry, ReadOnly).await;

    writer.write("hello").unwrap();
    reader.pump().await;
    reader.announce(r#"{"name":"reader"}"#);
    writer.pump().await;

    assert_eq!(title(&reader.doc), "hello");
    assert_eq!(writer.present(), vec![reader.doc.client_id().get()]);
}

// Awareness

#[tokio::test(start_paused = true)]
async fn a_closed_socket_leaves_no_ghost_cursor() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;
    let mut bob = Peer::join(&registry, ReadWrite).await;
    bob.announce(r#"{"name":"bob"}"#);
    alice.pump().await;
    assert_eq!(alice.present().len(), 1);

    drop(bob);
    alice.pump().await;
    assert!(alice.present().is_empty());

    let carol = Peer::join(&registry, ReadWrite).await;
    assert!(carol.present().is_empty());
}

/// `y-websocket` reconnects after 30 seconds without a message, and pings do not reach it. The
/// echo of its own 15-second heartbeat is what a client alone in a room hears.
#[tokio::test(start_paused = true)]
async fn a_client_hears_its_own_awareness_back() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;
    alice.pump().await;

    alice.announce(r#"{"name":"alice"}"#);

    let echo = tokio::time::timeout(Duration::from_secs(1), alice.conn.next()).await;
    assert!(matches!(echo, Ok(Outbound::Frame(_))));

    // A heartbeat bumps the clock and leaves the state as it was.
    alice.announce(r#"{"name":"alice"}"#);

    let echo = tokio::time::timeout(Duration::from_secs(1), alice.conn.next()).await;
    assert!(matches!(echo, Ok(Outbound::Frame(_))));
}

#[tokio::test(start_paused = true)]
async fn a_new_client_receives_current_presence() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;
    alice.announce(r#"{"name":"alice"}"#);

    let bob = Peer::join(&registry, ReadWrite).await;

    assert_eq!(bob.present(), vec![alice.doc.client_id().get()]);
}

#[tokio::test(start_paused = true)]
async fn an_awareness_query_returns_all_states() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;
    let mut bob = Peer::join(&registry, ReadWrite).await;
    alice.announce(r#"{"name":"alice"}"#);
    bob.pump().await;
    bob.awareness = Awareness::new(bob.doc.clone());

    bob.send(Message::AwarenessQuery).unwrap();
    bob.pump().await;

    assert_eq!(bob.present(), vec![alice.doc.client_id().get()]);
}

/// `y-websocket` sends back every awareness change it sees, including those of other clients.
#[tokio::test(start_paused = true)]
async fn an_echoed_state_does_not_change_its_owner() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::join(&registry, ReadWrite).await;
    let mut bob = Peer::join(&registry, ReadWrite).await;
    let mut carol = Peer::join(&registry, ReadWrite).await;
    alice.announce(r#"{"name":"alice"}"#);
    bob.pump().await;
    carol.pump().await;

    bob.send(Message::Awareness(bob.awareness.update().unwrap()))
        .unwrap();
    drop(bob);
    carol.pump().await;

    assert_eq!(carol.present(), vec![alice.doc.client_id().get()]);
}

#[tokio::test(start_paused = true)]
async fn a_reconnected_client_keeps_its_presence_when_the_old_socket_closes() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut watcher = Peer::join(&registry, ReadWrite).await;
    let mut old = Peer::join(&registry, ReadWrite).await;
    old.announce(r#"{"v":1}"#);

    let new = Peer::join(&registry, ReadWrite).await;
    old.awareness.set_local_state_raw(r#"{"v":2}"#);
    let update = old.awareness.update().unwrap();
    new.send(Message::Awareness(update)).unwrap();
    drop(old);
    watcher.pump().await;

    assert_eq!(watcher.present().len(), 1);
}

// Room cap and socket cap

fn capped(store: &Arc<MemStore>, max_rooms: usize) -> DocRegistry<MemStore> {
    let config = CollabConfig {
        max_rooms,
        ..config()
    };
    DocRegistry::new(store.clone(), config)
}

#[tokio::test(start_paused = true)]
async fn at_the_cap_with_only_live_rooms_a_connection_is_refused() {
    let store = MemStore::new();
    let registry = capped(&store, 2);
    let _a = registry.connect("a", ReadWrite, AUTHOR).await.unwrap();
    let _b = registry.connect("b", ReadWrite, AUTHOR).await.unwrap();

    let refused = registry.connect("c", ReadWrite, AUTHOR).await;

    assert!(matches!(refused, Err(CollabError::AtCapacity)));
    assert_eq!(registry.loaded_rooms(), 2);
    assert!(registry.connect("a", ReadWrite, AUTHOR).await.is_ok());
}

#[tokio::test(start_paused = true)]
async fn at_the_cap_the_oldest_idle_room_unloads_first() {
    let store = MemStore::new();
    let registry = capped(&store, 3);
    let older = registry.connect("older", ReadWrite, AUTHOR).await.unwrap();
    let newer = registry.connect("newer", ReadWrite, AUTHOR).await.unwrap();
    let _live = registry.connect("live", ReadWrite, AUTHOR).await.unwrap();
    let edit = Peer::new(older);
    edit.write("kept").unwrap();
    drop(edit);
    sleep(Duration::from_secs(1)).await;
    drop(newer);
    sleep(Duration::from_secs(1)).await;

    let _extra = registry.connect("extra", ReadWrite, AUTHOR).await.unwrap();

    assert_eq!(registry.loaded_rooms(), 3);
    assert_eq!(title(&rebuild(&store.stored("older"))), "kept");
    let reloads = store.loads();
    let _newer = registry.connect("newer", ReadWrite, AUTHOR).await.unwrap();
    assert_eq!(store.loads(), reloads, "the newer idle room stayed loaded");
}

#[tokio::test(start_paused = true)]
async fn a_room_failing_to_flush_is_not_evicted() {
    let store = MemStore::new();
    let registry = capped(&store, 1);
    let edit = Peer::new(registry.connect("a", ReadWrite, AUTHOR).await.unwrap());
    store.fail_append.store(true, Ordering::SeqCst);
    edit.write("kept").unwrap();
    drop(edit);
    sleep(FLUSH).await;

    let refused = registry.connect("b", ReadWrite, AUTHOR).await;

    assert!(matches!(refused, Err(CollabError::AtCapacity)));
    store.fail_append.store(false, Ordering::SeqCst);
    sleep(Duration::from_secs(60)).await;
    assert_eq!(title(&rebuild(&store.stored("a"))), "kept");
}

#[tokio::test(start_paused = true)]
async fn past_the_socket_cap_a_connection_is_refused() {
    let store = MemStore::new();
    let config = CollabConfig {
        max_sockets: 1,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let first = registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap();

    let refused = registry.connect(DOC, ReadWrite, AUTHOR).await;
    assert!(matches!(refused, Err(CollabError::AtCapacity)));

    drop(first);
    assert!(registry.connect(DOC, ReadWrite, AUTHOR).await.is_ok());
}

// Flush failure

#[tokio::test(start_paused = true)]
async fn a_failed_flush_retries_and_loses_nothing() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.fail_append.store(true, Ordering::SeqCst);

    alice.write("a").unwrap();
    sleep(FLUSH).await;
    assert!(!registry.healthy());

    alice.write("b").unwrap();
    sleep(Duration::from_secs(5)).await;
    assert!(store.appends() > 1, "the flush retries");
    assert!(store.stored(DOC).updates.is_empty());

    store.fail_append.store(false, Ordering::SeqCst);
    sleep(Duration::from_secs(31)).await;

    assert!(registry.healthy());
    let stored = store.stored(DOC);
    assert_eq!(stored.updates.len(), 2);
    assert_eq!(title(&rebuild(&stored)), "ab");
}

#[tokio::test(start_paused = true)]
async fn retries_back_off() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.fail_append.store(true, Ordering::SeqCst);
    alice.write("a").unwrap();

    sleep(Duration::from_secs(60)).await;

    assert!(store.appends() < 12, "{} attempts", store.appends());
}

#[tokio::test(start_paused = true)]
async fn a_full_queue_closes_write_sockets_and_refuses_the_edit() {
    let store = MemStore::new();
    let config = CollabConfig {
        room_queue_bytes: 200,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let mut writer = Peer::join(&registry, ReadWrite).await;
    let mut other = Peer::join(&registry, ReadWrite).await;
    let mut reader = Peer::join(&registry, ReadOnly).await;
    store.fail_append.store(true, Ordering::SeqCst);

    writer.write(&"a".repeat(150)).unwrap();
    other.pump().await;
    reader.pump().await;
    sleep(FLUSH).await;
    writer.write(&"b".repeat(150)).unwrap();
    writer.pump().await;
    other.pump().await;
    reader.pump().await;

    assert_eq!(writer.closed, Some(CLOSE_STORAGE_FAILING));
    assert_eq!(other.closed, Some(CLOSE_STORAGE_FAILING));
    assert_eq!(reader.closed, None);
    assert!(!title(&reader.doc).contains('b'), "the edit was refused");

    store.fail_append.store(false, Ordering::SeqCst);
    sleep(Duration::from_secs(31)).await;

    let mut back = Peer::new(registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap());
    back.doc = writer.doc.clone();
    back.sync().await;
    sleep(FLUSH).await;
    assert_eq!(back.closed, None);
    let stored = title(&rebuild(&store.stored(DOC)));
    assert!(stored.contains('a') && stored.contains('b'), "resent");
}

#[tokio::test(start_paused = true)]
async fn one_update_larger_than_the_queue_passes_when_the_queue_is_empty() {
    let store = MemStore::new();
    let config = CollabConfig {
        room_queue_bytes: 10,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let mut alice = Peer::join(&registry, ReadWrite).await;

    alice.write(&"a".repeat(100)).unwrap();
    alice.pump().await;
    sleep(FLUSH).await;

    assert_eq!(alice.closed, None);
    assert_eq!(title(&rebuild(&store.stored(DOC))).len(), 100);
}

#[tokio::test(start_paused = true)]
async fn a_failed_unload_keeps_the_room_and_its_edits() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.fail_append.store(true, Ordering::SeqCst);
    alice.write("kept").unwrap();
    drop(alice);

    sleep(IDLE * 2).await;
    assert_eq!(registry.loaded_rooms(), 1);
    assert_eq!(connect_title(&registry).await, "kept");

    store.fail_append.store(false, Ordering::SeqCst);
    sleep(IDLE * 2).await;

    assert_eq!(registry.loaded_rooms(), 0);
    assert!(registry.healthy());
    assert_eq!(title(&rebuild(&store.stored(DOC))), "kept");
}

#[tokio::test(start_paused = true)]
async fn a_failed_compaction_does_not_block_an_unload() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.fail_compact.store(true, Ordering::SeqCst);
    alice.write("kept").unwrap();
    drop(alice);

    sleep(IDLE).await;

    assert_eq!(registry.loaded_rooms(), 0);
    assert_eq!(title(&rebuild(&store.stored(DOC))), "kept");
}

// Shutdown

#[tokio::test(start_paused = true)]
async fn shutdown_flushes_every_room_and_closes_its_sockets() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut alice = Peer::new(registry.connect("a", ReadWrite, AUTHOR).await.unwrap());
    let bob = Peer::new(registry.connect("b", ReadWrite, AUTHOR).await.unwrap());
    alice.write("one").unwrap();
    bob.write("two").unwrap();

    registry.shutdown().await.unwrap();

    alice.pump().await;
    assert_eq!(alice.closed, Some(CLOSE_GOING_AWAY));
    assert_eq!(title(&rebuild(&store.stored("a"))), "one");
    assert_eq!(title(&rebuild(&store.stored("b"))), "two");
    assert_eq!(registry.loaded_rooms(), 0);
    assert!(matches!(
        registry.connect("a", ReadWrite, AUTHOR).await,
        Err(CollabError::ShuttingDown)
    ));
}

#[tokio::test(start_paused = true)]
async fn an_edit_after_shutdown_started_is_refused() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("a").unwrap();
    store.append_gate.hold();
    let stopping = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.shutdown().await })
    };
    store.append_gate.entered().await;

    let _ = alice.write("late");
    store.append_gate.release();
    stopping.await.unwrap().unwrap();

    assert_eq!(title(&rebuild(&store.stored(DOC))), "a");
}

#[tokio::test(start_paused = true)]
async fn shutdown_reports_a_failed_flush() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("lost").unwrap();
    store.fail_append.store(true, Ordering::SeqCst);

    let result = registry.shutdown().await;

    assert!(matches!(result, Err(CollabError::Store(_))));
    assert_eq!(registry.loaded_rooms(), 0);
}

// Socket limits and malformed input

#[tokio::test(start_paused = true)]
async fn an_oversized_frame_closes_the_socket() {
    let store = MemStore::new();
    let config = CollabConfig {
        max_frame_bytes: 64,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let alice = Peer::join(&registry, ReadWrite).await;

    let refused = alice.write(&"a".repeat(100));

    assert_eq!(refused, Err(CLOSE_FRAME_TOO_LARGE));
}

#[tokio::test(start_paused = true)]
async fn malformed_frames_close_the_socket_but_not_the_room() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("hello").unwrap();

    let frames: [&[u8]; 6] = [
        &[],
        &[0],
        &[0, 7],
        &[0, 2, 200],
        &[9, 1, 1],
        &[0, 2, 3, 255, 255, 255],
    ];
    for bad in frames {
        let conn = registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap();
        let refused = conn.receive(bad).map_err(|close| close.0);
        assert_eq!(refused, Err(CLOSE_PROTOCOL_ERROR), "frame {bad:?}");
    }

    let bob = Peer::join(&registry, ReadWrite).await;
    assert_eq!(title(&bob.doc), "hello");
}

/// A fixed-seed mutation of valid frames: none may break the room, whatever it does to the socket.
/// The client ids are fixed too, because yrs 0.28 can segfault on crafted updates (see
/// `docs/ARCHITECTURE.md`), and random ids would make this test crash now and then.
#[tokio::test(start_paused = true)]
async fn mutated_frames_do_not_break_the_room() {
    let store = MemStore::new();
    let registry = registry(&store);
    let conn = registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap();
    let mut source = Peer::with_doc(conn, Doc::with_client_id(7));
    source.sync().await;
    source.write("hello").unwrap();
    source.announce(r#"{"name":"source"}"#);
    let valid = [
        frame(Message::Sync(SyncMessage::Update(source.edit("abc")))),
        frame(Message::Sync(SyncMessage::SyncStep2(source.edit("def")))),
        frame(Message::Sync(SyncMessage::SyncStep1(state_vector(
            &source.doc,
        )))),
        frame(Message::Awareness(source.awareness.update().unwrap())),
    ];

    let mut seed = 0x2545_f491_4f6c_dd1d_u64;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed as usize
    };
    for round in 0..4000 {
        let mut bytes = valid[round % valid.len()].clone();
        for _ in 0..1 + next() % 3 {
            let at = next() % bytes.len();
            bytes[at] = next() as u8;
        }
        if next() % 4 == 0 {
            bytes.truncate(next() % bytes.len());
        }
        let conn = registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap();
        let _ = conn.receive(&bytes);
    }

    // A mutated delete set is a legal edit, so only the room's own consistency can be checked.
    let mut bob = Peer::join(&registry, ReadWrite).await;
    let carol = Peer::join(&registry, ReadWrite).await;
    carol.write("still editable").unwrap();
    bob.pump().await;
    sleep(FLUSH).await;

    assert!(registry.healthy());
    assert!(title(&bob.doc).contains("still editable"));
    // A crafted frame can make a replay place content differently, with the same ids and deletes.
    // `real_updates_in_any_order_survive_a_reload` holds real updates to strict equality.
    let reloaded = DocRegistry::new(store.clone(), common::config());
    assert!(connect_title(&reloaded).await.contains("still editable"));
}

#[tokio::test(start_paused = true)]
async fn a_client_that_stops_reading_is_closed() {
    let store = MemStore::new();
    let config = CollabConfig {
        send_queue: 4,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let alice = Peer::join(&registry, ReadWrite).await;
    let mut stuck = Peer::join(&registry, ReadWrite).await;

    for _ in 0..10 {
        alice.write("x").unwrap();
    }
    stuck.pump().await;

    assert_eq!(stuck.closed, Some(CLOSE_TOO_SLOW));
    assert_eq!(title(&Peer::join(&registry, ReadWrite).await.doc).len(), 10);
}

#[tokio::test(start_paused = true)]
async fn outbound_stays_closed_after_a_close() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut conn = registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap();
    registry.shutdown().await.unwrap();

    assert!(matches!(
        conn.next().await,
        Outbound::Close(CLOSE_GOING_AWAY)
    ));
    assert!(matches!(
        conn.next().await,
        Outbound::Close(CLOSE_GOING_AWAY)
    ));
}

// Findings of the independent review

fn peak_rss_kb() -> usize {
    let status = std::fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("VmHWM")).unwrap();
    line.split_whitespace().nth(1).unwrap().parse().unwrap()
}

/// `yrs` reserves a hash map for the count a frame declares, before reading any entry. The kernel
/// updates VmHWM lazily, so it can read slightly lower than a moment before.
#[cfg(target_os = "linux")]
#[tokio::test(start_paused = true)]
async fn a_tiny_frame_cannot_declare_a_huge_count() {
    let store = MemStore::new();
    let registry = registry(&store);
    let conn = registry.connect(DOC, ReadOnly, AUTHOR).await.unwrap();
    let count = [0x80, 0x80, 0x80, 0x80, 0x02]; // 1 << 29
    let frames: [Vec<u8>; 3] = [
        [&[0, 0, 5][..], &count].concat(),
        [&[1, 5][..], &count].concat(),
        [&[0, 2, 5][..], &count].concat(),
    ];
    let before = peak_rss_kb();

    for bad in frames {
        assert_eq!(conn.receive(&bad), Err(Close(CLOSE_PROTOCOL_ERROR)));
    }

    assert!(
        peak_rss_kb().saturating_sub(before) < 100 * 1024,
        "peak RSS jumped"
    );
}

#[tokio::test(start_paused = true)]
async fn a_snapshot_that_cannot_be_applied_is_refused() {
    let store = MemStore::new();
    let (base, bad) = common::inapplicable_update();
    let state = yrs::merge_updates_v1([base.as_slice(), bad.as_slice()]).unwrap_or(bad);
    let snapshot = cadocs_store::Snapshot { state, upto_seq: 1 };
    store.compact(DOC, &snapshot).await.unwrap();
    let registry = registry(&store);

    let refused = registry.connect(DOC, ReadWrite, AUTHOR).await;

    assert!(matches!(refused, Err(CollabError::CorruptDocument { .. })));
}

#[tokio::test(start_paused = true)]
async fn a_skipped_row_keeps_the_log_from_being_compacted() {
    let store = MemStore::new();
    let (base, bad) = common::inapplicable_update();
    let rows = [(1, base), (2, bad)].map(|(seq, data)| cadocs_store::StoredUpdate { seq, data });
    store
        .append_updates(DOC, &rows, &DocEdit::default())
        .await
        .unwrap();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    assert!(title(&alice.doc).starts_with('a'));
    alice.write("x").unwrap();
    drop(alice);

    sleep(IDLE).await;

    assert_eq!(registry.loaded_rooms(), 0);
    assert_eq!(store.compacts(), 0);
    assert_eq!(store.stored(DOC).updates.len(), 3);
}

#[tokio::test(start_paused = true)]
async fn a_client_requesting_the_document_without_reading_is_closed() {
    let store = MemStore::new();
    let config = CollabConfig {
        max_frame_bytes: 1000,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write(&"a".repeat(600)).unwrap();
    let mut reader = Peer::new(registry.connect(DOC, ReadOnly, AUTHOR).await.unwrap());

    for _ in 0..4 {
        let empty = yrs::StateVector::default();
        reader
            .send(Message::Sync(SyncMessage::SyncStep1(empty)))
            .unwrap();
    }
    reader.pump().await;

    assert_eq!(reader.closed, Some(CLOSE_TOO_SLOW));
}

#[tokio::test(start_paused = true)]
async fn a_burst_on_healthy_storage_closes_nobody() {
    let store = MemStore::new();
    let config = CollabConfig {
        room_queue_bytes: 10,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let mut alice = Peer::join(&registry, ReadWrite).await;

    alice.write(&"a".repeat(100)).unwrap();
    alice.write(&"b".repeat(100)).unwrap();
    alice.pump().await;
    sleep(FLUSH).await;

    assert_eq!(alice.closed, None);
    assert_eq!(title(&rebuild(&store.stored(DOC))).len(), 200);
}

#[tokio::test(start_paused = true)]
async fn a_returning_client_shows_again_at_its_next_change() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut watcher = Peer::join(&registry, ReadWrite).await;
    let mut alice = Peer::join(&registry, ReadWrite).await;
    alice.announce(r#"{"v":1}"#);
    let Peer {
        awareness, conn, ..
    } = alice;
    drop(conn);
    watcher.pump().await;
    assert!(watcher.present().is_empty());

    let mut back = Peer::join(&registry, ReadWrite).await;
    back.awareness = awareness;
    back.announce(r#"{"v":2}"#);
    watcher.pump().await;

    assert_eq!(watcher.present().len(), 1);
}

#[tokio::test(start_paused = true)]
async fn a_waiting_update_does_not_make_no_ops_stored() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    let bob = Peer::join(&registry, ReadWrite).await;
    let _gap = alice.edit("one ");
    let waiting = alice.edit("two");
    alice
        .send(Message::Sync(SyncMessage::Update(waiting)))
        .unwrap();
    bob.write("b").unwrap();
    let repeat = bob
        .doc
        .transact()
        .encode_state_as_update_v1(&Default::default());
    sleep(FLUSH).await;
    let rows = store.stored(DOC).updates.len();

    bob.send(Message::Sync(SyncMessage::SyncStep2(repeat)))
        .unwrap();
    sleep(FLUSH).await;

    assert_eq!(store.stored(DOC).updates.len(), rows);
}

#[tokio::test(start_paused = true)]
async fn shutdown_retries_a_transient_flush_failure() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    alice.write("kept").unwrap();
    store.fail_next_appends.store(1, Ordering::SeqCst);

    registry.shutdown().await.unwrap();

    assert_eq!(title(&rebuild(&store.stored(DOC))), "kept");
}

#[tokio::test(start_paused = true)]
async fn a_failed_load_is_not_a_failed_shutdown() {
    let store = MemStore::new();
    let registry = registry(&store);
    store.fail_load.store(true, Ordering::SeqCst);
    store.load_gate.hold();
    let loading = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.connect(DOC, ReadWrite, AUTHOR).await.map(drop) })
    };
    store.load_gate.entered().await;
    let stopping = {
        let registry = registry.clone();
        tokio::spawn(async move { registry.shutdown().await })
    };
    sleep(Duration::from_millis(1)).await;
    store.load_gate.release();

    assert!(loading.await.unwrap().is_err());
    assert!(stopping.await.unwrap().is_ok());
}

fn awareness_update(entries: impl IntoIterator<Item = (u64, String)>) -> AwarenessUpdate {
    let clients = entries
        .into_iter()
        .map(|(id, json)| {
            let entry = AwarenessUpdateEntry {
                clock: 1,
                json: json.into(),
            };
            (yrs::block::ClientID::new(id), entry)
        })
        .collect();
    AwarenessUpdate { clients }
}

#[tokio::test(start_paused = true)]
async fn a_socket_cannot_announce_unlimited_client_ids() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut watcher = Peer::join(&registry, ReadWrite).await;
    let flood = Peer::join(&registry, ReadOnly).await;

    let entries = (1..=100).map(|id| (id, "{}".to_owned()));
    let refused = flood.send(Message::Awareness(awareness_update(entries)));
    watcher.pump().await;

    assert_eq!(refused, Err(CLOSE_PROTOCOL_ERROR));
    assert!(watcher.present().is_empty());
}

#[tokio::test(start_paused = true)]
async fn the_id_limit_counts_every_id_a_socket_ever_announced() {
    let store = MemStore::new();
    let registry = registry(&store);
    let churn = Peer::join(&registry, ReadOnly).await;

    let mut last = Ok(());
    for id in 1..=100 {
        last = churn.send(Message::Awareness(awareness_update([(
            id,
            "{}".to_owned(),
        )])));
        if last.is_err() {
            break;
        }
    }

    assert_eq!(last, Err(CLOSE_PROTOCOL_ERROR));
}

#[tokio::test(start_paused = true)]
async fn an_oversized_awareness_state_closes_the_socket() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut watcher = Peer::join(&registry, ReadWrite).await;
    let big = Peer::join(&registry, ReadOnly).await;

    let state = format!("\"{}\"", "x".repeat(100 * 1024));
    let refused = big.send(Message::Awareness(awareness_update([(1, state)])));
    watcher.pump().await;

    assert_eq!(refused, Err(CLOSE_PROTOCOL_ERROR));
    assert!(watcher.present().is_empty());
}

/// `y-websocket` reports a peer it timed out with a null state, for ids the server may not know.
#[tokio::test(start_paused = true)]
async fn removing_unknown_ids_is_harmless() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;

    let entries = (1..=100).map(|id| (id, "null".to_owned()));

    assert_eq!(
        alice.send(Message::Awareness(awareness_update(entries))),
        Ok(())
    );
}

#[tokio::test(start_paused = true)]
async fn echoing_many_known_clients_is_allowed() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut peers = Vec::new();
    for _ in 0..40 {
        let mut peer = Peer::join(&registry, ReadWrite).await;
        peer.announce("{}");
        peers.push(peer);
    }
    let mut echo = Peer::join(&registry, ReadWrite).await;
    echo.pump().await;

    let update = echo.awareness.update().unwrap();

    assert_eq!(update.clients.len(), 40);
    assert_eq!(echo.send(Message::Awareness(update)), Ok(()));
}

struct Rng(u64);

impl Rng {
    fn next(&mut self, bound: usize) -> usize {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 % bound.max(1) as u64) as usize
    }
}

/// Real updates from two editors: inserts, deletes, and exchanges that create dependencies.
fn edit_history(rng: &mut Rng) -> Vec<Vec<u8>> {
    let docs = [Doc::with_client_id(11), Doc::with_client_id(12)];
    let mut updates: Vec<Vec<u8>> = Vec::new();
    for _ in 0..60 {
        let doc = &docs[rng.next(2)];
        let text = doc.get_or_insert_text("title");
        let mut txn = doc.transact_mut();
        let len = text.len(&txn);
        if len > 4 && rng.next(3) == 0 {
            let at = rng.next(len as usize - 2) as u32;
            text.remove_range(&mut txn, at, 2);
        } else {
            let at = rng.next(len as usize + 1) as u32;
            text.insert(&mut txn, at, "abc");
        }
        updates.push(txn.encode_update_v1());
        drop(txn);
        if rng.next(4) == 0 {
            for update in &updates {
                for doc in &docs {
                    let update = yrs::Update::decode_v1(update).unwrap();
                    doc.transact_mut().apply_update(update).unwrap();
                }
            }
        }
    }
    updates
}

/// What a client without a gap would hold: every update, in the order they were made.
fn merged(updates: &[Vec<u8>]) -> Doc {
    let doc = Doc::new();
    for update in updates {
        let update = yrs::Update::decode_v1(update).unwrap();
        doc.transact_mut().apply_update(update).unwrap();
    }
    doc
}

/// The seeds stay low on purpose: seed 1236 fails, and so does a plain yrs `Doc` given that order,
/// without cadocs. Real clients send in order over one socket, and resync through the handshake.
#[tokio::test(start_paused = true)]
async fn real_updates_in_any_order_survive_a_reload() {
    for seed in 1..=200 {
        let mut rng = Rng(seed * 0x9e37_79b9 + 1);
        let mut updates = edit_history(&mut rng);
        let expected = title(&merged(&updates));
        for at in (1..updates.len()).rev() {
            updates.swap(at, rng.next(at + 1));
        }

        let store = MemStore::new();
        let first = compacting(&store, 20);
        let writer = Peer::join(&first, ReadWrite).await;
        for update in updates {
            writer
                .send(Message::Sync(SyncMessage::Update(update)))
                .unwrap();
            if rng.next(10) == 0 {
                sleep(FLUSH).await;
            }
        }
        sleep(FLUSH).await;

        let live = Peer::join(&first, ReadWrite).await;
        assert_eq!(title(&live.doc), expected, "live, seed {seed}");
        let second = registry(&store);
        assert_eq!(
            connect_title(&second).await,
            expected,
            "reload, seed {seed}"
        );
    }
}

// Findings of the second review

#[tokio::test(start_paused = true)]
async fn a_stuck_update_does_not_double_later_edits() {
    let store = MemStore::new();
    let registry = registry(&store);
    let alice = Peer::join(&registry, ReadWrite).await;
    let bob = Peer::join(&registry, ReadWrite).await;
    let _gap = alice.edit("one ");
    let waiting = alice.edit("two");
    alice
        .send(Message::Sync(SyncMessage::Update(waiting)))
        .unwrap();
    sleep(FLUSH).await;
    let rows = store.stored(DOC).updates.len();

    bob.write("b").unwrap();
    sleep(FLUSH).await;

    assert_eq!(store.stored(DOC).updates.len(), rows + 1);
}

#[tokio::test(start_paused = true)]
async fn an_empty_sync_step2_passes_a_full_queue() {
    let store = MemStore::new();
    let config = CollabConfig {
        room_queue_bytes: 100,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let mut writer = Peer::join(&registry, ReadWrite).await;
    store.fail_append.store(true, Ordering::SeqCst);
    writer.write(&"a".repeat(150)).unwrap();
    sleep(FLUSH).await;

    let empty = yrs::Update::EMPTY_V1.to_vec();
    writer
        .send(Message::Sync(SyncMessage::SyncStep2(empty)))
        .unwrap();
    writer.pump().await;

    assert_eq!(writer.closed, None);
}

#[tokio::test(start_paused = true)]
async fn a_large_document_reaches_a_client_with_a_small_frame_queued() {
    let store = MemStore::new();
    let config = CollabConfig {
        max_frame_bytes: 1000,
        ..config()
    };
    let registry = DocRegistry::new(store.clone(), config);
    let alice = Peer::join(&registry, ReadWrite).await;
    for _ in 0..3 {
        alice.write(&"a".repeat(400)).unwrap();
    }

    let mut bob = Peer::new(registry.connect(DOC, ReadWrite, AUTHOR).await.unwrap());
    bob.sync().await;

    assert_eq!(bob.closed, None);
    assert_eq!(title(&bob.doc).len(), 1200);
}

#[tokio::test(start_paused = true)]
async fn an_awareness_state_must_be_json() {
    let store = MemStore::new();
    let registry = registry(&store);
    let mut watcher = Peer::join(&registry, ReadWrite).await;
    let reader = Peer::join(&registry, ReadOnly).await;

    let refused = reader.send(Message::Awareness(awareness_update([(1, "{".to_owned())])));
    watcher.pump().await;

    assert_eq!(refused, Err(CLOSE_PROTOCOL_ERROR));
    assert!(watcher.present().is_empty());
}

#[tokio::test(start_paused = true)]
async fn a_failing_compaction_backs_off() {
    let store = MemStore::new();
    let registry = compacting(&store, 1);
    let alice = Peer::join(&registry, ReadWrite).await;
    store.fail_compact.store(true, Ordering::SeqCst);

    for _ in 0..20 {
        alice.write("x").unwrap();
        sleep(FLUSH).await;
    }

    assert!(store.compacts() <= 2, "{} attempts", store.compacts());
}
