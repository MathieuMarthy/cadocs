//! HTTP and WebSocket layer. The only crate that knows about axum.

pub mod assets;
pub mod config;
mod ws;

use std::future::{Future, IntoFuture};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::get;
use cadocs_collab::{Authorizer, DocRegistry};
use cadocs_store::DocStore;
use rust_embed::RustEmbed;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;

use crate::config::SocketConfig;

/// How long shutdown waits for HTTP requests in progress.
const HTTP_GRACE: Duration = Duration::from_secs(2);
/// How long shutdown waits for sockets to finish closing.
const SOCKETS_GRACE: Duration = Duration::from_secs(3);

pub struct AppState<S, A> {
    pub registry: DocRegistry<S>,
    pub store: Arc<S>,
    pub authorizer: Arc<A>,
    pub socket: SocketConfig,
    /// Each socket task holds a receiver, so shutdown can wait for them.
    sockets: watch::Sender<()>,
}

impl<S, A> Clone for AppState<S, A> {
    fn clone(&self) -> Self {
        Self {
            registry: self.registry.clone(),
            store: self.store.clone(),
            authorizer: self.authorizer.clone(),
            socket: self.socket,
            sockets: self.sockets.clone(),
        }
    }
}

impl<S, A> AppState<S, A> {
    pub fn new(
        registry: DocRegistry<S>,
        store: Arc<S>,
        authorizer: Arc<A>,
        socket: SocketConfig,
    ) -> Self {
        Self {
            registry,
            store,
            authorizer,
            socket,
            sockets: watch::Sender::new(()),
        }
    }
}

pub fn router<S: DocStore, A: Authorizer, E: RustEmbed + 'static>(state: AppState<S, A>) -> Router {
    Router::new()
        .route("/health", get(health::<S, A>))
        .route("/ws/{doc}", get(ws::upgrade::<S, A>))
        .fallback_service(get(assets::serve::<E>))
        .with_state(state)
}

async fn health<S: DocStore, A: Authorizer>(State(state): State<AppState<S, A>>) -> StatusCode {
    if state.registry.healthy() && state.store.ping().await.is_ok() {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    }
}

/// Serves until `shutdown` resolves, then closes every socket and flushes every room.
pub async fn serve<S: DocStore, A: Authorizer, E: RustEmbed + 'static>(
    listener: TcpListener,
    state: AppState<S, A>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let registry = state.registry.clone();
    let sockets = state.sockets.clone();
    let (stop, stopped) = oneshot::channel::<()>();
    let mut http = axum::serve(listener, router::<S, A, E>(state))
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .into_future();

    tokio::select! {
        result = &mut http => return result.context("the HTTP server stopped"),
        () = shutdown => {}
    }
    let _ = stop.send(());

    // Before waiting on HTTP: a client that stalls mid-request must not hold back the flush.
    let flushed = registry.shutdown().await;
    if timeout(HTTP_GRACE, http).await.is_err() {
        tracing::warn!("some HTTP connections did not end in time");
    }
    // Upgraded sockets outlive `axum::serve`, and they still have a close frame to send.
    if timeout(SOCKETS_GRACE, sockets.closed()).await.is_err() {
        tracing::warn!("some sockets did not close in time");
    }
    flushed.context("some edits could not be flushed")
}
