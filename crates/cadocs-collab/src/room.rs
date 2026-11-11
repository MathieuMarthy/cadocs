use std::collections::HashMap;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cadocs_core::permissions::SocketMode;
use cadocs_store::{DocEdit, Snapshot, StoredDoc, StoredUpdate};
use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::Value;
use tokio::sync::{Notify, OwnedSemaphorePermit, mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::warn;
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{Awareness, AwarenessUpdate, Message, SyncMessage};
use yrs::updates::encoder::Encode;
use yrs::{
    ClientID, Doc, GetString, IdSet, ReadTxn, StateVector, Text, Transact, TransactionMut, Update,
    WriteTxn,
};

use crate::connection::{Close, Connection, decode_untrusted};
use crate::{
    CLOSE_PAGE_GONE, CLOSE_PROTOCOL_ERROR, CLOSE_STORAGE_FAILING, CLOSE_TOO_SLOW, CollabConfig,
    CollabError, ROOM_BYTES_PER_ENCODED_BYTE, ROOM_MIN_BYTES,
};

/// `None` until a load or an unload ends.
pub(crate) type Transition = Option<Result<(), CollabError>>;

const READ_ONLY_REASON: &str = "read-only";
/// Client ids one socket may ever introduce. A tab uses one, and the server keeps a clock for each
/// id until the room unloads, so this bounds what a socket can leave behind.
const MAX_ANNOUNCED_IDS: usize = 16;
/// Largest awareness state: a name, a colour and a cursor take well under 1 KiB.
const MAX_AWARENESS_STATE_BYTES: usize = 64 * 1024;
/// States a room holds at once, and their bytes. Past this a state is ignored and its socket stays:
/// presence is the only thing lost, and the client tries again at its next heartbeat.
const MAX_ROOM_STATES: usize = 256;
const MAX_ROOM_STATE_BYTES: usize = 1024 * 1024;
/// Ids a room remembers, gone ones included. yrs keeps a clock for each id it ever saw, so a room
/// that never empties would grow with each reload of a tab. Past this it forgets the gone ones.
const MAX_ROOM_KNOWN_IDS: usize = 4096;
/// Entries of one awareness frame: an honest client echoes what the room holds, plus its own.
const MAX_FRAME_STATES: usize = MAX_ROOM_STATES + MAX_ANNOUNCED_IDS;
/// How far past `compact_updates` a log may grow while an update waits for a dependency. Past this
/// the dependency counts as lost, and compaction drops what waits for it.
const WAITING_LOG_FACTOR: usize = 4;
const NULL_STATE: &str = "null";
/// The root type that holds the title (`docs/DATA_MODEL.md`).
const TITLE: &str = "title";

/// Poisoning is ignored: the critical sections keep no invariant a panic could break that matters
/// more than keeping the room's unflushed edits reachable.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Client {
    mode: SocketMode,
    /// The user behind this socket, shared by the edits it makes.
    user: Arc<str>,
    /// The hash of the session that opened it.
    session: Arc<str>,
    frames: mpsc::Sender<Arc<[u8]>>,
    /// Bytes waiting in `frames`. A frame count alone would let a client that requests the whole
    /// document and never reads pin `send_queue` copies of it.
    queued_bytes: Arc<AtomicUsize>,
    max_queued_bytes: usize,
    /// `None` once the client was told to close.
    close: Option<oneshot::Sender<u16>>,
    /// Ids this socket introduced to the room, removed ones included.
    announced: usize,
    told_read_only: bool,
}

impl Client {
    fn send(&mut self, frame: Arc<[u8]>) {
        if self.close.is_none() {
            return;
        }
        let queued = self.queued_bytes.load(Ordering::Relaxed);
        // Checked before adding, so a document larger than the budget can still be sent.
        let over_budget = queued > self.max_queued_bytes;
        let bytes = frame.len();
        if over_budget || self.frames.try_send(frame).is_err() {
            self.kick(CLOSE_TOO_SLOW);
        } else {
            self.queued_bytes.fetch_add(bytes, Ordering::Relaxed);
        }
    }

    fn kick(&mut self, code: u16) {
        if let Some(close) = self.close.take() {
            let _ = close.send(code);
        }
    }
}

