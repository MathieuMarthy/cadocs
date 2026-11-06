use std::cmp::min;
use std::collections::HashMap;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cadocs_core::permissions::SocketMode;
use cadocs_store::{DocStore, StoreError, StoredUpdate};
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore, watch};
use tokio::time::{Instant, sleep, sleep_until};
use tracing::warn;

use crate::connection::Connection;
use crate::room::{Room, Transition, lock};
use crate::{CLOSE_GOING_AWAY, CollabConfig, CollabError};

const MAX_RETRY_DELAY: Duration = Duration::from_secs(30);
/// Further flush attempts of a room at shutdown, after which its edits are given up.
const SHUTDOWN_FLUSH_RETRIES: usize = 3;

enum Slot {
    Loading(watch::Receiver<Transition>),
    Live(Arc<Room>),
    Unloading(watch::Receiver<Transition>),
}

#[derive(Default)]
struct Rooms {
    slots: HashMap<String, Slot>,
    closing: bool,
}

struct Inner<S> {
    store: Arc<S>,
    config: CollabConfig,
    rooms: Mutex<Rooms>,
    sockets: Arc<Semaphore>,
    failing_rooms: AtomicUsize,
    unloaded: Notify,
}

enum Attempt {
    Joined(Connection),
    /// The room is loading: its failure is the caller's failure.
    WaitLoad(watch::Receiver<Transition>),
    /// The room is unloading. Whatever the outcome, the next attempt finds it live or absent.
    WaitUnload(watch::Receiver<Transition>),
    /// Another room is unloading to make space.
    WaitEviction(watch::Receiver<Transition>),
}

/// Maps a document id to its loaded room. Load and unload of a document go through one slot, so a
/// client can never reach a second copy of a document that is still being written.
pub struct DocRegistry<S> {
    inner: Arc<Inner<S>>,
}

impl<S> Clone for DocRegistry<S> {
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
        }
    }
}

impl<S: DocStore> DocRegistry<S> {
    pub fn new(store: Arc<S>, config: CollabConfig) -> Self {
        let sockets = Arc::new(Semaphore::new(config.max_sockets));
        Self {
            inner: Arc::new(Inner {
                store,
                config,
                rooms: Mutex::default(),
                sockets,
                failing_rooms: AtomicUsize::new(0),
                unloaded: Notify::new(),
            }),
        }
    }

    /// Joins a document's room, loading it if needed. Cancelling this future is safe: a load in
    /// progress runs in its own task, and the room unloads if nobody joins it.
    pub async fn connect(&self, doc_id: &str, mode: SocketMode) -> Result<Connection, CollabError> {
        let mut permit = Some(
            self.inner
                .sockets
                .clone()
                .try_acquire_owned()
                .map_err(|_| CollabError::AtCapacity)?,
        );
        loop {
            match self.inner.attempt(doc_id, mode, &mut permit)? {
                Attempt::Joined(connection) => return Ok(connection),
                Attempt::WaitLoad(transition) => outcome(transition).await?,
                Attempt::WaitUnload(transition) => {
                    if let Err(CollabError::ShuttingDown) = outcome(transition).await {
                        return Err(CollabError::ShuttingDown);
                    }
                }
                Attempt::WaitEviction(transition) => outcome(transition)
                    .await
                    .map_err(|_| CollabError::AtCapacity)?,
            }
        }
    }

    /// Largest frame a socket may receive, for the socket layer's own limit.
    pub fn max_frame_bytes(&self) -> usize {
        self.inner.config.max_frame_bytes
    }

    /// Rooms in memory, including those loading or unloading.
    pub fn loaded_rooms(&self) -> usize {
        lock(&self.inner.rooms).slots.len()
    }

    /// Resolves once a room has unloaded since the last call. Several unloads may count as one.
    pub async fn room_unloaded(&self) {
        self.inner.unloaded.notified().await;
    }

    /// False while any room fails to flush, which `/health` reports.
    pub fn healthy(&self) -> bool {
        self.inner.failing_rooms.load(Ordering::SeqCst) == 0
    }

    /// Refuses new connections, closes every socket with `1001`, and flushes and unloads every
    /// room. Returns the last failure, if any: those edits only remain in connected clients.
    pub async fn shutdown(&self) -> Result<(), CollabError> {
        loop {
            // A room that failed in the last round may be back and flushed in this one.
            let mut result = Ok(());
            let transitions: Vec<_> = {
                let mut rooms = lock(&self.inner.rooms);
                rooms.closing = true;
                let live: Vec<_> = rooms
                    .slots
                    .values()
                    .filter_map(|slot| match slot {
                        Slot::Live(room) => Some(room.clone()),
                        _ => None,
                    })
                    .collect();
                for room in live {
                    room.state().kick_all(CLOSE_GOING_AWAY);
                    begin_unload(&mut rooms, &room);
                }
                rooms
                    .slots
                    .values()
                    .filter_map(|slot| match slot {
                        Slot::Loading(rx) => Some((rx.clone(), false)),
                        Slot::Unloading(rx) => Some((rx.clone(), true)),
                        Slot::Live(_) => None,
                    })
                    .collect()
            };
            let done = transitions.is_empty();
            for (transition, holds_edits) in transitions {
                if let Err(error) = outcome(transition).await
                    && holds_edits
                {
                    result = Err(error);
                }
            }
            if done || result.is_err() {
                return result;
            }
        }
    }
}

