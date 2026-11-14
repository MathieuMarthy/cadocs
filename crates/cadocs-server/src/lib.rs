//! HTTP and WebSocket layer. The only crate that knows about axum.

pub mod api;
pub mod assets;
pub mod backup;
pub mod cli;
pub mod client_ip;
pub mod config;
pub mod data_dir;
pub mod memory;
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
use axum::middleware::{from_fn_with_state, map_response_with_state};
use axum::routing::{any, get};
use cadocs_collab::{Authorizer, DocRegistry};
use cadocs_core::permissions::WorkspaceCreation;
use cadocs_files::FileStore;
use cadocs_jobs::{Cleanup, Index, JobConfig, JobQueue, Revisions, ServerJobs, Worker};
use cadocs_store::{
    AccountStore, AttachmentStore, DocStore, JobStore, PageStore, RevisionStore, SearchStore,
    SpaceStore, WorkspaceStore,
};
use rust_embed::RustEmbed;
use tokio::net::TcpListener;
use tokio::sync::{Notify, oneshot, watch};
use tokio::time::{MissedTickBehavior, interval, timeout};

use crate::config::{AttachmentLimits, HttpConfig, SocketConfig};
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
    pub files: Arc<dyn FileStore>,
    pub attachments: AttachmentLimits,
    /// Shared by the routes that check or hash a password.
    pub password_guesses: Arc<RateLimiter>,
    /// What first-run setup needs, new at each start and shown only in the log (ADR 0021).
    pub setup_code: Arc<str>,
    /// Who may create a workspace (ADR 0024): any user unless set otherwise after `new`.
    pub workspace_creation: WorkspaceCreation,
    /// Each socket task holds a receiver, so shutdown can wait for them.
    sockets: watch::Sender<()>,
    /// Wakes the recheck of every socket.
    access: Arc<Notify>,
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
            files: self.files.clone(),
            attachments: self.attachments,
            password_guesses: self.password_guesses.clone(),
            setup_code: self.setup_code.clone(),
            workspace_creation: self.workspace_creation,
            sockets: self.sockets.clone(),
            access: self.access.clone(),
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
        files: Arc<dyn FileStore>,
        attachments: AttachmentLimits,
    ) -> Self {
        Self {
            registry,
            store,
            authorizer,
            socket,
            http: Arc::new(http),
            passwords: Arc::default(),
            files,
            attachments,
            password_guesses: Arc::new(RateLimiter::new(
                api::auth::GUESS_BURST,
                api::auth::GUESS_REFILL,
            )),
            // Without a random source no session could start either.
            setup_code: session::new_token()
                .expect("the system's random source")
                .into(),
            workspace_creation: WorkspaceCreation::default(),
            sockets: watch::Sender::new(()),
            access: Arc::default(),
        }
    }

    /// To call once a change that may alter what a user can reach is stored. Every socket gets
    /// rechecked soon after. This does not wait for that: a client that goes away cancels a handler,
    /// and changes in a burst share one pass.
    pub fn access_changed(&self) {
        self.access.notify_one();
    }
}

/// Starts the worker, which runs until the process ends: a job it cuts short runs again at the next
/// start. The queue is for whatever adds jobs.
pub fn start_jobs<S: DocStore + JobStore + AttachmentStore + SearchStore + RevisionStore, A>(
    state: &AppState<S, A>,
    config: JobConfig,
) -> JobQueue<S> {
    let queue = JobQueue::new(state.store.clone());
    let cleanup = Cleanup::new(
        state.store.clone(),
        state.registry.clone(),
        state.access.clone(),
        state.files.clone(),
        config.trash_retention,
    );
    let index = Index::new(state.store.clone(), state.registry.clone());
    let revisions = Revisions::new(
        state.store.clone(),
        state.registry.clone(),
        config.revision_retention,
    );
    let jobs = ServerJobs::new(cleanup, index, revisions);
    tokio::spawn(Worker::new(queue.clone(), Arc::new(jobs), config).run());
    queue
}

/// Rechecks every socket after each change, and on a timer for what no request announces: a session
/// that expires, or a command run beside the server. The timer also retries a pass that failed.
async fn watch_access<S: DocStore, A: Authorizer>(
    registry: DocRegistry<S>,
    authorizer: Arc<A>,
    access: Arc<Notify>,
    every: Duration,
) {
    let mut timer = interval(every);
    timer.set_missed_tick_behavior(MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            () = access.notified() => {}
            _ = timer.tick() => {}
        }
        // In a task of its own, so that a panic costs one pass and not every later revocation. The
        // registry logs a storage failure, and the runtime a panic.
        let registry = registry.clone();
        let authorizer = authorizer.clone();
        let pass = tokio::spawn(async move { registry.recheck(authorizer.as_ref()).await });
        let _ = pass.await;
    }
}

pub fn router<S, A, E>(state: AppState<S, A>) -> Router
where
    S: DocStore
        + AccountStore
        + WorkspaceStore
        + SpaceStore
        + PageStore
        + AttachmentStore
        + SearchStore
        + RevisionStore,
    A: Authorizer,
    E: RustEmbed + 'static,
{
    Router::new()
        .route("/health", get(health::<S, A>))
        .route(
            "/ws/pages/{page}",
            get(ws::upgrade::<S, A>)
                .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
                .route_layer(from_fn_with_state(state.http.clone(), api::check_origin)),
        )
        .route(
            "/ws/events",
            get(ws::upgrade_events::<S, A>)
                .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
                .route_layer(from_fn_with_state(state.http.clone(), api::check_origin)),
        )
        .nest("/api/v1", api::router(&state))
        .route("/api/v1/", any(api::not_found))
        .fallback_service(get(assets::serve::<E>))
        .layer(map_response_with_state(
            Arc::new(security::SecurityHeaders::new(
                state.files.download_origin().as_deref(),
            )),
            security::add_headers,
        ))
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
    S: DocStore
        + AccountStore
        + WorkspaceStore
        + SpaceStore
        + PageStore
        + AttachmentStore
        + SearchStore
        + RevisionStore,
    A: Authorizer,
    E: RustEmbed + 'static,
>(
    listener: TcpListener,
    state: AppState<S, A>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> anyhow::Result<()> {
    let registry = state.registry.clone();
    let sockets = state.sockets.clone();
    let watcher = tokio::spawn(watch_access(
        registry.clone(),
        state.authorizer.clone(),
        state.access.clone(),
        state.socket.recheck,
    ));
    let (stop, stopped) = oneshot::channel::<()>();
    let app = router::<S, A, E>(state).into_make_service_with_connect_info::<SocketAddr>();
    let mut http = axum::serve(listener, app)
        .with_graceful_shutdown(async {
            let _ = stopped.await;
        })
        .into_future();

    tokio::select! {
        result = &mut http => {
            watcher.abort();
            return result.context("the HTTP server stopped");
        }
        () = shutdown => {}
    }
    watcher.abort();
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