/// Who announced an awareness state.
struct StateOwner {
    socket: u64,
    bytes: usize,
}

/// A socket a recheck must look at.
pub(crate) struct OpenSocket {
    pub(crate) id: u64,
    pub(crate) mode: SocketMode,
    pub(crate) user: Arc<str>,
    pub(crate) session: Arc<str>,
}

/// A loaded document. All of this is reached under one short lock, never held across an await.
pub(crate) struct RoomState {
    awareness: Awareness,
    /// Filled by the document observer: the part of an update that really changed the document.
    applied: Arc<Mutex<Option<Vec<u8>>>>,
    clients: HashMap<u64, Client>,
    next_client: u64,
    /// Every awareness state the room holds, removed when its socket closes.
    states: HashMap<ClientID, StateOwner>,
    /// Sum of `states`.
    state_bytes: usize,
    /// Ids `awareness` holds a clock for.
    known_ids: usize,
    /// Updates the room task has not picked up yet.
    pub(crate) queue: Vec<StoredUpdate>,
    /// Who wrote the last of `queue`.
    author: Option<Arc<str>>,
    /// Bytes not stored yet, in `queue` or in the room task's batch.
    pub(crate) unflushed_bytes: usize,
    next_seq: i64,
    /// Highest `seq` known to be stored.
    pub(crate) flushed_seq: i64,
    /// Rows of the log a compaction would remove.
    pub(crate) stored_updates: usize,
    /// Set while the room has no client.
    pub(crate) idle_since: Option<Instant>,
    /// The last flush failed.
    pub(crate) failing: bool,
    /// A stored row could not be applied. A snapshot would delete it for good, so the log stays.
    skipped_row: bool,
    /// Set when an unload is requested, and resolved when it ends.
    pub(crate) unload: Option<watch::Sender<Transition>>,
    /// Someone entered the room since the load: a client, or the server writing to it. Until then
    /// the room is idle only because whoever asked for it has not entered yet, so a cap must not
    /// evict it.
    pub(crate) joined: bool,
    /// `Room::estimate` when it was last measured from the document. Updates add to the estimate
    /// one by one, which counts far more than they cost once merged, so past twice this the
    /// estimate is measured again: a room that never compacts does not grow without limit.
    measured: usize,
}

/// Builds a document from what is stored, and tells whether a row had to be skipped. This can panic
/// when its transaction commits, so a caller holds a guard.
pub(crate) fn rebuild(doc_id: &str, stored: &StoredDoc) -> Result<(Doc, bool), CollabError> {
    let corrupt = || CollabError::CorruptDocument {
        doc_id: doc_id.to_owned(),
    };
    let doc = Doc::new();
    let mut skipped_row = false;
    {
        let mut txn = doc.transact_mut();
        if let Some(snapshot) = &stored.snapshot {
            let update: Update = decode_untrusted(&snapshot.state).map_err(|_| corrupt())?;
            let applied = catch_unwind(AssertUnwindSafe(|| txn.apply_update(update)));
            if !matches!(applied, Ok(Ok(()))) {
                return Err(corrupt());
            }
        }
        for row in &stored.updates {
            let update: Update = decode_untrusted(&row.data).map_err(|_| corrupt())?;
            // A stored update can turn out invalid once its dependency arrives. Refusing the
            // whole document for it would let one bad frame lock everybody out.
            let applied = catch_unwind(AssertUnwindSafe(|| txn.apply_update(update)));
            if !matches!(applied, Ok(Ok(()))) {
                warn!(%doc_id, seq = row.seq, "skipped a stored update that cannot be applied");
                skipped_row = true;
            }
        }
    }
    Ok((doc, skipped_row))
}

impl RoomState {
    fn others(&mut self, except: u64) -> impl Iterator<Item = &mut Client> {
        self.clients
            .iter_mut()
            .filter(move |(id, _)| **id != except)
            .map(|(_, client)| client)
    }

    fn broadcast(&mut self, except: u64, message: &Message) {
        if self.clients.keys().all(|client| *client == except) {
            return;
        }
        let frame: Arc<[u8]> = message.encode_v1().into();
        for client in self.others(except) {
            client.send(frame.clone());
        }
    }