async fn outcome(mut transition: watch::Receiver<Transition>) -> Result<(), CollabError> {
    match transition.wait_for(Option::is_some).await {
        Ok(value) => value.clone().unwrap_or(Ok(())),
        Err(_) => Err(CollabError::ShuttingDown),
    }
}

/// Marks a live room as unloading and asks its task to do it. No client can join from here on.
fn begin_unload(rooms: &mut Rooms, room: &Arc<Room>) -> watch::Receiver<Transition> {
    let (done, transition) = watch::channel(None);
    room.state().unload = Some(done);
    room.wake.notify_one();
    rooms
        .slots
        .insert(room.doc_id.clone(), Slot::Unloading(transition.clone()));
    transition
}

impl<S: DocStore> Inner<S> {
    fn attempt(
        self: &Arc<Self>,
        doc_id: &str,
        mode: SocketMode,
        permit: &mut Option<OwnedSemaphorePermit>,
    ) -> Result<Attempt, CollabError> {
        let mut rooms = lock(&self.rooms);
        if rooms.closing {
            return Err(CollabError::ShuttingDown);
        }
        match rooms.slots.get(doc_id) {
            Some(Slot::Live(room)) => {
                let permit = permit.take().expect("a connection joins only once");
                return Ok(Attempt::Joined(room.join(mode, permit)));
            }
            Some(Slot::Loading(transition)) => return Ok(Attempt::WaitLoad(transition.clone())),
            Some(Slot::Unloading(transition)) => {
                return Ok(Attempt::WaitUnload(transition.clone()));
            }
            None => {}
        }

        if rooms.slots.len() >= self.config.max_rooms {
            if let Some(victim) = oldest_idle(&rooms) {
                return Ok(Attempt::WaitEviction(begin_unload(&mut rooms, &victim)));
            }
            let unloading = rooms.slots.values().find_map(|slot| match slot {
                Slot::Unloading(transition) => Some(transition.clone()),
                _ => None,
            });
            return match unloading {
                Some(transition) => Ok(Attempt::WaitEviction(transition)),
                None => Err(CollabError::AtCapacity),
            };
        }

        let (done, transition) = watch::channel(None);
        rooms
            .slots
            .insert(doc_id.to_owned(), Slot::Loading(transition.clone()));
        tokio::spawn(self.clone().load(doc_id.to_owned(), done));
        Ok(Attempt::WaitLoad(transition))
    }

    async fn load(self: Arc<Self>, doc_id: String, done: watch::Sender<Transition>) {
        let room = match self.store.load(&doc_id).await {
            // Dropping the load's transaction commits it, outside the guards of `build`. A panic
            // here must not leave the slot loading forever.
            Ok(stored) => catch_unwind(AssertUnwindSafe(|| {
                Room::build(doc_id.clone(), stored, self.config.clone())
            }))
            .unwrap_or_else(|_| {
                Err(CollabError::CorruptDocument {
                    doc_id: doc_id.clone(),
                })
            }),
            Err(error) => Err(error.into()),
        };
        let mut rooms = lock(&self.rooms);
        let result = match room {
            Ok(room) => {
                rooms.slots.insert(doc_id, Slot::Live(room.clone()));
                tokio::spawn(RoomTask::new(self.clone(), room).run());
                Ok(())
            }
            Err(error) => {
                rooms.slots.remove(&doc_id);
                Err(error)
            }
        };
        drop(rooms);
        done.send_replace(Some(result));
    }

    /// Starts an unload if the room is still empty past its delay.
    fn unload_if_idle(&self, room: &Arc<Room>) {
        let mut rooms = lock(&self.rooms);
        let is_slot = matches!(
            rooms.slots.get(&room.doc_id),
            Some(Slot::Live(live)) if Arc::ptr_eq(live, room)
        );
        let expired = {
            let state = room.state();
            !state.has_clients()
                && state
                    .idle_since
                    .is_some_and(|since| since + self.config.room_idle <= Instant::now())
        };
        if is_slot && expired {
            begin_unload(&mut rooms, room);
        }
    }
}

/// The room to unload at the cap. A failing room is skipped: it could not unload anyway.
fn oldest_idle(rooms: &Rooms) -> Option<Arc<Room>> {
    rooms
        .slots
        .values()
        .filter_map(|slot| {
            let Slot::Live(room) = slot else { return None };
            let state = room.state();
            let since = state.idle_since.filter(|_| !state.failing)?;
            Some((since, room.clone()))
        })
        .min_by_key(|(since, _)| *since)
        .map(|(_, room)| room)
}

/// The one task that writes a room to the store, so flushes and compactions never overlap.
struct RoomTask<S> {
    inner: Arc<Inner<S>>,
    room: Arc<Room>,
    /// Updates taken from the queue and not stored yet. A retry resends them with the same `seq`,
    /// which the store accepts after a lost commit.
    batch: Vec<StoredUpdate>,
    retry_delay: Duration,
    /// Set after a failed compaction, which encodes the whole document under the room lock.
    next_compact: Option<Instant>,
}

