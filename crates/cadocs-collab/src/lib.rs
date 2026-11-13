//! Collaboration rooms: y-sync, batched persistence, compaction and unloading (ADR 0003).
//!
//! This crate knows no socket. `cadocs-server` moves frames between a WebSocket and a
//! [`Connection`], and maps [`CollabError`] to an HTTP status before the upgrade.

mod connection;
mod events;
mod registry;
mod room;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use cadocs_core::permissions::{
    CLOSE_FORBIDDEN, CLOSE_NOT_FOUND, CLOSE_SESSION_ENDED, Scope, SocketMode, SocketOutcome,
    SpaceRole, page_socket_mode, recompute_page_socket,
};
use cadocs_store::{
    AccountStore, PageStore, SessionStanding, SpaceStore, StoreError, WorkspaceReach, now_ms,
};

pub use connection::{Close, Connection, Outbound};
pub use events::{Event, EventConnection, EventOutbound, PageEvent, SpaceEvent};
pub use registry::DocRegistry;

/// Server shutting down.
pub const CLOSE_GOING_AWAY: u16 = 1001;
/// Malformed frame.
pub const CLOSE_PROTOCOL_ERROR: u16 = 1002;
/// Frame over `max_frame_bytes`.
pub const CLOSE_FRAME_TOO_LARGE: u16 = 1009;
/// The room queue is full because storage is failing.
pub const CLOSE_STORAGE_FAILING: u16 = 1013;
/// The page was deleted under the room.
pub const CLOSE_PAGE_GONE: u16 = CLOSE_NOT_FOUND;
/// The client stopped reading.
pub const CLOSE_TOO_SLOW: u16 = 4408;

/// What a loaded room is counted as at least, against [`CollabConfig::room_memory`].
pub const ROOM_MIN_BYTES: usize = 64 * 1024;
/// What a loaded room is counted as per byte of its encoded document. A room was measured at 10 to
/// 15 times its encoded size: each yrs block is a heap structure (`docs/ARCHITECTURE.md`).
pub const ROOM_BYTES_PER_ENCODED_BYTE: usize = 12;

/// The limits of [OPERATIONS.md](../../docs/OPERATIONS.md). Ping settings belong to the socket
/// layer in `cadocs-server`.
#[derive(Debug, Clone)]
pub struct CollabConfig {
    pub flush_interval: Duration,
    /// Stored updates of a document past which it compacts.
    pub compact_updates: usize,
    /// Delay before an empty room unloads.
    pub room_idle: Duration,
    /// Unflushed bytes a room may hold.
    pub room_queue_bytes: usize,
    /// What loaded rooms may take together, as estimated from their encoded size. Past it, idle
    /// rooms unload first, then connections are refused. It is the only bound on the number of
    /// rooms (ADR 0022). Rooms load one at a time, and a room on its way out counts until it is
    /// gone, so the budget is crossed by one room at most.
    pub room_memory: usize,
    pub max_sockets: usize,
    pub max_frame_bytes: usize,
    /// Queued frames per socket.
    pub send_queue: usize,
    /// Documents read at once outside a room.
    pub doc_reads: usize,
}

