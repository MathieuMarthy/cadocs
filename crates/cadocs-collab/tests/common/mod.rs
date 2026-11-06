//! An in-memory store with failure injection and gates, and a y-sync peer.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cadocs_collab::{CollabConfig, Connection, DocRegistry, Outbound};
use cadocs_core::permissions::SocketMode;
use cadocs_store::{DocEdit, DocStore, Snapshot, StoreError, StoredDoc, StoredUpdate};
use tokio::sync::Notify;
use tokio::time::timeout;
use yrs::sync::{Awareness, Message, SyncMessage};
use yrs::updates::decoder::Decode;
use yrs::updates::encoder::Encode;
use yrs::{Doc, GetString, Map, ReadTxn, StateVector, Text, TextPrelim, Transact, Update};

pub const DOC: &str = "doc";
pub const AUTHOR: &str = "ada";
pub const SESSION: &str = "session of ada";

/// Pauses a store call until it is released, so a test can act in the middle of it.
#[derive(Default)]
pub struct Gate {
    held: AtomicBool,
    entered: Notify,
    release: Notify,
}

impl Gate {
    pub fn hold(&self) {
        self.held.store(true, Ordering::SeqCst);
    }

    pub async fn entered(&self) {
        self.entered.notified().await;
    }

    pub fn release(&self) {
        self.held.store(false, Ordering::SeqCst);
        self.release.notify_waiters();
    }

    async fn pass(&self) {
        if self.held.load(Ordering::SeqCst) {
            let released = self.release.notified();
            self.entered.notify_one();
            released.await;
        }
    }
}

#[derive(Default)]
pub struct Stats {
    pub loads: usize,
    pub appends: usize,
    pub compacts: usize,
}

#[derive(Default)]
pub struct MemStore {
    docs: Mutex<HashMap<String, StoredDoc>>,
    /// The edit of each flush that stored something, in order.
    edits: Mutex<Vec<DocEdit>>,
    stats: Mutex<Stats>,
    /// The page of every document is gone.
    pub page_gone: AtomicBool,
    pub fail_load: AtomicBool,
    pub fail_append: AtomicBool,
    /// Appends that fail before the store recovers.
    pub fail_next_appends: AtomicUsize,
    pub fail_compact: AtomicBool,
    pub load_gate: Gate,
    pub append_gate: Gate,
    pub compact_gate: Gate,
}

fn failure() -> StoreError {
    StoreError::Database(sqlx::Error::PoolTimedOut)
}

impl MemStore {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    pub fn stored(&self, doc_id: &str) -> StoredDoc {
        self.docs
            .lock()
            .unwrap()
            .get(doc_id)
            .cloned()
            .unwrap_or_default()
    }

    pub fn edits(&self) -> Vec<DocEdit> {
        self.edits.lock().unwrap().clone()
    }

    pub fn loads(&self) -> usize {
        self.stats.lock().unwrap().loads
    }

    pub fn appends(&self) -> usize {
        self.stats.lock().unwrap().appends
    }

    pub fn compacts(&self) -> usize {
        self.stats.lock().unwrap().compacts
    }
}

impl DocStore for MemStore {
    async fn ping(&self) -> Result<(), StoreError> {
        Ok(())
    }

    async fn load(&self, doc_id: &str) -> Result<StoredDoc, StoreError> {
        self.stats.lock().unwrap().loads += 1;
        self.load_gate.pass().await;
        if self.fail_load.load(Ordering::SeqCst) {
            return Err(failure());
        }
        Ok(self.stored(doc_id))
    }

    async fn append_updates(
        &self,
        doc_id: &str,
        updates: &[StoredUpdate],
        edit: &DocEdit,
    ) -> Result<(), StoreError> {
        self.stats.lock().unwrap().appends += 1;
        self.append_gate.pass().await;
        if self.page_gone.load(Ordering::SeqCst) {
            return Err(StoreError::PageNotFound);
        }
        let once = self
            .fail_next_appends
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1));
        if self.fail_append.load(Ordering::SeqCst) || once.is_ok() {
            return Err(failure());
        }
        let mut docs = self.docs.lock().unwrap();
        let doc = docs.entry(doc_id.to_owned()).or_default();
        let covered = doc.snapshot.as_ref().map_or(0, |s| s.upto_seq);
        for update in updates {
            let stored = doc.updates.iter().find(|u| u.seq == update.seq);
            if update.seq <= covered || stored.is_some_and(|u| u.data != update.data) {
                return Err(StoreError::SeqConflict {
                    doc_id: doc_id.to_owned(),
                    seq: update.seq,
                });
            }
            if stored.is_none() {
                doc.updates.push(update.clone());
            }
        }
        self.edits.lock().unwrap().push(edit.clone());
        Ok(())
    }

    async fn compact(&self, doc_id: &str, snapshot: &Snapshot) -> Result<(), StoreError> {
        self.stats.lock().unwrap().compacts += 1;
        self.compact_gate.pass().await;
        if self.page_gone.load(Ordering::SeqCst) {
            return Err(StoreError::PageNotFound);
        }
        if self.fail_compact.load(Ordering::SeqCst) {
            return Err(failure());
        }
        let mut docs = self.docs.lock().unwrap();
        let doc = docs.entry(doc_id.to_owned()).or_default();
        if let Some(stored) = &doc.snapshot
            && stored.upto_seq > snapshot.upto_seq
        {
            return Err(StoreError::StaleSnapshot {
                doc_id: doc_id.to_owned(),
                stored: stored.upto_seq,
                offered: snapshot.upto_seq,
            });
        }
        doc.snapshot = Some(snapshot.clone());
        doc.updates.retain(|u| u.seq > snapshot.upto_seq);
        Ok(())
    }
}

