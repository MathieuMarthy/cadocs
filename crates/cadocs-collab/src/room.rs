use std::collections::{HashMap, HashSet};
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use cadocs_core::permissions::SocketMode;
use cadocs_store::{Snapshot, StoredDoc, StoredUpdate};
use serde::de::IgnoredAny;
use tokio::sync::{Notify, OwnedSemaphorePermit, mpsc, oneshot, watch};
use tokio::time::Instant;
use tracing::warn;
use yrs::sync::awareness::AwarenessUpdateEntry;
use yrs::sync::{Awareness, AwarenessUpdate, Message, SyncMessage};
use yrs::updates::encoder::Encode;
use yrs::{ClientID, Doc, IdSet, ReadTxn, StateVector, Transact, Update};

use crate::connection::{Close, Connection, decode_untrusted};
use crate::{
    CLOSE_PROTOCOL_ERROR, CLOSE_STORAGE_FAILING, CLOSE_TOO_SLOW, CollabConfig, CollabError,
};

/// `None` until a load or an unload ends.
pub(crate) type Transition = Option<Result<(), CollabError>>;

const READ_ONLY_REASON: &str = "read-only";
/// Client ids one socket may ever introduce. A tab uses one, and the server keeps a clock for each
/// id until the room unloads, so this bounds what a socket can leave behind.
const MAX_ANNOUNCED_IDS: usize = 16;
/// Largest awareness state: a name, a colour and a cursor take well under 1 KiB.
const MAX_AWARENESS_STATE_BYTES: usize = 64 * 1024;
const NULL_STATE: &str = "null";

/// Poisoning is ignored: the critical sections keep no invariant a panic could break that matters
/// more than keeping the room's unflushed edits reachable.
pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

struct Client {
    mode: SocketMode,
    frames: mpsc::Sender<Arc<[u8]>>,
    /// Bytes waiting in `frames`. A frame count alone would let a client that requests the whole
    /// document and never reads pin `send_queue` copies of it.
    queued_bytes: Arc<AtomicUsize>,
    max_queued_bytes: usize,
    /// `None` once the client was told to close.
    close: Option<oneshot::Sender<u16>>,
    /// Awareness ids this socket owns, removed when it closes.
    owned: HashSet<ClientID>,
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

/// A loaded document. All of this is reached under one short lock, never held across an await.
pub(crate) struct RoomState {
    awareness: Awareness,
    /// Filled by the document observer: the part of an update that really changed the document.
    applied: Arc<Mutex<Option<Vec<u8>>>>,
    clients: HashMap<u64, Client>,
    next_client: u64,
    /// Updates the room task has not picked up yet.
    pub(crate) queue: Vec<StoredUpdate>,
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

    pub(crate) fn has_clients(&self) -> bool {
        !self.clients.is_empty()
    }

    pub(crate) fn kick_all(&mut self, code: u16) {
        for client in self.clients.values_mut() {
            client.kick(code);
        }
    }
}

pub(crate) struct Room {
    pub(crate) doc_id: String,
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
        let corrupt = || CollabError::CorruptDocument {
            doc_id: doc_id.clone(),
        };
        let doc = Doc::new();
        let last_seq = stored.last_seq();
        let stored_updates = stored.updates.len();
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

        let applied = Arc::new(Mutex::new(None));
        let sink = applied.clone();
        doc.observe_update_v1("room", move |_, event| {
            *lock(&sink) = Some(event.update.clone());
        })
        .expect("a new document has no open transaction");

