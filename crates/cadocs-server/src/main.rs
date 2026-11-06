use std::env;
use std::ffi::c_int;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use cadocs_collab::{DocRegistry, PageAuthorizer};
use cadocs_server::api::auth::normalize_email;
use cadocs_server::assets::WebAssets;
use cadocs_server::cli::{self, Command};
use cadocs_server::config::Config;
use cadocs_server::password::Passwords;
use cadocs_server::session::new_password_reset;
use cadocs_server::{AppState, data_dir, serve};
use cadocs_store::{AccountStore, DocStore, SqliteStore};
use tokio::net::TcpListener;
use tokio::task::spawn_blocking;
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;

const WORKER_THREADS: usize = 2;
const MAX_BLOCKING_THREADS: usize = 4;
/// The default made explicit: only the pages a thread touches count towards RSS.
const THREAD_STACK_BYTES: usize = 2 * 1024 * 1024;
/// Rooms left idle together unload together, and one trim is enough for all of them. So is one
/// for a burst of logins.
const TRIM_DELAY: Duration = Duration::from_secs(5);

unsafe extern "C" {
    /// glibc, the only C library cadocs targets (`docs/OPERATIONS.md`).
    fn malloc_trim(pad: usize) -> c_int;
}

fn main() -> anyhow::Result<()> {
    // First, so a mistyped command is not answered with a missing setting.
    let command = cli::parse(env::args_os().skip(1))?;
    let config = Config::from_env()?;
    match command {
        Command::Serve => run_server(config),
        Command::ResetPassword { email } => tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?
            .block_on(reset_password(config, &email)),
    }
}

fn run_server(config: Config) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&config.log).context("invalid CADOCS_LOG")?)
        .init();

    tokio::runtime::Builder::new_multi_thread()
        .worker_threads(WORKER_THREADS)
        .max_blocking_threads(MAX_BLOCKING_THREADS)
        .thread_stack_size(THREAD_STACK_BYTES)
        .enable_all()
        .build()?
        .block_on(run(config))
}

/// Beside a running server, so it takes no lock on the data folder (`docs/OPERATIONS.md`).
async fn reset_password(config: Config, email: &str) -> anyhow::Result<()> {
    let path = config.data_dir.join("cadocs.db");
    let store = SqliteStore::open_existing(&path)
        .await
        .with_context(|| format!("cannot open {}", path.display()))?;
    let email = normalize_email(email);
    let found = store.credentials(&email).await?;
    let found = found.with_context(|| format!("no account uses {email}"))?;
    let (link, reset) = new_password_reset(&config.http.public_url, &found.user.id, None)?;
    store.create_password_reset(&reset).await?;
    println!("{link}");
    Ok(())
}

async fn run(config: Config) -> anyhow::Result<()> {
    let _lock = data_dir::lock(&config.data_dir)?;

    let store = Arc::new(SqliteStore::open(&config.data_dir.join("cadocs.db")).await?);
    let registry = DocRegistry::new(store.clone(), config.collab.clone());
    let state = AppState::new(
        registry.clone(),
        store.clone(),
        Arc::new(PageAuthorizer::new(store)),
        config.socket,
        config.http,
    );
    tokio::spawn(return_freed_memory(registry, state.passwords.clone()));

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("cannot listen on {}", config.bind))?;
    tracing::info!(address = %config.bind, "listening");
    serve::<_, _, WebAssets>(listener, state, shutdown_signal()).await
}

/// glibc keeps what an unloaded room frees, because the free blocks sit below live ones (ADR 0015).
/// It keeps the 19 MB of a password hash too, as its mmap threshold grows after the first one.
async fn return_freed_memory<S: DocStore>(registry: DocRegistry<S>, passwords: Arc<Passwords>) {
    loop {
        tokio::select! {
            () = registry.room_unloaded() => {}
            () = passwords.hash_finished() => {}
        }
        sleep(TRIM_DELAY).await;
        // Blocking: it walks each heap with that arena locked.
        // SAFETY: takes no pointer, and glibc locks each arena itself.
        let _ = spawn_blocking(|| unsafe { malloc_trim(0) }).await;
    }
}

async fn shutdown_signal() {
    use tokio::signal::unix::{SignalKind, signal};

    let mut terminate = signal(SignalKind::terminate()).expect("cannot listen for SIGTERM");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = terminate.recv() => {}
    }
    tracing::info!("shutting down");
}