    fn broadcast_all(&mut self, message: &Message) {
        let frame: Arc<[u8]> = message.encode_v1().into();
        for client in self.clients.values_mut() {
            client.send(frame.clone());
        }
    }

    fn reply(&mut self, id: u64, message: Message) {
        if let Some(client) = self.clients.get_mut(&id) {
            client.send(message.encode_v1().into());
        }
    }

    /// One update may exceed the limit on an empty queue. Refusing it would make its client
    /// reconnect and resend it forever, and `max_frame_bytes` still bounds it. While storage works
    /// the next flush drains the queue, so a burst gets one more frame of room: `1013` means
    /// storage is failing, and a large offline edit must not close everybody.
    fn queue_refuses(&self, bytes: usize, config: &CollabConfig) -> bool {
        let limit = match self.failing {
            true => config.room_queue_bytes,
            false => config.room_queue_bytes + config.max_frame_bytes,
        };
        self.unflushed_bytes > 0 && self.unflushed_bytes + bytes > limit
    }

    pub(crate) fn encode_full(&self) -> Vec<u8> {
        self.awareness
            .doc()
            .transact()
            .encode_state_as_update_v1(&StateVector::default())
    }

    /// The document without what still waits for a dependency.
    fn encode_applied(&self) -> Vec<u8> {
        self.awareness
            .doc()
            .transact()
            .encode_diff_v1(&StateVector::default())
    }

    /// Starts over from the live states, which keep their clocks. A clock forgotten this way only
    /// lets an old state of a gone client back in, and that client would have to send it.
    fn forget_gone_ids(&mut self) {
        if self.known_ids <= MAX_ROOM_KNOWN_IDS {
            return;
        }
        let Ok(live) = self.awareness.update() else {
            return;
        };
        let mut awareness = Awareness::new(self.awareness.doc().clone());
        if awareness.apply_update(live).is_ok() {
            self.awareness = awareness;
            self.known_ids = self.states.len();
        }
    }

    fn own_state(&mut self, client: ClientID, owner: StateOwner) {
        self.state_bytes += owner.bytes;
        if let Some(previous) = self.states.insert(client, owner) {
            self.state_bytes -= previous.bytes;
        }
    }

    fn forget_state(&mut self, client: ClientID) {
        if let Some(previous) = self.states.remove(&client) {
            self.state_bytes -= previous.bytes;
        }
    }

    /// `None` for a document with no title yet. The panic guard is for the room task, which must
    /// survive to flush.
    fn title(&self) -> Option<String> {
        let doc = self.awareness.doc();
        catch_unwind(AssertUnwindSafe(|| {
            let txn = doc.transact();
            let text = txn.get_text(TITLE)?;
            Some(DocEdit::clip_title(text.get_string(&txn)))
        }))
        .unwrap_or_else(|_| {
            warn!("cannot read the title of a document");
            None
        })
    }

    pub(crate) fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    pub(crate) fn kick_all(&mut self, code: u16) {
        for client in self.clients.values_mut() {
            client.kick(code);
        }
    }
}

/// What a document of `encoded` bytes is counted as once loaded.
fn room_estimate(encoded: usize) -> usize {
    encoded
        .saturating_mul(ROOM_BYTES_PER_ENCODED_BYTE)
        .max(ROOM_MIN_BYTES)
}

pub(crate) struct Room {
    pub(crate) doc_id: String,
    /// What the room is counted as against `room_memory`: set from the stored size at load and at
    /// each compaction, and grown by each update in between.
    pub(crate) estimate: AtomicUsize,
    state: Mutex<RoomState>,
    /// Wakes the room task: first queued update, last client gone, unload requested.
    pub(crate) wake: Notify,
    config: CollabConfig,
}

