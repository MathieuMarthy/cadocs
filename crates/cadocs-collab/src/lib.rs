//! Collaboration rooms: y-sync, batched persistence, compaction and unloading (ADR 0003).
//!
//! This crate knows no socket. `cadocs-server` moves frames between a WebSocket and a
//! [`Connection`], and maps [`CollabError`] to an HTTP status before the upgrade.

mod connection;
mod registry;
mod room;

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use cadocs_core::permissions::{CLOSE_NOT_FOUND, Scope, SocketMode, page_socket_mode};
use cadocs_store::{PageStore, StoreError};

pub use connection::{Close, Connection, Outbound};
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
    pub max_rooms: usize,
    pub max_sockets: usize,
    pub max_frame_bytes: usize,
    /// Queued frames per socket.
    pub send_queue: usize,
}

impl Default for CollabConfig {
    fn default() -> Self {
        Self {
            flush_interval: Duration::from_millis(500),
            compact_updates: 500,
            room_idle: Duration::from_secs(60),
            room_queue_bytes: 4 * 1024 * 1024,
            max_rooms: 256,
            max_sockets: 512,
            max_frame_bytes: 8 * 1024 * 1024,
            send_queue: 256,
        }
    }
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum CollabError {
    #[error("store error: {0}")]
    Store(Arc<StoreError>),
    #[error("stored document {doc_id} cannot be decoded")]
    CorruptDocument { doc_id: String },
    /// Every room is live at `max_rooms`, or `max_sockets` is reached.
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

/// Decides the mode of a page socket before the upgrade. `None` refuses it.
pub trait Authorizer: Send + Sync + 'static {
    fn authorize(
        &self,
        page_id: &str,
        user_id: &str,
    ) -> impl Future<Output = Result<Option<SocketMode>, StoreError>> + Send;
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

impl<S: PageStore> Authorizer for PageAuthorizer<S> {
    async fn authorize(
        &self,
        page_id: &str,
        user_id: &str,
    ) -> Result<Option<SocketMode>, StoreError> {
        let page = self.store.page(page_id, user_id).await?;
        // `page` hides what is trashed, so a page it returns is live.
        Ok(page.and_then(|page| page_socket_mode(Scope::live(Some(page.role)))))
    }
}