pub fn config() -> CollabConfig {
    CollabConfig {
        flush_interval: Duration::from_millis(500),
        compact_updates: 500,
        room_idle: Duration::from_secs(60),
        ..CollabConfig::default()
    }
}

/// An update that decodes, but that `apply_update` rejects once `base` is in the document: its
/// parent is an id that `base` gives to plain text instead of a shared type.
pub fn inapplicable_update() -> (Vec<u8>, Vec<u8>) {
    let client = 3;
    let plain = Doc::with_client_id(client);
    let title = plain.get_or_insert_text("title");
    let base = {
        let mut txn = plain.transact_mut();
        title.push(&mut txn, "a");
        txn.encode_update_v1()
    };

    let nesting = Doc::with_client_id(client);
    let map = nesting.get_or_insert_map("m");
    let nested = map.insert(&mut nesting.transact_mut(), "k", TextPrelim::new(""));
    let mut txn = nesting.transact_mut();
    nested.push(&mut txn, "x");
    (base, txn.encode_update_v1())
}

/// Rebuilds a document from what is stored, the way a room load does.
pub fn rebuild(stored: &StoredDoc) -> Doc {
    let doc = Doc::new();
    let mut txn = doc.transact_mut();
    if let Some(snapshot) = &stored.snapshot {
        txn.apply_update(Update::decode_v1(&snapshot.state).unwrap())
            .unwrap();
    }
    for update in &stored.updates {
        txn.apply_update(Update::decode_v1(&update.data).unwrap())
            .unwrap();
    }
    drop(txn);
    doc
}

pub fn title(doc: &Doc) -> String {
    let text = doc.get_or_insert_text("title");
    text.get_string(&doc.transact())
}

pub fn state_vector(doc: &Doc) -> StateVector {
    doc.transact().state_vector()
}

pub fn frame(message: Message) -> Vec<u8> {
    message.encode_v1()
}

/// A y-sync client: a local document on one side of a `Connection`.
pub struct Peer {
    pub doc: Doc,
    pub awareness: Awareness,
    pub conn: Connection,
    /// Auth frames received.
    pub denied: Vec<Option<String>>,
    /// Update frames received since the start.
    pub updates: usize,
    pub closed: Option<u16>,
}

impl Peer {
    pub fn new(conn: Connection) -> Self {
        Self::with_doc(conn, Doc::new())
    }

    pub fn with_doc(conn: Connection, doc: Doc) -> Self {
        Self {
            awareness: Awareness::new(doc.clone()),
            doc,
            conn,
            denied: Vec::new(),
            updates: 0,
            closed: None,
        }
    }

    /// Joins and completes the y-sync handshake.
    pub async fn join<S: DocStore>(registry: &DocRegistry<S>, mode: SocketMode) -> Self {
        let conn = registry.connect(DOC, mode, AUTHOR, SESSION).await.unwrap();
        let mut peer = Self::new(conn);
        peer.sync().await;
        peer
    }

    pub fn send(&self, message: Message) -> Result<(), u16> {
        self.conn.receive(&frame(message)).map_err(|close| close.0)
    }

    /// Sends the client's SyncStep1, then handles everything the server sent.
    pub async fn sync(&mut self) {
        let sv = state_vector(&self.doc);
        let _ = self.send(Message::Sync(SyncMessage::SyncStep1(sv)));
        self.pump().await;
    }

    /// Handles every frame already queued for this client.
    pub async fn pump(&mut self) {
        while self.closed.is_none() {
            match timeout(Duration::from_millis(1), self.conn.next()).await {
                Err(_) => return,
                Ok(Outbound::Close(code)) => self.closed = Some(code),
                Ok(Outbound::Frame(bytes)) => self.handle(&bytes),
            }
        }
    }

    fn handle(&mut self, bytes: &[u8]) {
        match Message::decode_v1(bytes).unwrap() {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                let update = self.doc.transact().encode_state_as_update_v1(&sv);
                let _ = self.send(Message::Sync(SyncMessage::SyncStep2(update)));
            }
            Message::Sync(SyncMessage::SyncStep2(update)) => self.apply(&update),
            Message::Sync(SyncMessage::Update(update)) => {
                self.updates += 1;
                self.apply(&update);
            }
            Message::Awareness(update) => self.awareness.apply_update(update).unwrap(),
            Message::Auth(reason) => self.denied.push(reason),
            other => panic!("unexpected frame {other:?}"),
        }
    }

    fn apply(&self, update: &[u8]) {
        self.doc
            .transact_mut()
            .apply_update(Update::decode_v1(update).unwrap())
            .unwrap();
    }

    /// Edits the title locally and returns the update, without sending it.
    pub fn edit(&self, text: &str) -> Vec<u8> {
        let title = self.doc.get_or_insert_text("title");
        let mut txn = self.doc.transact_mut();
        title.push(&mut txn, text);
        txn.encode_update_v1()
    }

    /// Edits the title and sends the update.
    pub fn write(&self, text: &str) -> Result<(), u16> {
        let update = self.edit(text);
        self.send(Message::Sync(SyncMessage::Update(update)))
    }

    /// Announces this client's presence.
    pub fn announce(&mut self, state: &str) {
        self.awareness.set_local_state_raw(state);
        let update = self.awareness.update().unwrap();
        self.send(Message::Awareness(update)).unwrap();
    }

    /// The client ids with a live awareness state, as this peer sees them.
    pub fn present(&self) -> Vec<u64> {
        let mut ids: Vec<_> = self
            .awareness
            .iter()
            .filter(|(_, state)| state.data.is_some())
            .map(|(id, _)| id.get())
            .collect();
        ids.sort_unstable();
        ids
    }
}
