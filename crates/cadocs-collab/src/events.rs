//! Workspace hubs behind events sockets (ADR 0011). Events are hints with ids only, so a client
//! that misses some loses nothing but freshness, and nothing here is ever stored.

use std::collections::HashMap;
use std::mem;
use std::sync::{Arc, Mutex};

use cadocs_core::permissions::SpaceRole;
use serde_json::json;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{OwnedSemaphorePermit, broadcast, watch};

use crate::room::lock;
use crate::{CLOSE_GOING_AWAY, CollabError, EventSeat, EventStanding};

const ACCESS_CHANGED: &str = r#"{"type":"access.changed"}"#;
const RESYNC: &str = r#"{"type":"resync"}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageEvent {
    Created,
    Moved,
    Renamed,
    Trashed,
    Restored,
    Purged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpaceEvent {
    Created,
    Updated,
    Trashed,
    Restored,
}

impl PageEvent {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Created => "page.created",
            Self::Moved => "page.moved",
            Self::Renamed => "page.renamed",
            Self::Trashed => "page.trashed",
            Self::Restored => "page.restored",
            Self::Purged => "page.purged",
        }
    }
}

impl SpaceEvent {
    fn wire_name(self) -> &'static str {
        match self {
            Self::Created => "space.created",
            Self::Updated => "space.updated",
            Self::Trashed => "space.trashed",
            Self::Restored => "space.restored",
        }
    }
}

/// A change in one space. Its frame is built once, however many sockets send it.
#[derive(Debug)]
pub struct Event {
    space: String,
    frame: String,
}

impl Event {
    pub fn page(kind: PageEvent, space: &str, page: &str) -> Self {
        let frame = json!({ "type": kind.wire_name(), "space": space, "page": page });
        Self {
            space: space.to_owned(),
            frame: frame.to_string(),
        }
    }

    pub fn space(kind: SpaceEvent, space: &str) -> Self {
        let frame = json!({ "type": kind.wire_name(), "space": space });
        Self {
            space: space.to_owned(),
            frame: frame.to_string(),
        }
    }
}

/// What the socket task must do next.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventOutbound {
    Text(String),
    Close(u16),
}

/// What a recheck decided about a socket, read by its task.
struct Standing {
    /// Ordered by id, as `WorkspaceReach` gives them.
    spaces: Vec<(String, SpaceRole)>,
    /// How many times `spaces` changed.
    changes: u64,
    close: Option<u16>,
}

/// The most events a hub keeps, whatever `CADOCS_SEND_QUEUE` says: a broadcast channel allocates
/// all its slots at once, for each workspace with a socket.
const MAX_HUB_EVENTS: usize = 4096;

struct Listener {
    user: String,
    session: String,
    standing: watch::Sender<Standing>,
}

impl Listener {
    fn close(&self, code: u16) {
        self.standing.send_if_modified(|standing| {
            let first = standing.close.is_none();
            standing.close = standing.close.or(Some(code));
            first
        });
    }

    fn settle(&self, new: EventStanding) {
        match new {
            EventStanding::Close(code) => self.close(code),
            EventStanding::Open(spaces) => {
                self.standing.send_if_modified(|standing| {
                    let changed = standing.spaces != spaces;
                    if changed {
                        standing.spaces = spaces;
                        standing.changes += 1;
                    }
                    changed
                });
            }
        }
    }
}

struct Hub {
    events: broadcast::Sender<Arc<Event>>,
    listeners: HashMap<u64, Listener>,
}

#[derive(Default)]
struct Hubs {
    by_workspace: HashMap<String, Hub>,
    next_id: u64,
    /// Waiting for a recheck to give the space its readers.
    held: Vec<(String, Event)>,
    closing: bool,
}

impl Hubs {
    fn listener(&self, workspace: &str, id: u64) -> Option<&Listener> {
        let hub = self.by_workspace.get(workspace)?;
        hub.listeners.get(&id)
    }
}

/// An open events socket, copied out of the hub for a recheck.
pub(crate) struct OpenSeat {
    workspace: String,
    id: u64,
    user: String,
    session: String,
}

impl OpenSeat {
    pub(crate) fn view(&self) -> EventSeat<'_> {
        EventSeat {
            workspace_id: &self.workspace,
            user_id: &self.user,
            session: &self.session,
        }
    }
}

pub(crate) struct EventHubs {
    hubs: Mutex<Hubs>,
    /// Events a socket may fall behind by before it gets `resync`.
    capacity: usize,
}

impl EventHubs {
    pub(crate) fn new(capacity: usize) -> Self {
        Self {
            hubs: Mutex::default(),
            capacity: capacity.min(MAX_HUB_EVENTS),
        }
    }

    pub(crate) fn join(
        self: &Arc<Self>,
        workspace: &str,
        user: &str,
        session: &str,
        spaces: Vec<(String, SpaceRole)>,
        permit: OwnedSemaphorePermit,
    ) -> Result<EventConnection, CollabError> {
        let mut hubs = lock(&self.hubs);
        if hubs.closing {
            return Err(CollabError::ShuttingDown);
        }
        let id = hubs.next_id;
        hubs.next_id += 1;
        let hub = hubs
            .by_workspace
            .entry(workspace.to_owned())
            .or_insert_with(|| Hub {
                events: broadcast::Sender::new(self.capacity),
                listeners: HashMap::new(),
            });
        let (standing, watching) = watch::channel(Standing {
            spaces,
            changes: 0,
            close: None,
        });
        let listener = Listener {
            user: user.to_owned(),
            session: session.to_owned(),
            standing,
        };
        hub.listeners.insert(id, listener);
        Ok(EventConnection {
            hubs: self.clone(),
            workspace: workspace.to_owned(),
            id,
            events: hub.events.subscribe(),
            standing: watching,
            told: 0,
            _permit: permit,
        })
    }