        Ok(Arc::new(Self {
            doc_id,
            state: Mutex::new(RoomState {
                awareness: Awareness::new(doc),
                applied,
                clients: HashMap::new(),
                next_client: 0,
                queue: Vec::new(),
                unflushed_bytes: 0,
                next_seq: last_seq + 1,
                flushed_seq: last_seq,
                stored_updates,
                idle_since: Some(Instant::now()),
                failing: false,
                skipped_row,
                unload: None,
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
                frames,
                queued_bytes: queued_bytes.clone(),
                max_queued_bytes: self.config.max_frame_bytes,
                close: Some(close),
                owned: HashSet::new(),
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
                    // Only real changes change ownership: `y-websocket` echoes the states of
                    // other clients back, with a clock the server already knows.
                    for changed in summary.added.iter().chain(&summary.updated) {
                        for client in state.clients.values_mut() {
                            client.owned.remove(changed);
                        }
                    }
                    for removed in &summary.removed {
                        for client in state.clients.values_mut() {
                            client.owned.remove(removed);
                        }
                    }
                    if let Some(client) = state.clients.get_mut(&id) {
                        client.owned.extend(&summary.added);
                        client.owned.extend(&summary.updated);
                    }
                    let update = state
                        .awareness
                        .update_with_clients(summary.all_changes())
                        .map_err(|_| Close(CLOSE_PROTOCOL_ERROR))?;
                    state.broadcast(id, &Message::Awareness(update));
                }
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
        // Both can exist: `applied` may hold older updates that this one released, which the frame
        // does not contain.
        for data in applied.into_iter().chain(raw) {
            let seq = state.next_seq;
            state.next_seq += 1;
            state.unflushed_bytes += data.len();
            if state.queue.is_empty() {
                self.wake.notify_one();
            }
            state.queue.push(StoredUpdate { seq, data });
        }
        valid.then_some(()).ok_or(Close(CLOSE_PROTOCOL_ERROR))
    }

    /// Removes a client and the awareness states it owned, so no ghost cursor stays.
    pub(crate) fn leave(&self, id: u64) {
        let mut state = self.state();
        let Some(client) = state.clients.remove(&id) else {
            return;
        };
        // A null state at the same clock removes the entry everywhere, as `y-websocket`'s server
        // does. `remove_state` would bump the clock, and the client's next change after a
        // reconnect would be ignored as old.
        if !client.owned.is_empty()
            && let Ok(mut update) = state.awareness.update_with_clients(client.owned)
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
    pub(crate) fn snapshot(&self) -> Option<Snapshot> {
        let state = self.state();
        if state.stored_updates == 0 || state.skipped_row {
            return None;
        }
        // yrs merges what is waiting into the encoded state, and that merge lost a delete in
        // `real_updates_in_any_order_survive_a_reload`. The log stays correct, so it waits.
        if state.awareness.doc().transact().has_missing_updates() {
            return None;
        }
        // The panic guard matters here: this runs in the room task, which must survive to flush.
        let encoded = catch_unwind(AssertUnwindSafe(|| state.encode_full()));
        match encoded {
            Ok(encoded) => Some(Snapshot {
                state: encoded,
                upto_seq: state.flushed_seq,
            }),
            Err(_) => {
                warn!(doc_id = %self.doc_id, "cannot encode the document, keeping its log");
                None
            }
        }
    }

    /// Takes the queue for a flush.
    pub(crate) fn take_queue(&self) -> Vec<StoredUpdate> {
        mem::take(&mut self.state().queue)
    }
}

/// Applies the limits on an awareness update from a socket. Removing an id the room does not know
/// changes nothing, so it is dropped here instead of leaving a clock behind.
fn admit_awareness(
    state: &mut RoomState,
    id: u64,
    update: &mut AwarenessUpdate,
) -> Result<(), Close> {
    let awareness = &state.awareness;
    update.clients.retain(|client, entry| {
        entry.json.as_ref() != NULL_STATE || awareness.meta(*client).is_some()
    });
    // The browser parses each state and stops at the first bad one, which would hide every cursor
    // after it, for everybody.
    let refused = |entry: &AwarenessUpdateEntry| {
        entry.json.len() > MAX_AWARENESS_STATE_BYTES
            || serde_json::from_str::<IgnoredAny>(&entry.json).is_err()
    };
    if update.clients.values().any(refused) {
        return Err(Close(CLOSE_PROTOCOL_ERROR));
    }
    let introduced = update
        .clients
        .keys()
        .filter(|client| awareness.meta(**client).is_none())
        .count();
    let Some(client) = state.clients.get_mut(&id) else {
        return Ok(());
    };
    client.announced += introduced;
    if client.announced > MAX_ANNOUNCED_IDS {
        return Err(Close(CLOSE_PROTOCOL_ERROR));
    }
    Ok(())
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
