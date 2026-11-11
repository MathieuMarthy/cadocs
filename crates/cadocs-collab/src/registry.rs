use std::cmp::min;
use std::collections::HashMap;
use std::mem;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cadocs_core::permissions::{SocketMode, SocketOutcome, SpaceRole};
use cadocs_store::{DocEdit, DocStore, StoreError, StoredDoc, StoredUpdate};
use tokio::sync::{Notify, Semaphore, watch};
use tokio::time::{Instant, sleep, sleep_until};
use tracing::warn;
use yrs::{Doc, TransactionMut};

use crate::connection::Connection;
use crate::events::{Event, EventConnection, EventHubs, PageEvent};
use crate::room::{Room, Transition, lock, rebuild};
use crate::{Authorizer, CLOSE_GOING_AWAY, CollabConfig, CollabError, Seat};

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
    /// Page and events sockets alike.
    sockets: Arc<Semaphore>,
    /// Documents built outside a room, and what they are built from.
    reads: Semaphore,
    events: Arc<EventHubs>,
    failing_rooms: AtomicUsize,
    unloaded: Notify,
}

enum Attempt<T> {
    /// The room was live, and this is what the caller did with it.
    Entered(T),
    /// The room is loading: its failure is the caller's failure.
    WaitLoad(watch::Receiver<Transition>),
    /// The room is unloading. Whatever the outcome, the next attempt finds it live or absent.
    WaitUnload(watch::Receiver<Transition>),
    /// Another room is unloading to make space.
    WaitEviction(watch::Receiver<Transition>),
    /// Another room is loading, and rooms load one at a time. Whatever its outcome, try again.
    WaitTurn(watch::Receiver<Transition>),
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

impl<S> DocRegistry<S> {
    /// Tells the sockets of a workspace, for those whose user reads the space of `event`.
    pub fn publish(&self, workspace_id: &str, event: Event) {
        self.inner.events.publish(workspace_id, event);
    }

    /// Publishes `event` after the next recheck, for a space whose readers only that recheck can
    /// tell. A recheck must follow, through `AppState::access_changed` in the server.
    pub fn publish_after_recheck(&self, workspace_id: &str, event: Event) {
        self.inner.events.hold(workspace_id, event);
    }

    /// Workspaces with an events socket.
    pub fn event_hubs(&self) -> usize {
        self.inner.events.count()
    }
}

impl<S: DocStore> DocRegistry<S> {
    pub fn new(store: Arc<S>, config: CollabConfig) -> Self {
        let sockets = Arc::new(Semaphore::new(config.max_sockets));
        let events = Arc::new(EventHubs::new(config.send_queue));
        let reads = Semaphore::new(config.doc_reads);
        Self {
            inner: Arc::new(Inner {
                store,
                config,
                rooms: Mutex::default(),
                sockets,
                reads,
                events,
                failing_rooms: AtomicUsize::new(0),
                unloaded: Notify::new(),
            }),
        }
    }

    /// Joins a document's room, loading it if needed. Cancelling this future is safe: a load in
    /// progress runs in its own task, and the room unloads if nobody joins it.
    pub async fn connect(
        &self,
        doc_id: &str,
        mode: SocketMode,
        user_id: &str,
        session: &str,
    ) -> Result<Connection, CollabError> {
        let mut permit = Some(
            self.inner
                .sockets
                .clone()
                .try_acquire_owned()
                .map_err(|_| CollabError::AtCapacity)?,
        );
        self.enter(doc_id, |room| {
            let permit = permit.take().expect("a connection joins only once");
            room.join(mode, user_id, session, permit)
        })
        .await
    }

    /// Opens an events socket on a workspace, for a user with a role on `spaces`, ordered by id.
    pub fn join_events(
        &self,
        workspace_id: &str,
        user_id: &str,
        session: &str,
        spaces: Vec<(String, SpaceRole)>,
    ) -> Result<EventConnection, CollabError> {
        let sockets = self.inner.sockets.clone();
        let permit = sockets
            .try_acquire_owned()
            .map_err(|_| CollabError::AtCapacity)?;
        let hubs = &self.inner.events;
        hubs.join(workspace_id, user_id, session, spaces, permit)
    }

    /// Waits until a document's room is live, loading it if needed. A check that decides a write
    /// comes after this, so the wait for a load does not sit between the two.
    pub async fn open(&self, doc_id: &str) -> Result<(), CollabError> {
        self.enter(doc_id, |_| ()).await
    }

    /// Replaces a document's title through its room, loading it if needed: an edit like any other,
    /// so a client cannot miss it and a flush cannot undo it. An empty room unloads as usual.
    pub async fn set_title(
        &self,
        doc_id: &str,
        title: &str,
        user_id: &str,
    ) -> Result<(), CollabError> {
        self.enter(doc_id, |room| room.set_title(title, user_id))
            .await?
    }