    pub(crate) fn publish(&self, workspace: &str, event: Event) {
        if let Some(hub) = lock(&self.hubs).by_workspace.get(workspace) {
            // Fails only with no receiver, and then nobody is missing anything.
            let _ = hub.events.send(Arc::new(event));
        }
    }

    pub(crate) fn hold(&self, workspace: &str, event: Event) {
        let mut hubs = lock(&self.hubs);
        if hubs.by_workspace.contains_key(workspace) {
            hubs.held.push((workspace.to_owned(), event));
        }
    }

    pub(crate) fn take_held(&self) -> Vec<(String, Event)> {
        mem::take(&mut lock(&self.hubs).held)
    }

    pub(crate) fn open_seats(&self) -> Vec<OpenSeat> {
        let hubs = lock(&self.hubs);
        let mut seats = Vec::new();
        for (workspace, hub) in &hubs.by_workspace {
            for (id, listener) in &hub.listeners {
                seats.push(OpenSeat {
                    workspace: workspace.clone(),
                    id: *id,
                    user: listener.user.clone(),
                    session: listener.session.clone(),
                });
            }
        }
        seats
    }

    /// Does nothing for a socket that has left since.
    pub(crate) fn settle(&self, seat: &OpenSeat, standing: EventStanding) {
        let hubs = lock(&self.hubs);
        if let Some(listener) = hubs.listener(&seat.workspace, seat.id) {
            listener.settle(standing);
        }
    }

    /// Refuses new sockets and closes the open ones.
    pub(crate) fn close_all(&self) {
        let mut hubs = lock(&self.hubs);
        hubs.closing = true;
        for hub in hubs.by_workspace.values() {
            for listener in hub.listeners.values() {
                listener.close(CLOSE_GOING_AWAY);
            }
        }
    }

    pub(crate) fn count(&self) -> usize {
        lock(&self.hubs).by_workspace.len()
    }

    fn leave(&self, workspace: &str, id: u64) {
        let mut hubs = lock(&self.hubs);
        if let Some(hub) = hubs.by_workspace.get_mut(workspace) {
            hub.listeners.remove(&id);
            if hub.listeners.is_empty() {
                hubs.by_workspace.remove(workspace);
            }
        }
    }
}

/// One events socket. Dropping it leaves the hub and frees its place in the socket count.
pub struct EventConnection {
    hubs: Arc<EventHubs>,
    workspace: String,
    id: u64,
    events: broadcast::Receiver<Arc<Event>>,
    standing: watch::Receiver<Standing>,
    /// The value of `Standing::changes` the client knows of.
    told: u64,
    _permit: OwnedSemaphorePermit,
}

impl EventConnection {
    /// Applies a check made after joining, for a change that landed before the join. A recheck
    /// that already changed the spaces may have read later than this check, and any change after
    /// it has a recheck of its own, so its spaces stay. A close always holds.
    pub fn settle(&self, standing: EventStanding) {
        let hubs = lock(&self.hubs.hubs);
        if let Some(listener) = hubs.listener(&self.workspace, self.id)
            && (matches!(standing, EventStanding::Close(_))
                || listener.standing.borrow().changes == 0)
        {
            listener.settle(standing);
        }
    }

    /// The next frame or close. Cancel safe.
    pub async fn next(&mut self) -> EventOutbound {
        loop {
            tokio::select! {
                biased;
                changed = self.standing.changed() => {
                    if changed.is_err() {
                        return EventOutbound::Close(CLOSE_GOING_AWAY);
                    }
                    let standing = self.standing.borrow_and_update();
                    if let Some(code) = standing.close {
                        return EventOutbound::Close(code);
                    }
                    if standing.changes != self.told {
                        self.told = standing.changes;
                        return EventOutbound::Text(ACCESS_CHANGED.to_owned());
                    }
                }
                event = self.events.recv() => match event {
                    Ok(event) => {
                        if self.reads(&event.space) {
                            return EventOutbound::Text(event.frame.clone());
                        }
                    }
                    // The client reloads what it shows, which covers all that is still queued.
                    Err(RecvError::Lagged(_)) => {
                        self.events = self.events.resubscribe();
                        return EventOutbound::Text(RESYNC.to_owned());
                    }
                    Err(RecvError::Closed) => return EventOutbound::Close(CLOSE_GOING_AWAY),
                },
            }
        }
    }

    fn reads(&self, space: &str) -> bool {
        let standing = self.standing.borrow();
        let found = standing
            .spaces
            .binary_search_by(|(id, _)| id.as_str().cmp(space));
        found.is_ok()
    }
}

impl Drop for EventConnection {
    fn drop(&mut self) {
        self.hubs.leave(&self.workspace, self.id);
    }
}