impl Room {
    /// Rebuilds the document: snapshot, then the updates it does not cover.
    pub(crate) fn build(
        doc_id: String,
        stored: StoredDoc,
        config: CollabConfig,
    ) -> Result<Arc<Self>, CollabError> {
        let last_seq = stored.last_seq();
        let stored_updates = stored.updates.len();
        let encoded = stored.snapshot.as_ref().map_or(0, |s| s.state.len())
            + stored.updates.iter().map(|u| u.data.len()).sum::<usize>();
        let (doc, skipped_row) = rebuild(&doc_id, &stored)?;

        let applied = Arc::new(Mutex::new(None));
        let sink = applied.clone();
        doc.observe_update_v1("room", move |_, event| {
            *lock(&sink) = Some(event.update.clone());
        })
        .expect("a new document has no open transaction");

        let estimate = room_estimate(encoded);
        Ok(Arc::new(Self {
            doc_id,
            estimate: AtomicUsize::new(estimate),
            state: Mutex::new(RoomState {
                awareness: Awareness::new(doc),
                applied,
                clients: HashMap::new(),
                next_client: 0,
                states: HashMap::new(),
                state_bytes: 0,
                known_ids: 0,
                queue: Vec::new(),
                author: None,
                unflushed_bytes: 0,
                next_seq: last_seq + 1,
                flushed_seq: last_seq,
                stored_updates,
                idle_since: Some(Instant::now()),
                failing: false,
                skipped_row,
                unload: None,
                measured: estimate,
                joined: false,
            }),
            wake: Notify::new(),
            config,
        }))
    }