    /// Runs `action` on a document's room, loading it if needed, as one edit by `user_id` that
    /// every client receives. `bytes` is about what it writes, for the bound on unflushed edits.
    /// This is for writes that replace content, such as a revision restore.
    pub async fn rewrite<T>(
        &self,
        doc_id: &str,
        user_id: &str,
        bytes: usize,
        action: impl FnOnce(&mut TransactionMut) -> T,
    ) -> Result<T, CollabError> {
        let mut action = Some(action);
        self.enter(doc_id, |room| {
            let action = action.take().expect("a room is entered once");
            room.change(user_id, bytes, action)
        })
        .await?
    }

    /// Hands `action` a private copy of a document, with no client needed. A loaded room gives its
    /// edits, flushed or not, and only for as long as it takes to encode them, so a slow `action`
    /// holds up no room. Otherwise the copy comes from the store: no room is made, nothing is
    /// written, and `CADOCS_MAX_ROOMS` does not apply. A read waits its turn past `doc_reads`.
    pub async fn read<T>(
        &self,
        doc_id: &str,
        action: impl FnOnce(&Doc) -> T,
    ) -> Result<T, CollabError> {
        let _turn = self
            .inner
            .reads
            .acquire()
            .await
            .expect("this semaphore is never closed");
        let stored = self.latest(doc_id).await?;
        // The guard is for the commit of the transaction, as in `Inner::load`.
        let built = catch_unwind(AssertUnwindSafe(|| rebuild(doc_id, &stored)));
        drop(stored);
        let corrupt = || CollabError::CorruptDocument {
            doc_id: doc_id.to_owned(),
        };
        let (doc, _) = built.map_err(|_| corrupt())??;
        Ok(action(&doc))
    }

    /// Runs `action` under a turn of `doc_reads`, for work outside a room that weighs as much as
    /// a read, such as decoding a stored revision. It never fails today; the `Result` matches
    /// `read`, so a caller can treat both alike.
    pub async fn bounded<T>(&self, action: impl FnOnce() -> T) -> Result<T, CollabError> {
        let _turn = self
            .inner
            .reads
            .acquire()
            .await
            .expect("this semaphore is never closed");
        Ok(action())
    }

    /// The latest state of a document. An unload still holds edits the store lacks, so it is waited
    /// for. A load is not: the store has all it will read.
    async fn latest(&self, doc_id: &str) -> Result<StoredDoc, CollabError> {
        loop {
            let unloading = {
                let rooms = lock(&self.inner.rooms);
                if rooms.closing {
                    return Err(CollabError::ShuttingDown);
                }
                match rooms.slots.get(doc_id) {
                    Some(Slot::Live(room)) => Ok(room.clone()),
                    Some(Slot::Unloading(transition)) => Err(Some(transition.clone())),
                    Some(Slot::Loading(_)) | None => Err(None),
                }
            };
            match unloading {
                Ok(room) => return room.read(),
                Err(None) => return Ok(self.inner.store.load(doc_id).await?),
                // A failed unload leaves the room live, and the next turn reads it.
                Err(Some(transition)) => {
                    if let Err(CollabError::ShuttingDown) = outcome(transition).await {
                        return Err(CollabError::ShuttingDown);
                    }
                }
            }
        }
    }

