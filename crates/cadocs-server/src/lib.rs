//! HTTP and WebSocket layer. The only crate that knows about axum.

pub mod api;
pub mod assets;
pub mod cli;
pub mod client_ip;
pub mod config;
pub mod data_dir;
pub mod password;
pub mod rate_limit;
pub mod security;
pub mod session;
mod ws;

use std::future::{Future, IntoFuture};
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use axum::Router;
use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::map_response;
use axum::routing::{any, get};
use cadocs_collab::{Authorizer, DocRegistry};
use cadocs_store::{AccountStore, DocStore, PageStore, SpaceStore, WorkspaceStore};
use rust_embed::RustEmbed;
use tokio::net::TcpListener;
use tokio::sync::{oneshot, watch};
use tokio::time::timeout;

use crate::config::{HttpConfig, SocketConfig};
use crate::password::Passwords;
use crate::rate_limit::RateLimiter;

/// How long shutdown waits for HTTP requests in progress.
const HTTP_GRACE: Duration = Duration::from_secs(2);
/// How long shutdown waits for sockets to finish closing.
const SOCKETS_GRACE: Duration = Duration::from_secs(3);

pub struct AppState<S, A> {
    pub registry: DocRegistry<S>,
    pub store: Arc<S>,
    pub authorizer: Arc<A>,
    pub socket: SocketConfig,
    pub http: Arc<HttpConfig>,
    pub passwords: Arc<Passwords>,
    /// Shared by the routes that check or hash a password.
    pub password_guesses: Arc<RateLimiter>,
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
            http: self.http.clone(),
            passwords: self.passwords.clone(),
            password_guesses: self.password_guesses.clone(),
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
        http: HttpConfig,
    ) -> Self {
        Self {
            registry,
            store,
            authorizer,
            socket,
            http: Arc::new(http),
            passwords: Arc::default(),
            password_guesses: Arc::new(RateLimiter::new(
                api::auth::GUESS_BURST,
                api::auth::GUESS_REFILL,
            )),
            sockets: watch::Sender::new(()),
        }
    }
}

pub fn router<S, A, E>(state: AppState<S, A>) -> Router
where
    S: DocStore + AccountStore + WorkspaceStore + SpaceStore + PageStore,
    A: Authorizer,
    E: RustEmbed + 'static,
{
    Router::new()
        .route("/health", get(health::<S, A>))
        .route("/ws/{doc}", get(ws::upgrade::<S, A>))
        .nest("/api/v1", api::router(&state))
        .route("/api/v1/", any(api::not_found))
        .fallback_service(get(assets::serve::<E>))
        .layer(map_response(security::add_headers))
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
pub async fn serve<
    S: DocStore + AccountStore + WorkspaceStore + SpaceStore + PageStore,
    A: Authorizer,
    E: RustEmbed + 'static,
>(
    listener: TcpListener,
    state: AppState<S, A>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let registry = state.registry.clone();
    let sockets = state.sockets.clone();
    let (stop, stopped) = oneshot::channel::<()>();
    let app = router::<S, A, E>(state).into_make_service_with_connect_info::<SocketAddr>();
    let mut http = axum::serve(listener, app)
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