    pub(crate) fn state(&self) -> MutexGuard<'_, RoomState> {
        lock(&self.state)
    }

    pub(crate) fn max_frame_bytes(&self) -> usize {
        self.config.max_frame_bytes
    }

    /// The sockets not told to close yet.
    pub(crate) fn sockets(&self) -> Vec<OpenSocket> {
        self.state()
            .clients
            .iter()
            .filter(|(_, client)| client.close.is_some())
            .map(|(id, client)| OpenSocket {
                id: *id,
                mode: client.mode,
                user: client.user.clone(),
                session: client.session.clone(),
            })
            .collect()
    }

    /// Tells one socket to close. Its frames are ignored from here on.
    pub(crate) fn kick(&self, id: u64, code: u16) {
        if let Some(client) = self.state().clients.get_mut(&id) {
            client.kick(code);
        }
    }

    pub(crate) fn mode(&self, id: u64) -> SocketMode {
        self.state()
            .clients
            .get(&id)
            .map_or(SocketMode::ReadOnly, |client| client.mode)
    }

    /// Adds a client and queues the server's side of the handshake. The registry calls this under
    /// its lock, so the room cannot start unloading at the same time.
    pub(crate) fn join(
        self: &Arc<Self>,
        mode: SocketMode,
        user: &str,
        session: &str,
        permit: OwnedSemaphorePermit,
    ) -> Connection {
        // The handshake alone takes two frames.
        let (frames, frames_rx) = mpsc::channel(self.config.send_queue.max(2));
        let (close, close_rx) = oneshot::channel();
        let queued_bytes = Arc::new(AtomicUsize::new(0));
        let mut state = self.state();
        let id = state.next_client;
        state.next_client += 1;
        state.idle_since = None;
        state.clients.insert(
            id,
            Client {
                mode,
                user: user.into(),
                session: session.into(),
                frames,
                queued_bytes: queued_bytes.clone(),
                max_queued_bytes: self.config.max_frame_bytes,
                close: Some(close),
                announced: 0,
                told_read_only: false,
            },
        );

        let sv = state.awareness.doc().transact().state_vector();
        state.reply(id, Message::Sync(SyncMessage::SyncStep1(sv)));
        if let Ok(update) = state.awareness.update()
            && !update.clients.is_empty()
        {
            state.reply(id, Message::Awareness(update));
        }
        drop(state);
        Connection::new(self.clone(), id, frames_rx, queued_bytes, close_rx, permit)
    }

    pub(crate) fn handle(&self, id: u64, message: Message) -> Result<(), Close> {
        let mut state = self.state();
        let Some(client) = state.clients.get(&id) else {
            return Ok(());
        };
        if client.close.is_none() {
            // Already told to close: its edits come back through the next handshake.
            return Ok(());
        }
        let mode = client.mode;
        match message {
            Message::Sync(SyncMessage::SyncStep1(sv)) => {
                // This includes updates still waiting, unlike `encode_diff_v1`: without them a joiner
                // misses content (`real_updates_in_any_order_survive_a_reload`).
                let update = state
                    .awareness
                    .doc()
                    .transact()
                    .encode_state_as_update_v1(&sv);
                state.reply(id, Message::Sync(SyncMessage::SyncStep2(update)));
                Ok(())
            }
            Message::Sync(SyncMessage::SyncStep2(update) | SyncMessage::Update(update)) => {
                match mode {
                    SocketMode::ReadWrite => self.apply(&mut state, id, &update),
                    SocketMode::ReadOnly => {
                        deny_write(&mut state, id);
                        Ok(())
                    }
                }
            }
            Message::Awareness(mut update) => {
                admit_awareness(&mut state, id, &mut update)?;
                let summary = state
                    .awareness
                    .apply_update_summary(update)
                    .map_err(|_| Close(CLOSE_PROTOCOL_ERROR))?;
                if let Some(summary) = summary {
                    let update = state
                        .awareness
                        .update_with_clients(summary.all_changes())
                        .map_err(|_| Close(CLOSE_PROTOCOL_ERROR))?;
                    // Only real changes change ownership: `y-websocket` echoes the states of
                    // other clients back, with a clock the server already knows.
                    for changed in summary.added.iter().chain(&summary.updated) {
                        let entry = update.clients.get(changed);
                        let bytes = entry.map_or(0, |entry| entry.json.len());
                        state.own_state(*changed, StateOwner { socket: id, bytes });
                    }
                    for removed in &summary.removed {
                        state.forget_state(*removed);
                    }
                    // To the sender too, as `y-websocket`'s server does: its client reconnects
                    // after 30 seconds of silence, and this is all it hears when alone.
                    state.broadcast_all(&Message::Awareness(update));
                }
                state.forget_gone_ids();
                Ok(())
            }
            Message::AwarenessQuery => {
                if let Ok(update) = state.awareness.update() {
                    state.reply(id, Message::Awareness(update));
                }
                Ok(())
            }
            Message::Auth(_) | Message::Custom(..) => Err(Close(CLOSE_PROTOCOL_ERROR)),
        }
    }

    fn apply(&self, state: &mut RoomState, id: u64, bytes: &[u8]) -> Result<(), Close> {
        let update: Update = decode_untrusted(bytes)?;
        let insertions = update.insertions(true);
        let deletes = update.delete_set().clone();

        // A frame without new blocks is let through: each reconnect sends one, and refusing it
        // would keep writers from staying connected during an outage.
        let known = state.awareness.doc().transact().state_vector();
        if reaches_past(&insertions, &known) && state.queue_refuses(bytes.len(), &self.config) {
            for client in state.clients.values_mut() {
                if client.mode == SocketMode::ReadWrite {
                    client.kick(CLOSE_STORAGE_FAILING);
                }
            }
            return Ok(());
        }

        // The transaction commits when it drops, panic or not, which runs the observer. So what a
        // crafted update did change is still stored and broadcast below, and the room stays usable.
        let doc = state.awareness.doc();
        let result = catch_unwind(AssertUnwindSafe(|| doc.transact_mut().apply_update(update)));
        let applied = lock(&state.applied).take();
        // yrs keeps an update with a missing dependency outside the document, and the observer
        // does not see it. A frame still reaching past the document after being applied is such
        // an update, by its blocks or by deletes of blocks not seen yet. The frame itself is
        // stored then, or a restart would lose that part.
        let valid = matches!(result, Ok(Ok(())));
        let now_known = state.awareness.doc().transact().state_vector();
        let waits = reaches_past(&insertions, &now_known) || reaches_past(&deletes, &now_known);
        let raw = (valid && waits).then(|| bytes.to_vec());
        // The update moves through the broadcast into the queue: a large paste must not be copied
        // more than needed under the room lock.
        let applied = applied.map(|update| {
            let message = Message::Sync(SyncMessage::Update(update));
            state.broadcast(id, &message);
            match message {
                Message::Sync(SyncMessage::Update(update)) => update,
                _ => unreachable!("built just above"),
            }
        });
        let author = state.clients.get(&id).map(|client| client.user.clone());
        // Both can exist: `applied` may hold older updates that this one released, which the frame
        // does not contain.
        for data in applied.into_iter().chain(raw) {
            self.enqueue(state, data, author.clone());
        }
        valid.then_some(()).ok_or(Close(CLOSE_PROTOCOL_ERROR))
    }

    fn enqueue(&self, state: &mut RoomState, data: Vec<u8>, author: Option<Arc<str>>) {
        let seq = state.next_seq;
        state.next_seq += 1;
        state.unflushed_bytes += data.len();
        let grown = self
            .estimate
            .fetch_add(
                data.len().saturating_mul(ROOM_BYTES_PER_ENCODED_BYTE),
                Ordering::Relaxed,
            )
            .saturating_add(data.len().saturating_mul(ROOM_BYTES_PER_ENCODED_BYTE));
        if grown > state.measured.saturating_mul(2) {
            // The same encoding as a compaction, guarded the same way. A failure keeps the count.
            if let Ok(encoded) = catch_unwind(AssertUnwindSafe(|| state.encode_full())) {
                let measured = room_estimate(encoded.len());
                self.estimate.store(measured, Ordering::Relaxed);
                state.measured = measured;
            }
        }
        if state.queue.is_empty() {
            self.wake.notify_one();
        }
        state.queue.push(StoredUpdate { seq, data });
        state.author = author;
    }

    /// Replaces the title, as an edit every client receives. The registry calls this under its
    /// lock, so the room cannot start unloading at the same time.
    pub(crate) fn set_title(&self, title: &str, author: &str) -> Result<(), CollabError> {
        let plain = self.change(author, title.len(), |txn| {
            let text = txn.get_or_insert_text(TITLE);
            let current = text.get_string(txn);
            let length = text.len(txn);
            // A client can put more than text in this root. yrs counts it, but cannot remove it:
            // it would panic after deleting the text.
            let plain = current.len() == length as usize;
            if plain && current != title {
                text.remove_range(txn, 0, length);
                text.insert(txn, 0, title);
            }
            plain
        })?;
        plain.then_some(()).ok_or_else(|| self.corrupt())
    }

    /// Runs `action` in one transaction, whose update every client receives and the store gets as
    /// an edit by `author`. `bytes` is about what it writes, for the queue's bound. A panic in
    /// `action` is a corrupt document: what it wrote before still goes out, as any edit would.
    pub(crate) fn change<T>(
        &self,
        author: &str,
        bytes: usize,
        action: impl FnOnce(&mut TransactionMut) -> T,
    ) -> Result<T, CollabError> {
        let mut state = self.state();
        if state.queue_refuses(bytes, &self.config) {
            return Err(CollabError::StorageFailing);
        }
        let doc = state.awareness.doc();
        // The observer runs as this transaction drops, and a stored document can make yrs panic.
        let result = catch_unwind(AssertUnwindSafe(|| {
            let mut txn = doc.transact_mut();
            action(&mut txn)
        }));
        let applied = lock(&state.applied).take();
        if let Some(update) = applied {
            let message = Message::Sync(SyncMessage::Update(update));
            state.broadcast_all(&message);
            if let Message::Sync(SyncMessage::Update(update)) = message {
                self.enqueue(&mut state, update, Some(author.into()));
            }
        }
        result.map_err(|_| self.corrupt())
    }

    fn corrupt(&self) -> CollabError {
        CollabError::CorruptDocument {
            doc_id: self.doc_id.clone(),
        }
    }

    /// Removes a client and the awareness states it owned, so no ghost cursor stays.
    pub(crate) fn leave(&self, id: u64) {
        let mut state = self.state();
        if state.clients.remove(&id).is_none() {
            return;
        }
        let owned: Vec<ClientID> = state
            .states
            .iter()
            .filter(|(_, owner)| owner.socket == id)
            .map(|(client, _)| *client)
            .collect();
        for client in &owned {
            state.forget_state(*client);
        }
        // A null state at the same clock removes the entry everywhere, as `y-websocket`'s server
        // does. `remove_state` would bump the clock, and the client's next change after a
        // reconnect would be ignored as old.
        if !owned.is_empty()
            && let Ok(mut update) = state.awareness.update_with_clients(owned)
        {
            for entry in update.clients.values_mut() {
                entry.json = NULL_STATE.into();
            }
            let _ = state.awareness.apply_update(update.clone());
            state.broadcast(id, &Message::Awareness(update));
        }
        if state.clients.is_empty() {
            state.idle_since = Some(Instant::now());
            self.wake.notify_one();
        }
    }

    /// The state to compact, covering every stored `seq`. It may also hold edits still queued:
    /// these keep a higher `seq`, so a later flush is accepted and replaying them changes nothing.
    /// The document as it stands, for a reader. What waits for a dependency is not part of it, and
    /// a merge could lose a delete (see `snapshot`).
    pub(crate) fn read(&self) -> Result<StoredDoc, CollabError> {
        let state = self.state();
        let encoded = catch_unwind(AssertUnwindSafe(|| state.encode_applied()));
        drop(state);
        let state = encoded.map_err(|_| CollabError::CorruptDocument {
            doc_id: self.doc_id.clone(),
        })?;
        Ok(StoredDoc {
            snapshot: Some(Snapshot { state, upto_seq: 0 }),
            updates: Vec::new(),
        })
    }

    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        let mut state = self.state();
        if state.stored_updates == 0 || state.skipped_row {
            return None;
        }
        // yrs merges what is waiting into the encoded state, and that merge lost a delete in
        // `real_updates_in_any_order_survive_a_reload`. The log stays correct, so it waits, but not
        // forever. A connected writer resends what this drops, and real clients send in order, so
        // this loses a crafted update, or one whose writer left and whose dependency never came.
        let waiting = state.awareness.doc().transact().has_missing_updates();
        if waiting && state.stored_updates <= self.config.compact_updates * WAITING_LOG_FACTOR {
            return None;
        }
        if waiting {
            warn!(doc_id = %self.doc_id, "compacting without an update whose dependency never came");
        }
        // The panic guard matters here: this runs in the room task, which must survive to flush.
        let encoded = catch_unwind(AssertUnwindSafe(|| {
            if waiting {
                state.encode_applied()
            } else {
                state.encode_full()
            }
        }));
        match encoded {
            Ok(encoded) => {
                let measured = room_estimate(encoded.len());
                self.estimate.store(measured, Ordering::Relaxed);
                state.measured = measured;
                Some(Snapshot {
                    state: encoded,
                    upto_seq: state.flushed_seq,
                })
            }
            Err(_) => {
                warn!(doc_id = %self.doc_id, "cannot encode the document, keeping its log");
                None
            }
        }
    }

    /// Takes the queue for a flush, with what the row of the page must get. The title may include
    /// edits queued after this, which a later flush stores: it is only a copy.
    pub(crate) fn take_queue(&self) -> (Vec<StoredUpdate>, DocEdit) {
        let mut state = self.state();
        let queue = mem::take(&mut state.queue);
        let edit = DocEdit {
            title: (!queue.is_empty()).then(|| state.title()).flatten(),
            author: state.author.take().map(|author| author.to_string()),
        };
        (queue, edit)
    }

    /// Forgets what was not stored and closes every socket: the page is gone, and so are its edits.
    pub(crate) fn abandon(&self) {
        let mut state = self.state();
        state.queue.clear();
        state.author = None;
        state.unflushed_bytes = 0;
        state.stored_updates = 0;
        state.kick_all(CLOSE_PAGE_GONE);
    }
}