    /// Runs `action` once on the live room of a document, under the lock that keeps it live.
    async fn enter<T>(
        &self,
        doc_id: &str,
        mut action: impl FnMut(&Arc<Room>) -> T,
    ) -> Result<T, CollabError> {
        loop {
            match self.inner.attempt(doc_id, &mut action)? {
                Attempt::Entered(result) => return Ok(result),
                Attempt::WaitLoad(transition) => outcome(transition).await?,
                Attempt::WaitUnload(transition) | Attempt::WaitTurn(transition) => {
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

    /// Asks `authorizer` about every open socket, and closes those it refuses. A socket that cannot
    /// be checked stays open, and the last failure is returned, for the next pass to fix.
    pub async fn recheck(&self, authorizer: &impl Authorizer) -> Result<(), StoreError> {
        // Taken first: what is held was stored before this pass, so the pass sees its readers.
        let held = self.inner.events.take_held();
        let mut result = self.recheck_page_sockets(authorizer).await;
        for seat in self.inner.events.open_seats() {
            match authorizer.recheck_events(&seat.view()).await {
                Ok(standing) => self.inner.events.settle(&seat, standing),
                Err(error) => {
                    warn!(%error, "cannot recheck an events socket");
                    result = Err(error);
                }
            }
        }
        for (workspace_id, event) in held {
            self.inner.events.publish(&workspace_id, event);
        }
        result
    }

    async fn recheck_page_sockets(&self, authorizer: &impl Authorizer) -> Result<(), StoreError> {
        let rooms: Vec<_> = lock(&self.inner.rooms)
            .slots
            .values()
            .filter_map(|slot| match slot {
                Slot::Live(room) => Some(room.clone()),
                _ => None,
            })
            .collect();
        let mut result = Ok(());
        for room in rooms {
            for socket in room.sockets() {
                let seat = Seat {
                    page_id: &room.doc_id,
                    user_id: &socket.user,
                    session: &socket.session,
                    mode: socket.mode,
                };
                match authorizer.recheck(&seat).await {
                    Ok(SocketOutcome::Keep) => {}
                    Ok(SocketOutcome::Close(code)) => room.kick(socket.id, code),
                    Err(error) => {
                        warn!(doc_id = %room.doc_id, %error, "cannot recheck a socket");
                        result = Err(error);
                    }
                }
            }
        }
        result
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
        self.inner.events.close_all();
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
    fn attempt<T>(
        self: &Arc<Self>,
        doc_id: &str,
        action: &mut impl FnMut(&Arc<Room>) -> T,
    ) -> Result<Attempt<T>, CollabError> {
        let mut rooms = lock(&self.rooms);
        if rooms.closing {
            return Err(CollabError::ShuttingDown);
        }
        match rooms.slots.get(doc_id) {
            Some(Slot::Live(room)) => {
                // Whoever loaded it is here: from now on a cap may evict it once it is idle.
                room.state().joined = true;
                return Ok(Attempt::Entered(action(room)));
            }
            Some(Slot::Loading(transition)) => return Ok(Attempt::WaitLoad(transition.clone())),
            Some(Slot::Unloading(transition)) => {
                return Ok(Attempt::WaitUnload(transition.clone()));
            }
            None => {}
        }

        if rooms.slots.len() >= self.config.max_rooms
            || estimated_bytes(&rooms) >= self.config.room_memory
        {
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

        // A loading room's size is unknown until it is built, so rooms load one at a time: the
        // budget is crossed by one room at most, even by a burst of reconnections after a restart.
        // A load is a read and a rebuild, milliseconds for a page of a few hundred KB.
        let loading = rooms.slots.values().find_map(|slot| match slot {
            Slot::Loading(transition) => Some(transition.clone()),
            _ => None,
        });
        if let Some(transition) = loading {
            return Ok(Attempt::WaitTurn(transition));
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

/// What the live rooms are counted as together. A room loading or unloading counts for nothing,
/// so a load can start while the room it evicted is still on its way out: one more transient room.
fn estimated_bytes(rooms: &Rooms) -> usize {
    rooms
        .slots
        .values()
        .filter_map(|slot| match slot {
            Slot::Live(room) => Some(room.estimate.load(Ordering::Relaxed)),
            _ => None,
        })
        .sum()
}

/// The room to unload at the cap. A failing room is skipped: it could not unload anyway. So is one
/// no client has joined yet: the client that loaded it is on its way, and evicting it would make
/// two connections take turns loading and evicting. Its idle delay unloads it if nobody comes.
fn oldest_idle(rooms: &Rooms) -> Option<Arc<Room>> {
    rooms
        .slots
        .values()
        .filter_map(|slot| {
            let Slot::Live(room) = slot else { return None };
            let state = room.state();
            let since = state.idle_since.filter(|_| !state.failing && state.joined)?;
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
    /// What the row of the page gets with `batch`.
    edit: DocEdit,
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
            edit: DocEdit::default(),
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
        let (mut queue, edit) = self.room.take_queue();
        self.batch.append(&mut queue);
        self.edit.title = edit.title.or(self.edit.title.take());
        self.edit.author = edit.author.or(self.edit.author.take());
        let Some(last) = self.batch.last() else {
            return Ok(());
        };
        let last_seq = last.seq;
        match self
            .inner
            .store
            .append_updates(&self.room.doc_id, &self.batch, &self.edit)
            .await
        {
            Ok(renamed) => {
                if let Some(site) = renamed {
                    let event = Event::page(PageEvent::Renamed, &site.space_id, &self.room.doc_id);
                    self.inner.events.publish(&site.workspace_id, event);
                }
                let batch = mem::take(&mut self.batch);
                self.edit = DocEdit::default();
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
            // Retrying could not help, and no client may go on editing a page that is gone.
            Err(StoreError::PageNotFound) => {
                warn!(doc_id = %self.room.doc_id, "the page of this room is gone");
                self.batch.clear();
                self.edit = DocEdit::default();
                self.room.abandon();
                let mut state = self.room.state();
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