impl<S: DocStore> RoomTask<S> {
    fn new(inner: Arc<Inner<S>>, room: Arc<Room>) -> Self {
        let retry_delay = inner.config.flush_interval;
        Self {
            inner,
            room,
            batch: Vec::new(),
            retry_delay,
            next_compact: None,
        }
    }

    async fn run(mut self) {
        let mut next_flush: Option<Instant> = None;
        loop {
            let (unloading, queued, idle_at) = {
                let state = self.room.state();
                let idle_at = state
                    .idle_since
                    .filter(|_| !state.failing)
                    .map(|since| since + self.inner.config.room_idle);
                (state.unload.is_some(), !state.queue.is_empty(), idle_at)
            };
            if unloading {
                if self.unload().await {
                    return;
                }
                next_flush = None;
                continue;
            }
            if queued || !self.batch.is_empty() {
                next_flush.get_or_insert_with(|| Instant::now() + self.retry_delay);
            }

            let wake_at = [next_flush, idle_at].into_iter().flatten().min();
            tokio::select! {
                () = self.room.wake.notified() => {}
                () = sleep_until(wake_at.unwrap_or_else(Instant::now)), if wake_at.is_some() => {}
            }

            let now = Instant::now();
            if next_flush.is_some_and(|at| at <= now) {
                next_flush = None;
                if self.flush().await.is_ok() {
                    self.compact_past_threshold().await;
                }
            }
            if idle_at.is_some_and(|at| at <= now) {
                self.inner.unload_if_idle(&self.room);
            }
        }
    }

    async fn flush(&mut self) -> Result<(), StoreError> {
        self.batch.append(&mut self.room.take_queue());
        let Some(last) = self.batch.last() else {
            return Ok(());
        };
        let last_seq = last.seq;
        match self
            .inner
            .store
            .append_updates(&self.room.doc_id, &self.batch)
            .await
        {
            Ok(()) => {
                let batch = mem::take(&mut self.batch);
                let bytes: usize = batch.iter().map(|update| update.data.len()).sum();
                let mut state = self.room.state();
                state.unflushed_bytes -= bytes;
                state.flushed_seq = last_seq;
                state.stored_updates += batch.len();
                if mem::replace(&mut state.failing, false) {
                    self.inner.failing_rooms.fetch_sub(1, Ordering::SeqCst);
                }
                self.retry_delay = self.inner.config.flush_interval;
                Ok(())
            }
            Err(error) => {
                warn!(doc_id = %self.room.doc_id, %error, "flush failed");
                let mut state = self.room.state();
                if !mem::replace(&mut state.failing, true) {
                    self.inner.failing_rooms.fetch_add(1, Ordering::SeqCst);
                }
                let ceiling = MAX_RETRY_DELAY.max(self.inner.config.flush_interval);
                self.retry_delay = min(self.retry_delay * 2, ceiling);
                Err(error)
            }
        }
    }

    async fn compact_past_threshold(&mut self) {
        let due = self.next_compact.is_none_or(|at| at <= Instant::now());
        if due && self.room.state().stored_updates > self.inner.config.compact_updates {
            self.compact().await;
        }
    }

    /// A failure only delays compaction: the log still holds everything.
    async fn compact(&mut self) {
        let Some(snapshot) = self.room.snapshot() else {
            return;
        };
        match self.inner.store.compact(&self.room.doc_id, &snapshot).await {
            Ok(()) => {
                self.room.state().stored_updates = 0;
                self.next_compact = None;
            }
            Err(error) => {
                warn!(doc_id = %self.room.doc_id, %error, "compaction failed");
                self.next_compact = Some(Instant::now() + MAX_RETRY_DELAY);
            }
        }
    }

    /// Returns whether the room is gone. A flush failure keeps it, because dropping it would drop
    /// the edits, unless the process is exiting anyway.
    async fn unload(&mut self) -> bool {
        let mut flushed = self.flush().await;
        // Read once: a shutdown starting after this finds the room live again and unloads it.
        let closing = lock(&self.inner.rooms).closing;
        if closing {
            for _ in 0..SHUTDOWN_FLUSH_RETRIES {
                if flushed.is_ok() {
                    break;
                }
                sleep(self.retry_delay).await;
                flushed = self.flush().await;
            }
        }
        if flushed.is_ok() {
            self.compact().await;
        }

        let mut rooms = lock(&self.inner.rooms);
        let gone = flushed.is_ok() || closing;
        if gone {
            rooms.slots.remove(&self.room.doc_id);
        } else {
            rooms
                .slots
                .insert(self.room.doc_id.clone(), Slot::Live(self.room.clone()));
        }
        let done = self.room.state().unload.take();
        drop(rooms);
        if let Some(done) = done {
            done.send_replace(Some(flushed.map_err(CollabError::from)));
        }
        if gone {
            self.inner.unloaded.notify_one();
        }
        gone
    }
}