/// Applies the limits on an awareness update from a socket. Removing an id the room does not know
/// changes nothing, so it is dropped here instead of leaving a clock behind.
fn admit_awareness(
    state: &mut RoomState,
    id: u64,
    update: &mut AwarenessUpdate,
) -> Result<(), Close> {
    if update.clients.len() > MAX_FRAME_STATES {
        return Err(Close(CLOSE_PROTOCOL_ERROR));
    }
    let awareness = &state.awareness;
    update.clients.retain(|client, entry| {
        entry.json.as_ref() != NULL_STATE || awareness.meta(*client).is_some()
    });
    let Some(user) = state.clients.get(&id).map(|client| client.user.clone()) else {
        return Ok(());
    };
    // Dropped, not refused: `y-websocket` echoes what it hears, and removes a peer it has not heard
    // from. A reconnecting client reuses its id while its old socket may still be open, so a
    // socket of the same user takes the id over.
    let clients = &state.clients;
    let states = &state.states;
    update.clients.retain(|client, _| {
        states
            .get(client)
            .and_then(|owner| clients.get(&owner.socket))
            .is_none_or(|owner| owner.user == user)
    });

    // After that drop, as a joiner echoes everything the room holds. The browser parses each state
    // and stops at the first bad one, which would hide every cursor after it, for everybody.
    let refused = |entry: &AwarenessUpdateEntry| {
        entry.json.len() > MAX_AWARENESS_STATE_BYTES || !valid_state(&entry.json)
    };
    if update.clients.values().any(refused) {
        return Err(Close(CLOSE_PROTOCOL_ERROR));
    }

    let mut held = states.len();
    let mut bytes = state.state_bytes;
    update.clients.retain(|client, entry| {
        if entry.json.as_ref() == NULL_STATE {
            return true;
        }
        let current = states.get(client).map(|owner| owner.bytes);
        let held_after = held + usize::from(current.is_none());
        let bytes_after = bytes - current.unwrap_or_default() + entry.json.len();
        let fits = held_after <= MAX_ROOM_STATES && bytes_after <= MAX_ROOM_STATE_BYTES;
        if fits {
            held = held_after;
            bytes = bytes_after;
        }
        fits
    });

    let introduced = update
        .clients
        .keys()
        .filter(|client| awareness.meta(**client).is_none())
        .count();
    state.known_ids += introduced;
    let Some(client) = state.clients.get_mut(&id) else {
        return Ok(());
    };
    client.announced += introduced;
    if client.announced > MAX_ANNOUNCED_IDS {
        return Err(Close(CLOSE_PROTOCOL_ERROR));
    }
    Ok(())
}