impl Default for CollabConfig {
    fn default() -> Self {
        Self {
            flush_interval: Duration::from_millis(500),
            compact_updates: 500,
            room_idle: Duration::from_secs(60),
            room_queue_bytes: 4 * 1024 * 1024,
            // 100 MB of RSS, the MVP target, less about 15 MB idle and 12 MB for 200 sockets. The
            // 100-document benchmark needs about 66 MiB (docs/notes/benchmark.md).
            room_memory: 72 * 1024 * 1024,
            max_sockets: 512,
            max_frame_bytes: 8 * 1024 * 1024,
            send_queue: 256,
            doc_reads: 2,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CollabError {
    #[error("store error: {0}")]
    Store(Arc<StoreError>),
    #[error("stored document {doc_id} cannot be decoded")]
    CorruptDocument { doc_id: String },
    /// Every room is live at `room_memory`, or `max_sockets` is reached.
    #[error("no room or socket available")]
    AtCapacity,
    #[error("the server is shutting down")]
    ShuttingDown,
    /// The room holds all it may while its flushes fail.
    #[error("the storage of this document is failing")]
    StorageFailing,
}

impl From<StoreError> for CollabError {
    fn from(error: StoreError) -> Self {
        Self::Store(Arc::new(error))
    }
}

/// An open page socket, as a recheck sees it.
#[derive(Debug, Clone, Copy)]
pub struct Seat<'a> {
    pub page_id: &'a str,
    pub user_id: &'a str,
    /// The hash of the session that opened the socket.
    pub session: &'a str,
    pub mode: SocketMode,
}

/// An open events socket, as a recheck sees it.
#[derive(Debug, Clone, Copy)]
pub struct EventSeat<'a> {
    pub workspace_id: &'a str,
    pub user_id: &'a str,
    /// The hash of the session that opened the socket.
    pub session: &'a str,
}

/// What an events socket is worth now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EventStanding {
    /// It stays, with the spaces its user has a role on, ordered by id.
    Open(Vec<(String, SpaceRole)>),
    Close(u16),
}

pub trait Authorizer: Send + Sync + 'static {
    /// Decides the mode of a page socket before the upgrade. `None` refuses it.
    fn authorize(
        &self,
        page_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<SocketMode>, StoreError>> + Send;

    /// What must happen to an open socket, as things stand now.
    fn recheck(
        &self,
        seat: &Seat<'_>,
    ) -> impl Future<Output = Result<SocketOutcome, StoreError>> + Send;

    /// Decides an events socket, before the upgrade and for as long as it stays open.
    fn recheck_events(
        &self,
        seat: &EventSeat<'_>,
    ) -> impl Future<Output = Result<EventStanding, StoreError>> + Send;
}

/// Resolves the role of a user from the database, as REST does.
#[derive(Debug)]
pub struct PageAuthorizer<S> {
    store: Arc<S>,
}

impl<S> PageAuthorizer<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self { store }
    }
}

impl<S: PageStore + SpaceStore + AccountStore> Authorizer for PageAuthorizer<S> {
    async fn authorize(
        &self,
        page_id: &str,
        user_id: &str,
    ) -> Result<Option<SocketMode>, StoreError> {
        let page = self.store.page(page_id, user_id).await?;
        // `page` hides what is trashed, so a page it returns is live.
        Ok(page.and_then(|page| page_socket_mode(Scope::live(Some(page.role)))))
    }

    async fn recheck(&self, seat: &Seat<'_>) -> Result<SocketOutcome, StoreError> {
        let standing = self
            .store
            .session_standing(seat.session, seat.user_id, now_ms());
        match standing.await? {
            SessionStanding::Active => {}
            SessionStanding::Disabled => return Ok(SocketOutcome::Close(CLOSE_FORBIDDEN)),
            SessionStanding::Ended => return Ok(SocketOutcome::Close(CLOSE_SESSION_ENDED)),
        }
        let scope = self.store.page_scope(seat.page_id, seat.user_id).await?;
        Ok(
            scope.map_or(SocketOutcome::Close(CLOSE_NOT_FOUND), |scope| {
                recompute_page_socket(seat.mode, scope)
            }),
        )
    }

    async fn recheck_events(&self, seat: &EventSeat<'_>) -> Result<EventStanding, StoreError> {
        let standing = self
            .store
            .session_standing(seat.session, seat.user_id, now_ms());
        match standing.await? {
            SessionStanding::Active => {}
            SessionStanding::Disabled => return Ok(EventStanding::Close(CLOSE_FORBIDDEN)),
            SessionStanding::Ended => return Ok(EventStanding::Close(CLOSE_SESSION_ENDED)),
        }
        let reach = self.store.workspace_reach(seat.workspace_id, seat.user_id);
        Ok(match reach.await? {
            WorkspaceReach::Member(spaces) => EventStanding::Open(spaces),
            WorkspaceReach::Stranger => EventStanding::Close(CLOSE_FORBIDDEN),
            WorkspaceReach::Gone => EventStanding::Close(CLOSE_NOT_FOUND),
        })
    }
}
