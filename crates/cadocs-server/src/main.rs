use std::env;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use cadocs_collab::{DocRegistry, PageAuthorizer};
use cadocs_files::{FileStore, LocalFileStore};
use cadocs_mail::{Mail, Mailer, Outbox, SmtpMailer};
use cadocs_server::api::auth::normalize_email;
use cadocs_server::assets::WebAssets;
use cadocs_server::cli::{self, Command};
use cadocs_server::config::{Config, FileStoreConfig};
use cadocs_server::memory;
use cadocs_server::session::{new_password_reset, setup_link};
use cadocs_server::{AppState, backup, data_dir, privileges, serve, start_jobs};
use cadocs_store::{AccountStore, DocStore, SqliteStore};
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

fn main() -> anyhow::Result<()> {
    memory::fix_mmap_threshold();
    // First, so a mistyped command is not answered with a missing setting.
    let command = cli::parse(env::args_os().skip(1))?;
    let config = Config::from_env()?;
    // Before any thread or file, and for every command: a backup run as root would leave root's
    // files beside the server's (ADR 0025).
    privileges::drop_root(&config.data_dir, config.run_as)?;
    match command {
        Command::Serve => run_server(config),
        Command::ResetPassword { email } => on_one_thread(reset_password(config, &email)),
        Command::TestMail { to } => on_one_thread(test_mail(config, to)),
        Command::Backup { to } => on_one_thread(async move {
            let copied = backup::backup(&config.data_dir, &to, local_files(&config)).await?;
            println!(
                "backed up to {}: the database and {} new files",
                to.display(),
                copied.files
            );
            Ok(())
        }),
        Command::Restore { from } => on_one_thread(async move {
            let copied = backup::restore(&from, &config.data_dir, local_files(&config)).await?;
            let data_dir = config.data_dir.display();
            println!(
                "restored into {data_dir}: the database and {} files",
                copied.files
            );
            Ok(())
        }),
    }
}

/// A command other than `serve` runs on one thread.
fn on_one_thread(work: impl Future<Output = anyhow::Result<()>>) -> anyhow::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(work)
}

/// With S3, files are backed up on the S3 side (ADR 0020).
fn local_files(config: &Config) -> bool {
    matches!(config.files, FileStoreConfig::Local)
}

fn run_server(config: Config) -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&config.log).context("invalid CADOCS_LOG")?)
        .init();
    for name in &config.ignored {
        tracing::warn!("{name} is no longer read and is ignored");
    }

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

/// Sends one message through the relay, so an operator sees a wrong setting in the relay's own
/// words. It opens no database.
async fn test_mail(config: Config, to: String) -> anyhow::Result<()> {
    let smtp = config
        .mail
        .context("mail is off: set CADOCS_SMTP_HOST and CADOCS_SMTP_FROM")?;
    let relay = format!("{}:{}", smtp.host, smtp.port);
    let origin = config.http.public_url.origin();
    let mail = Mail {
        to,
        subject: "cadocs test message".to_owned(),
        // Short lines, so the message goes out as it is written, with no encoding.
        text: format!(
            "This is a test message from the cadocs instance at\n{origin}\n\n\
             If you can read it, the instance can send mail.\n"
        ),
    };
    SmtpMailer::new(smtp)?
        .send(&mail)
        .await
        .with_context(|| format!("no message sent through {relay}"))?;
    println!("{relay} took a message for {}", mail.to);
    Ok(())
}

/// Says in the log whether the relay answers. A relay that is down stops no start: the wiki
/// works without it, and mail comes back with the relay.
async fn check_relay(mailer: Arc<SmtpMailer>, relay: String) {
    match mailer.check().await {
        Ok(()) => tracing::info!("the mail relay {relay} answers"),
        Err(error) => tracing::warn!("the mail relay {relay} cannot be used for now, {error}"),
    }
}

/// The store `CADOCS_FILE_STORE` names (ADR 0020). S3 is checked here, so a wrong setting stops
/// the start instead of failing the first upload.
async fn open_files(config: &Config) -> anyhow::Result<Arc<dyn FileStore>> {
    let data_dir = &config.data_dir;
    Ok(match &config.files {
        FileStoreConfig::Local => Arc::new(
            LocalFileStore::open(data_dir)
                .with_context(|| format!("cannot prepare {}", data_dir.display()))?,
        ),
        #[cfg(feature = "s3")]
        FileStoreConfig::S3(s3) => {
            let what = format!("cannot reach bucket {:?} at {}", s3.bucket, s3.endpoint);
            Arc::new(
                cadocs_files::S3FileStore::open(data_dir, s3.clone())
                    .await
                    .context(what)?,
            )
        }
    })
}

async fn run(config: Config) -> anyhow::Result<()> {
    let _lock = data_dir::lock(&config.data_dir)?;

    let store = SqliteStore::open(&config.data_dir.join("cadocs.db")).await?;
    let store = Arc::new(store.with_revision_timing(config.revisions));
    let files = open_files(&config).await?;
    let registry = DocRegistry::new(store.clone(), config.collab.clone());
    let mut state = AppState::new(
        registry.clone(),
        store.clone(),
        Arc::new(PageAuthorizer::new(store)),
        config.socket,
        config.http,
        files,
        config.attachments,
    );
    state.workspace_creation = config.workspace_creation;
    state.registration = config.registration;
    if let Some(smtp) = config.mail {
        let relay = format!("{}:{}", smtp.host, smtp.port);
        let mailer = Arc::new(SmtpMailer::new(smtp)?);
        state.mail = Some(Outbox::start(mailer.clone()));
        tokio::spawn(check_relay(mailer, relay));
    }
    tokio::spawn(return_freed_memory(registry));
    start_jobs(&state, config.jobs);

    let listener = TcpListener::bind(config.bind)
        .await
        .with_context(|| format!("cannot listen on {}", config.bind))?;
    tracing::info!(address = %config.bind, "listening");
    // The only place the code shows, so whoever reaches the instance first cannot claim it.
    if !state.store.has_users().await? {
        let link = setup_link(&state.http.public_url, &state.setup_code);
        tracing::warn!("first-run setup is pending: open {link} to create the first account");
    }
    serve::<_, _, WebAssets>(listener, state, shutdown_signal()).await
}

/// glibc keeps what an unloaded room frees, because the free blocks sit below live ones (ADR 0015).
async fn return_freed_memory<S: DocStore>(registry: DocRegistry<S>) {
    loop {
        registry.room_unloaded().await;
        sleep(TRIM_DELAY).await;
        let _ = spawn_blocking(memory::trim).await;
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