#[derive(Deserialize)]
struct CursorField {
    #[serde(default)]
    cursor: Option<Value>,
}

/// JSON, with a `cursor` that every other client can read: the cursor plugin throws on anything
/// else, for each client that renders it.
fn valid_state(json: &str) -> bool {
    // Only the cursor is built in memory. A repeated key is refused here, so it cannot hide one.
    if !json.trim_start().starts_with('{') {
        return serde_json::from_str::<IgnoredAny>(json).is_ok();
    }
    serde_json::from_str::<CursorField>(json).is_ok_and(|state| {
        state.cursor.is_none_or(|cursor| {
            ["anchor", "head"]
                .iter()
                .all(|end| cursor.get(end).is_some_and(valid_position))
        })
    })
}

/// A `Y.RelativePosition` as JSON. `Y.createAbsolutePositionFromRelativePosition` throws when it
/// points at nothing.
fn valid_position(position: &Value) -> bool {
    let present = |field| position.get(field).filter(|value| !value.is_null());
    let tname = present("tname");
    let (item, kind) = (present("item"), present("type"));
    (item.is_some() || kind.is_some() || tname.is_some())
        && item.is_none_or(valid_id)
        && kind.is_none_or(valid_id)
        && tname.is_none_or(Value::is_string)
        && present("assoc").is_none_or(Value::is_i64)
}

fn valid_id(id: &Value) -> bool {
    ["client", "clock"]
        .iter()
        .all(|field| id.get(field).is_some_and(Value::is_u64))
}

/// Whether these ids go past what `known` holds.
fn reaches_past(ids: &IdSet, known: &StateVector) -> bool {
    ids.iter().any(|(client, ranges)| {
        let clock = known.get(client);
        ranges.iter().any(|range| range.end > clock)
    })
}

fn deny_write(state: &mut RoomState, id: u64) {
    let Some(client) = state.clients.get_mut(&id) else {
        return;
    };
    if !mem::replace(&mut client.told_read_only, true) {
        state.reply(id, Message::Auth(Some(READ_ONLY_REASON.to_owned())));
    }
}
