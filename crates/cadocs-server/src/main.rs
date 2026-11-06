use std::ffi::c_int;
use std::fs::{self, File};
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use cadocs_collab::{DocRegistry, Permissive};
use cadocs_server::assets::WebAssets;
use cadocs_server::config::Config;
use cadocs_server::{AppState, serve};
use cadocs_store::{DocStore, SqliteStore};
use tokio::net::TcpListener;
use tokio::task::spawn_blocking;
use tokio::time::sleep;
use tracing_subscriber::EnvFilter;

const WORKER_THREADS: usize = 2;
const MAX_BLOCKING_THREADS: usize = 4;
/// The default made explicit: only the pages a thread touches count towards RSS.
const THREAD_STACK_BYTES: usize = 2 * 1024 * 1024;
/// Rooms left idle together unload together, and one trim is enough for all of them.
const TRIM_DELAY: Duration = Duration::from_secs(5);

unsafe extern "C" {
    /// glibc, the only C library cadocs targets (`docs/OPERATIONS.md`).
    fn malloc_trim(pad: usize) -> c_int;
}

fn main() -> anyhow::Result<()> {
    let config = Config::from_env()?;
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

async fn run(config: Config) -> anyhow::Result<()> {
    fs::create_dir_all(&config.data_dir)
        .with_context(|| format!("cannot create {}", config.data_dir.display()))?;
    let _lock = lock_data_dir(&config)?;

    let store = Arc::new(SqliteStore::open(&config.data_dir.join("cadocs.db")).await?);
    let registry = DocRegistry::new(store.clone(), config.collab.clone());
    tokio::spawn(return_freed_memory(registry.clone()));
    let state = AppState::new(registry, store, Arc::new(Permissive), config.socket);

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("cannot listen on {}", config.bind))?;
    tracing::info!(address = %config.bind, "listening");
    serve::<_, _, WebAssets>(listener, state, shutdown_signal()).await
}

/// glibc keeps what an unloaded room frees, because the free blocks sit below live ones (ADR 0015).
async fn return_freed_memory<S: DocStore>(registry: DocRegistry<S>) {
    loop {
        registry.room_unloaded().await;
        sleep(TRIM_DELAY).await;
        // Blocking: it walks each heap with that arena locked.
        // SAFETY: takes no pointer, and glibc locks each arena itself.
        let _ = spawn_blocking(|| unsafe { malloc_trim(0) }).await;
    }
}

/// Two servers on one folder would hand out the same `seq` (ADR 0006). The OS drops the lock when
/// the process ends, however it ends.
fn lock_data_dir(config: &Config) -> anyhow::Result<File> {
    let path = config.data_dir.join("cadocs.lock");
    let file = File::create(&path).with_context(|| format!("cannot open {}", path.display()))?;
    file.try_lock()
        .with_context(|| format!("{} is in use by another server", config.data_dir.display()))?;
    Ok(file)
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
