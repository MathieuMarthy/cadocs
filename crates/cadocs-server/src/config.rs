//! Settings from the environment, as listed in `docs/OPERATIONS.md`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use axum::http::{HeaderMap, Uri, header};
use cadocs_collab::CollabConfig;
use cadocs_core::permissions::WorkspaceCreation;
use cadocs_jobs::JobConfig;
use cadocs_mail::{MailError, Security, SmtpConfig, SmtpMailer};
use cadocs_store::RevisionTiming;

use crate::client_ip::TrustedProxies;
use crate::oidc::{self, Provider};
use crate::privileges::RunAs;

/// Far above any sensible value, and small enough for timers and semaphores not to overflow.
const MAX_SECS: u64 = 365 * 24 * 60 * 60;
const MAX_COUNT: usize = 1 << 24;
const SECS_PER_DAY: u64 = 24 * 60 * 60;
const DEFAULT_SESSION_DAYS: u64 = 30;
const MAX_SESSION_DAYS: u64 = 3650;
const DEFAULT_TRASH_DAYS: u64 = 30;
const DEFAULT_REVISION_DAYS: u64 = 30;
const DEFAULT_MAX_UPLOAD_MB: u64 = 25;
/// A zip of a page and its attachments, as an import receives it.
const DEFAULT_MAX_IMPORT_MB: u64 = 100;
/// 1 TiB: far past any wiki, and still far from overflowing once in bytes.
const MAX_MB: u64 = 1024 * 1024;
const MIB: u64 = 1024 * 1024;
const MAX_TRASH_DAYS: u64 = 3650;
#[cfg(feature = "s3")]
const DEFAULT_S3_LINK_SECS: u64 = 3600;
/// The longest a SigV4 presigned URL may live.
#[cfg(feature = "s3")]
const MAX_S3_LINK_SECS: u64 = 7 * 24 * 3600;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    /// The user root falls back to at start (ADR 0025).
    pub run_as: Option<RunAs>,
    pub bind: SocketAddr,
    pub log: String,
    pub http: HttpConfig,
    pub collab: CollabConfig,
    pub socket: SocketConfig,
    pub jobs: JobConfig,
    /// When an edit's revision is taken.
    pub revisions: RevisionTiming,
    pub attachments: AttachmentLimits,
    pub files: FileStoreConfig,
    /// Who may create a workspace (ADR 0024).
    pub workspace_creation: WorkspaceCreation,
    /// The relay mail goes through, or `None` for an instance that sends none (ADR 0026).
    pub mail: Option<SmtpConfig>,
    /// Whether people register by themselves: by mail (ADR 0026), or through `oidc`.
    pub registration: Registration,
    /// The OpenID Connect providers people log in with (ADR 0028).
    pub oidc: Vec<Provider>,
    /// Retired settings that are set, which the server names at start and otherwise ignores.
    pub ignored: Vec<&'static str>,
}

/// Settings a past version read, still accepted so an old deployment starts.
const RETIRED: &[&str] = &[
    // ADR 0022: the room budget and `CADOCS_MAX_SOCKETS` bound rooms.
    "CADOCS_MAX_ROOMS",
];

/// `CADOCS_REGISTRATION`: whether an instance with mail lets anyone make an account (ADR 0026).
/// Without mail no one registers, whatever this says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Registration {
    #[default]
    Open,
    Closed,
}

/// Where attachments go: `CADOCS_FILE_STORE` (ADR 0020).
#[derive(Debug, Clone)]
pub enum FileStoreConfig {
    /// `<data>/files`.
    Local,
    #[cfg(feature = "s3")]
    S3(cadocs_files::S3Config),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentLimits {
    pub max_upload_bytes: u64,
    /// What each workspace's attachments may take, `None` for no limit.
    pub workspace_quota_bytes: Option<u64>,
    /// The largest import body, written to `data/tmp` as it arrives.
    pub max_import_bytes: u64,
}

impl Default for AttachmentLimits {
    fn default() -> Self {
        Self {
            max_upload_bytes: DEFAULT_MAX_UPLOAD_MB * MIB,
            workspace_quota_bytes: None,
            max_import_bytes: DEFAULT_MAX_IMPORT_MB * MIB,
        }
    }
}

/// What the HTTP layer knows about how the server is reached.
#[derive(Debug, Clone)]
pub struct HttpConfig {
    pub public_url: PublicUrl,
    pub trusted_proxies: TrustedProxies,
    /// How long a session lasts after its last use.
    pub session_lifetime: Duration,
}

/// `CADOCS_PUBLIC_URL`, reduced to what is compared with a browser's `Origin`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PublicUrl {
    origin: String,
    secure: bool,
}

impl PublicUrl {
    /// Takes a scheme and a host, with an optional port. The front end lives at the root, so a path
    /// cannot work.
    pub fn parse(url: &str) -> anyhow::Result<Self> {
        let uri = url.parse::<Uri>().context("not a URL")?;
        let scheme = uri.scheme_str().unwrap_or_default();
        ensure!(matches!(scheme, "http" | "https"), "must be http or https");
        let authority = uri.authority().context("no host")?;
        let host = authority.host().to_ascii_lowercase();
        ensure!(!host.is_empty(), "no host");
        ensure!(!authority.as_str().contains('@'), "must not hold a user");
        // `Uri` accepts a port it cannot read, and `port_u16` then says there is none.
        ensure!(
            authority.as_str().len() == authority.host().len() || uri.port_u16().is_some(),
            "invalid port"
        );
        // `Uri` drops a fragment without a word.
        ensure!(
            uri.path() == "/" && uri.query().is_none() && !url.contains('#'),
            "must not have a path"
        );

        let secure = scheme == "https";
        let default_port = if secure { 443 } else { 80 };
        let origin = match uri.port_u16().filter(|port| *port != default_port) {
            Some(port) => format!("{scheme}://{host}:{port}"),
            None => format!("{scheme}://{host}"),
        };
        Ok(Self { origin, secure })
    }

    /// The form browsers send in `Origin`: lower case, and no default port.
    pub fn origin(&self) -> &str {
        &self.origin
    }

    /// The host, with its port when not the default one: what names the instance in an app.
    pub fn host(&self) -> &str {
        self.origin
            .split_once("://")
            .map_or(self.origin.as_str(), |(_, host)| host)
    }

    /// Whether cookies must carry the `Secure` flag.
    pub fn secure(&self) -> bool {
        self.secure
    }

    /// Browsers always send `Origin` on WebSocket upgrades and on mutating requests. A client
    /// outside a browser sends none, and it has no cookies of a victim to abuse.
    pub fn allows(&self, headers: &HeaderMap) -> bool {
        headers
            .get(header::ORIGIN)
            .is_none_or(|origin| origin.as_bytes() == self.origin.as_bytes())
    }
}

/// Keep-alive of a socket.
#[derive(Debug, Clone, Copy)]
pub struct SocketConfig {
    pub ping: Duration,
    /// Silence before a socket closes. Also bounds how long one send may take.
    pub ping_timeout: Duration,
    /// Time between two rechecks of every socket, which is how an expired session or a change made
    /// outside the server closes one.
    pub recheck: Duration,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(30),
            ping_timeout: Duration::from_secs(90),
            recheck: Duration::from_secs(60),
        }
    }
}

impl Config {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_lookup(|name| std::env::var(name).ok())
    }

    /// Reads the settings through `lookup`, which returns a variable's value if it is set.
    pub fn from_lookup(lookup: impl Fn(&str) -> Option<String>) -> anyhow::Result<Self> {
        let read = Reader(lookup);
        let collab = CollabConfig::default();
        let socket = SocketConfig::default();
        let config = Self {
            data_dir: read.value("CADOCS_DATA_DIR", PathBuf::from("./data"))?,
            run_as: read.run_as()?,
            bind: read.value("CADOCS_BIND", SocketAddr::from(([127, 0, 0, 1], 3000)))?,
            log: read.value("CADOCS_LOG", "info".to_owned())?,
            http: HttpConfig {
                public_url: read.public_url()?,
                trusted_proxies: read.trusted_proxies()?,
                session_lifetime: read.session_lifetime()?,
            },
            collab: CollabConfig {
                flush_interval: Duration::from_millis(read.value(
                    "CADOCS_FLUSH_INTERVAL_MS",
                    collab.flush_interval.as_millis() as u64,
                )?),
                compact_updates: read.value("CADOCS_COMPACT_UPDATES", collab.compact_updates)?,
                room_idle: read.secs("CADOCS_ROOM_IDLE_SECS", collab.room_idle)?,
                room_queue_bytes: read.value("CADOCS_ROOM_QUEUE_BYTES", collab.room_queue_bytes)?,
                room_memory: read.room_memory(collab.room_memory)?,
                max_sockets: read.value("CADOCS_MAX_SOCKETS", collab.max_sockets)?,
                max_frame_bytes: read.value("CADOCS_MAX_FRAME_BYTES", collab.max_frame_bytes)?,
                send_queue: read.value("CADOCS_SEND_QUEUE", collab.send_queue)?,
                doc_reads: read.value("CADOCS_DOC_READS", collab.doc_reads)?,
            },
            socket: SocketConfig {
                ping: read.secs("CADOCS_PING_SECS", socket.ping)?,
                ping_timeout: read.secs("CADOCS_PING_TIMEOUT_SECS", socket.ping_timeout)?,
                recheck: read.secs("CADOCS_SOCKET_RECHECK_SECS", socket.recheck)?,
            },
            jobs: JobConfig {
                trash_retention: read.trash_retention()?,
                revision_retention: read.revision_retention()?,
                ..JobConfig::default()
            },
            revisions: read.revision_timing()?,
            attachments: read.attachment_limits()?,
            files: read.file_store()?,
            workspace_creation: read.workspace_creation()?,
            mail: read.mail()?,
            registration: read.registration()?,
            oidc: read.oidc()?,
            ignored: RETIRED
                .iter()
                .copied()
                .filter(|name| (read.0)(name).is_some())
                .collect(),
        };
        config.validate()?;
        Ok(config)
    }

    /// Refuses what would panic in a timer or a channel, or close every healthy socket.
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.collab.flush_interval.is_zero(),
            "CADOCS_FLUSH_INTERVAL_MS must not be 0"
        );
        ensure!(
            self.collab.flush_interval.as_secs() <= MAX_SECS,
            "CADOCS_FLUSH_INTERVAL_MS is too large"
        );
        ensure!(
            self.collab.room_idle.as_secs() <= MAX_SECS,
            "CADOCS_ROOM_IDLE_SECS is too large"
        );
        ensure!(
            self.socket.ping_timeout.as_secs() <= MAX_SECS,
            "CADOCS_PING_TIMEOUT_SECS is too large"
        );
        ensure!(
            self.collab.max_sockets <= MAX_COUNT,
            "CADOCS_MAX_SOCKETS is too large"
        );
        ensure!(
            (1..=MAX_COUNT).contains(&self.collab.send_queue),
            "CADOCS_SEND_QUEUE must be between 1 and {MAX_COUNT}"
        );
        ensure!(
            (1..=MAX_COUNT).contains(&self.collab.doc_reads),
            "CADOCS_DOC_READS must be between 1 and {MAX_COUNT}"
        );
        ensure!(
            !self.socket.ping.is_zero(),
            "CADOCS_PING_SECS must not be 0"
        );
        ensure!(
            (1..=MAX_SECS).contains(&self.socket.recheck.as_secs()),
            "CADOCS_SOCKET_RECHECK_SECS must be between 1 and {MAX_SECS}"
        );
        ensure!(
            self.socket.ping_timeout > self.socket.ping,
            "CADOCS_PING_TIMEOUT_SECS must be above CADOCS_PING_SECS"
        );
        Ok(())
    }
}

struct Reader<F>(F);

impl<F: Fn(&str) -> Option<String>> Reader<F> {
    fn trash_retention(&self) -> anyhow::Result<Duration> {
        let days = self.value("CADOCS_TRASH_DAYS", DEFAULT_TRASH_DAYS)?;
        ensure!(
            (1..=MAX_TRASH_DAYS).contains(&days),
            "CADOCS_TRASH_DAYS must be between 1 and {MAX_TRASH_DAYS}"
        );
        Ok(Duration::from_secs(days * SECS_PER_DAY))
    }

    fn revision_retention(&self) -> anyhow::Result<Duration> {
        let days = self.value("CADOCS_REVISION_KEEP_DAYS", DEFAULT_REVISION_DAYS)?;
        ensure!(
            (1..=MAX_TRASH_DAYS).contains(&days),
            "CADOCS_REVISION_KEEP_DAYS must be between 1 and {MAX_TRASH_DAYS}"
        );
        Ok(Duration::from_secs(days * SECS_PER_DAY))
    }

    /// A pause of 0 takes a revision after each flush.
    fn revision_timing(&self) -> anyhow::Result<RevisionTiming> {
        let defaults = RevisionTiming::default();
        let pause = self.secs("CADOCS_REVISION_PAUSE_SECS", defaults.pause)?;
        let max = self.secs("CADOCS_REVISION_MAX_SECS", defaults.max)?;
        ensure!(
            max.as_secs() <= MAX_SECS,
            "CADOCS_REVISION_MAX_SECS is too large"
        );
        ensure!(
            pause <= max,
            "CADOCS_REVISION_PAUSE_SECS must be at most CADOCS_REVISION_MAX_SECS"
        );
        Ok(RevisionTiming { pause, max })
    }

    fn attachment_limits(&self) -> anyhow::Result<AttachmentLimits> {
        let upload = self.value("CADOCS_MAX_UPLOAD_MB", DEFAULT_MAX_UPLOAD_MB)?;
        ensure!(
            (1..=MAX_MB).contains(&upload),
            "CADOCS_MAX_UPLOAD_MB must be between 1 and {MAX_MB}"
        );
        let quota = self.value("CADOCS_WORKSPACE_QUOTA_MB", 0)?;
        ensure!(
            quota <= MAX_MB,
            "CADOCS_WORKSPACE_QUOTA_MB must be at most {MAX_MB}"
        );
        let import = self.value("CADOCS_MAX_IMPORT_MB", DEFAULT_MAX_IMPORT_MB)?;
        ensure!(
            (1..=MAX_MB).contains(&import),
            "CADOCS_MAX_IMPORT_MB must be between 1 and {MAX_MB}"
        );
        Ok(AttachmentLimits {
            max_upload_bytes: upload * MIB,
            workspace_quota_bytes: (quota > 0).then_some(quota * MIB),
            max_import_bytes: import * MIB,
        })
    }

    fn room_memory(&self, default: usize) -> anyhow::Result<usize> {
        let mb = self.value("CADOCS_ROOM_MEMORY_MB", default as u64 / MIB)?;
        ensure!(
            (1..=MAX_MB).contains(&mb),
            "CADOCS_ROOM_MEMORY_MB must be between 1 and {MAX_MB}"
        );
        usize::try_from(mb * MIB).context("CADOCS_ROOM_MEMORY_MB is too large for this platform")
    }

    /// Empty counts as unset, so a deployment can turn off what the image sets.
    fn run_as(&self) -> anyhow::Result<Option<RunAs>> {
        let name = "CADOCS_RUN_AS";
        match (self.0)(name).as_deref() {
            None | Some("") => Ok(None),
            Some(value) => RunAs::parse(value)
                .map(Some)
                .with_context(|| format!("invalid {name}: {value:?}")),
        }
    }

    fn workspace_creation(&self) -> anyhow::Result<WorkspaceCreation> {
        let name = "CADOCS_WORKSPACE_CREATION";
        match (self.0)(name).as_deref() {
            None | Some("all") => Ok(WorkspaceCreation::All),
            Some("admins") => Ok(WorkspaceCreation::Admins),
            Some(other) => bail!("invalid {name}: {other:?}, expected all or admins"),
        }
    }

    fn registration(&self) -> anyhow::Result<Registration> {
        let name = "CADOCS_REGISTRATION";
        match (self.0)(name).as_deref() {
            None | Some("open") => Ok(Registration::Open),
            Some("closed") => Ok(Registration::Closed),
            Some(other) => bail!("invalid {name}: {other:?}, expected open or closed"),
        }
    }

    fn file_store(&self) -> anyhow::Result<FileStoreConfig> {
        let name = "CADOCS_FILE_STORE";
        match (self.0)(name).as_deref() {
            None | Some("local") => Ok(FileStoreConfig::Local),
            Some("s3") => self.s3(),
            Some(other) => bail!("invalid {name}: {other:?}, expected local or s3"),
        }
    }

    #[cfg(not(feature = "s3"))]
    fn s3(&self) -> anyhow::Result<FileStoreConfig> {
        bail!(
            "CADOCS_FILE_STORE=s3 needs a build with S3 support: cargo build --release --features s3"
        )
    }

    #[cfg(feature = "s3")]
    fn s3(&self) -> anyhow::Result<FileStoreConfig> {
        use cadocs_files::{Downloads, S3Config};

        let required = |name: &str| {
            (self.0)(name)
                .filter(|value| !value.is_empty())
                .with_context(|| format!("{name} is required when CADOCS_FILE_STORE is s3"))
        };
        let endpoint = required("CADOCS_S3_ENDPOINT")?;
        ensure!(
            endpoint.starts_with("https://") || endpoint.starts_with("http://"),
            "invalid CADOCS_S3_ENDPOINT: {endpoint:?}, expected an http or https URL"
        );
        let downloads = match (self.0)("CADOCS_S3_DOWNLOADS").as_deref() {
            None | Some("proxy") => Downloads::Proxy,
            Some("redirect") => Downloads::Redirect,
            Some(other) => {
                bail!("invalid CADOCS_S3_DOWNLOADS: {other:?}, expected proxy or redirect")
            }
        };
        let prefix = (self.0)("CADOCS_S3_PREFIX").unwrap_or_default();
        ensure!(
            cadocs_files::valid_prefix(&prefix),
            "invalid CADOCS_S3_PREFIX: {prefix:?}, expected segments such as apps/cadocs, \
             with no leading /, no empty segment, and no . or .."
        );
        let link = self.value("CADOCS_S3_LINK_SECS", DEFAULT_S3_LINK_SECS)?;
        ensure!(
            (2..=MAX_S3_LINK_SECS).contains(&link),
            "CADOCS_S3_LINK_SECS must be between 2 and {MAX_S3_LINK_SECS}"
        );
        Ok(FileStoreConfig::S3(S3Config {
            endpoint,
            bucket: required("CADOCS_S3_BUCKET")?,
            region: self.value("CADOCS_S3_REGION", "us-east-1".to_owned())?,
            access_key_id: required("CADOCS_S3_ACCESS_KEY_ID")?,
            secret_access_key: self.s3_secret()?,
            prefix,
            downloads,
            link_lifetime: Duration::from_secs(link),
            ca_file: (self.0)("CADOCS_S3_CA_FILE").map(PathBuf::from),
        }))
    }

    #[cfg(feature = "s3")]
    fn s3_secret(&self) -> anyhow::Result<String> {
        let name = "CADOCS_S3_SECRET_ACCESS_KEY";
        let file = "CADOCS_S3_SECRET_ACCESS_KEY_FILE";
        self.secret(name, file)?
            .with_context(|| format!("{name} or {file} is required when CADOCS_FILE_STORE is s3"))
    }

    /// From the variable, or from the file `_FILE` names, as container secrets are given.
    fn secret(&self, name: &str, file: &str) -> anyhow::Result<Option<String>> {
        // An empty value is an unset one: a compose file passes that for what it does not define.
        let set = |name: &str| (self.0)(name).filter(|value| !value.is_empty());
        match (set(name), set(file)) {
            (Some(_), Some(_)) => bail!("set {name} or {file}, not both"),
            (Some(secret), None) => Ok(Some(secret)),
            (None, Some(path)) => {
                let secret = std::fs::read_to_string(&path)
                    .with_context(|| format!("cannot read {file}: {path:?}"))?;
                let secret = secret.trim_end().to_owned();
                ensure!(!secret.is_empty(), "{file} names an empty file: {path:?}");
                Ok(Some(secret))
            }
            _ => Ok(None),
        }
    }

    /// Mail is off unless `CADOCS_SMTP_HOST` names a relay (ADR 0026).
    fn mail(&self) -> anyhow::Result<Option<SmtpConfig>> {
        let set = |name: &str| (self.0)(name).filter(|value| !value.is_empty());
        let Some(host) = set("CADOCS_SMTP_HOST") else {
            return Ok(None);
        };
        // A colon is a port or a scheme, unless the whole is an IPv6 address.
        let stray_colon = host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err();
        ensure!(
            !stray_colon && !host.contains(|c: char| c.is_whitespace() || c == '/' || c == '@'),
            "invalid CADOCS_SMTP_HOST: {host:?}, expected a host name or an address, with no \
             scheme or port"
        );
        let (security, port) = match set("CADOCS_SMTP_TLS").as_deref() {
            None | Some("starttls") => (Security::StartTls, 587),
            Some("tls") => (Security::Tls, 465),
            Some("none") => (Security::None, 25),
            Some(other) => {
                bail!("invalid CADOCS_SMTP_TLS: {other:?}, expected starttls, tls or none")
            }
        };
        let port = match set("CADOCS_SMTP_PORT") {
            None => port,
            Some(value) => match value.parse::<u16>() {
                Ok(port) if port != 0 => port,
                _ => bail!("invalid CADOCS_SMTP_PORT: {value:?}, expected 1 to 65535"),
            },
        };
        let user = set("CADOCS_SMTP_USERNAME");
        let password = self.secret("CADOCS_SMTP_PASSWORD", "CADOCS_SMTP_PASSWORD_FILE")?;
        let credentials = match (user, password) {
            (Some(user), Some(password)) => Some((user, password)),
            (None, None) => None,
            (Some(_), None) => bail!(
                "CADOCS_SMTP_USERNAME needs CADOCS_SMTP_PASSWORD or CADOCS_SMTP_PASSWORD_FILE"
            ),
            (None, Some(_)) => bail!("CADOCS_SMTP_PASSWORD needs CADOCS_SMTP_USERNAME"),
        };
        ensure!(
            credentials.is_none() || security != Security::None,
            "CADOCS_SMTP_TLS=none would send CADOCS_SMTP_PASSWORD in clear: use starttls or \
             tls, or a relay that asks for no password"
        );
        let from = set("CADOCS_SMTP_FROM")
            .context("CADOCS_SMTP_FROM is required when CADOCS_SMTP_HOST is set")?;
        let config = SmtpConfig {
            host,
            port,
            security,
            credentials,
            from,
        };
        if let Err(MailError::Address(from)) = SmtpMailer::new(config.clone()) {
            bail!(
                "invalid CADOCS_SMTP_FROM: {from:?}, expected wiki@example.org or \
                 Team wiki <wiki@example.org>"
            );
        }
        Ok(Some(config))
    }

    /// `CADOCS_OIDC_PROVIDERS` names them, and each has its `CADOCS_OIDC_<ID>_*` (ADR 0028).
    fn oidc(&self) -> anyhow::Result<Vec<Provider>> {
        let set = |name: &str| (self.0)(name).filter(|value| !value.is_empty());
        let list = set("CADOCS_OIDC_PROVIDERS").unwrap_or_default();
        let mut providers = Vec::<Provider>::new();
        for id in list.split(',').map(str::trim).filter(|_| !list.is_empty()) {
            let valid = (1..=32).contains(&id.len())
                && id
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
            ensure!(
                valid,
                "invalid CADOCS_OIDC_PROVIDERS: {id:?}, expected names of lowercase letters and \
                 digits, separated by commas"
            );
            ensure!(
                providers.iter().all(|provider| provider.id != id),
                "CADOCS_OIDC_PROVIDERS names {id} twice"
            );
            let name = |setting: &str| format!("CADOCS_OIDC_{}_{setting}", id.to_uppercase());
            let required = |setting: &str| {
                let name = name(setting);
                set(&name).with_context(|| format!("{name} is required"))
            };
            let issuer = required("ISSUER")?;
            let checked = oidc::check_url(&issuer).and_then(|url| {
                ensure!(
                    url.query().is_none() && url.fragment().is_none(),
                    "it has a query"
                );
                Ok(())
            });
            if let Err(error) = checked {
                bail!("invalid {}: {issuer:?}, {error}", name("ISSUER"));
            }
            let secret = name("CLIENT_SECRET");
            let file = name("CLIENT_SECRET_FILE");
            providers.push(Provider {
                id: id.to_owned(),
                issuer,
                client_id: required("CLIENT_ID")?,
                client_secret: self
                    .secret(&secret, &file)?
                    .with_context(|| format!("{secret} or {file} is required"))?,
            });
        }
        Ok(providers)
    }

    fn session_lifetime(&self) -> anyhow::Result<Duration> {
        let days = self.value("CADOCS_SESSION_DAYS", DEFAULT_SESSION_DAYS)?;
        ensure!(
            (1..=MAX_SESSION_DAYS).contains(&days),
            "CADOCS_SESSION_DAYS must be between 1 and {MAX_SESSION_DAYS}"
        );
        Ok(Duration::from_secs(days * SECS_PER_DAY))
    }

    fn value<T>(&self, name: &str, default: T) -> anyhow::Result<T>
    where
        T: FromStr,
        T::Err: std::error::Error + Send + Sync + 'static,
    {
        match (self.0)(name) {
            Some(value) => value
                .parse()
                .with_context(|| format!("invalid {name}: {value:?}")),
            None => Ok(default),
        }
    }

    fn public_url(&self) -> anyhow::Result<PublicUrl> {
        let name = "CADOCS_PUBLIC_URL";
        let value = (self.0)(name).with_context(|| format!("{name} is required"))?;
        PublicUrl::parse(&value).with_context(|| format!("invalid {name}: {value:?}"))
    }

    fn trusted_proxies(&self) -> anyhow::Result<TrustedProxies> {
        let name = "CADOCS_TRUSTED_PROXIES";
        let value = (self.0)(name).unwrap_or_default();
        TrustedProxies::parse(&value).with_context(|| format!("invalid {name}: {value:?}"))
    }

    fn secs(&self, name: &str, default: Duration) -> anyhow::Result<Duration> {
        self.value(name, default.as_secs()).map(Duration::from_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PUBLIC_URL: (&str, &str) = ("CADOCS_PUBLIC_URL", "https://wiki.example.org");

    fn lookup(vars: &[(&str, &str)]) -> anyhow::Result<Config> {
        Config::from_lookup(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        })
    }

    /// The settings in `vars`, on top of the one that has no default.
    fn config(vars: &[(&str, &str)]) -> anyhow::Result<Config> {
        lookup(&[vars, &[PUBLIC_URL]].concat())
    }

    fn origin_headers(origin: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::ORIGIN, origin.parse().unwrap());
        headers
    }

    #[test]
    fn attachments_take_25_mb_each_and_no_quota_unless_set_otherwise() {
        let limits = config(&[]).unwrap().attachments;
        assert_eq!(limits.max_upload_bytes, 25 * 1024 * 1024);
        assert_eq!(limits.workspace_quota_bytes, None);
        assert_eq!(limits.max_import_bytes, 100 * 1024 * 1024);

        let set = config(&[
            ("CADOCS_MAX_UPLOAD_MB", "2"),
            ("CADOCS_WORKSPACE_QUOTA_MB", "10"),
            ("CADOCS_MAX_IMPORT_MB", "3"),
        ]);
        let limits = set.unwrap().attachments;
        assert_eq!(limits.max_upload_bytes, 2 * 1024 * 1024);
        assert_eq!(limits.workspace_quota_bytes, Some(10 * 1024 * 1024));
        assert_eq!(limits.max_import_bytes, 3 * 1024 * 1024);

        for (name, refused) in [
            ("CADOCS_MAX_UPLOAD_MB", "0"),
            ("CADOCS_MAX_UPLOAD_MB", "1048577"),
            ("CADOCS_WORKSPACE_QUOTA_MB", "1048577"),
            ("CADOCS_WORKSPACE_QUOTA_MB", "-1"),
            ("CADOCS_MAX_IMPORT_MB", "0"),
            ("CADOCS_MAX_IMPORT_MB", "1048577"),
        ] {
            let error = config(&[(name, refused)]).unwrap_err();
            assert!(
                format!("{error:#}").contains(name),
                "{name}={refused}: {error:#}"
            );
        }
    }

    #[test]
    fn a_session_lasts_thirty_days_unless_set_otherwise() {
        let days = |count: u64| Duration::from_secs(count * SECS_PER_DAY);
        assert_eq!(config(&[]).unwrap().http.session_lifetime, days(30));
        let set = config(&[("CADOCS_SESSION_DAYS", "7")]).unwrap();
        assert_eq!(set.http.session_lifetime, days(7));
        for refused in ["0", "3651", "-1", "soon"] {
            let error = config(&[("CADOCS_SESSION_DAYS", refused)]).unwrap_err();
            assert!(
                format!("{error:#}").contains("CADOCS_SESSION_DAYS"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn the_public_url_is_required_and_named() {
        let error = lookup(&[]).unwrap_err().to_string();
        assert!(error.contains("CADOCS_PUBLIC_URL"), "{error}");
    }

    #[test]
    fn an_invalid_public_url_names_its_variable() {
        let error = lookup(&[("CADOCS_PUBLIC_URL", "wiki")]).unwrap_err();
        assert!(error.to_string().contains("CADOCS_PUBLIC_URL"), "{error}");
    }

    #[test]
    fn the_origin_is_what_a_browser_would_send() {
        for (url, origin, secure) in [
            ("https://wiki.example.org", "https://wiki.example.org", true),
            (
                "https://wiki.example.org/",
                "https://wiki.example.org",
                true,
            ),
            ("HTTPS://Wiki.Example.ORG", "https://wiki.example.org", true),
            (
                "https://wiki.example.org:443",
                "https://wiki.example.org",
                true,
            ),
            (
                "https://wiki.example.org:8443",
                "https://wiki.example.org:8443",
                true,
            ),
            (
                "http://wiki.example.org:80",
                "http://wiki.example.org",
                false,
            ),
            ("http://127.0.0.1:3000", "http://127.0.0.1:3000", false),
            ("http://[::1]:3000", "http://[::1]:3000", false),
        ] {
            let parsed = PublicUrl::parse(url).unwrap();
            assert_eq!(parsed.origin(), origin, "{url}");
            assert_eq!(parsed.secure(), secure, "{url}");
        }
    }

    #[test]
    fn urls_that_cannot_be_an_origin_are_refused() {
        for url in [
            "",
            "wiki.example.org",
            "/path",
            "ftp://wiki.example.org",
            "https://",
            "https://:443",
            "https://wiki.example.org/wiki",
            "https://wiki.example.org/?a=1",
            "https://wiki.example.org/#top",
            "https://user@wiki.example.org",
            "https://wiki.example.org:",
            "https://wiki.example.org:99999",
            "https://wiki.example.org:84a3",
        ] {
            assert!(PublicUrl::parse(url).is_err(), "{url:?}");
        }
    }

    #[test]
    fn only_the_public_origin_passes() {
        let url = PublicUrl::parse("https://wiki.example.org").unwrap();
        assert!(url.allows(&HeaderMap::new()));
        assert!(url.allows(&origin_headers("https://wiki.example.org")));
        for origin in [
            "https://evil.example",
            "http://wiki.example.org",
            "https://wiki.example.org:8443",
            "https://wiki.example.org.evil.example",
            "null",
        ] {
            assert!(!url.allows(&origin_headers(origin)), "{origin}");
        }
    }

    #[test]
    fn defaults_match_the_operations_guide() {
        let config = config(&[]).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("./data"));
        assert_eq!(config.run_as, None);
        assert_eq!(config.bind, "127.0.0.1:3000".parse().unwrap());
        assert_eq!(config.log, "info");
        assert_eq!(config.http.trusted_proxies, TrustedProxies::default());
        assert_eq!(config.collab.flush_interval, Duration::from_millis(500));
        assert_eq!(config.collab.compact_updates, 500);
        assert_eq!(config.collab.room_idle, Duration::from_secs(60));
        assert_eq!(config.collab.room_queue_bytes, 4_194_304);
        assert_eq!(config.collab.room_memory, 72 * 1024 * 1024);
        assert_eq!(config.collab.max_sockets, 512);
        assert_eq!(config.collab.max_frame_bytes, 8_388_608);
        assert_eq!(config.collab.send_queue, 256);
        assert_eq!(config.collab.doc_reads, 2);
        assert_eq!(
            config.jobs.trash_retention,
            Duration::from_secs(30 * 86_400)
        );
        assert_eq!(config.socket.ping, Duration::from_secs(30));
        assert_eq!(config.socket.ping_timeout, Duration::from_secs(90));
    }

    #[test]
    fn every_setting_is_read() {
        let config = config(&[
            ("CADOCS_DATA_DIR", "/var/lib/cadocs"),
            ("CADOCS_BIND", "127.0.0.1:8080"),
            ("CADOCS_LOG", "debug"),
            ("CADOCS_TRUSTED_PROXIES", "10.0.0.1"),
            ("CADOCS_FLUSH_INTERVAL_MS", "100"),
            ("CADOCS_COMPACT_UPDATES", "10"),
            ("CADOCS_ROOM_IDLE_SECS", "5"),
            ("CADOCS_ROOM_QUEUE_BYTES", "1024"),
            ("CADOCS_ROOM_MEMORY_MB", "16"),
            ("CADOCS_MAX_SOCKETS", "3"),
            ("CADOCS_MAX_FRAME_BYTES", "512"),
            ("CADOCS_SEND_QUEUE", "4"),
            ("CADOCS_DOC_READS", "5"),
            ("CADOCS_TRASH_DAYS", "9"),
            ("CADOCS_REVISION_PAUSE_SECS", "11"),
            ("CADOCS_REVISION_MAX_SECS", "12"),
            ("CADOCS_REVISION_KEEP_DAYS", "13"),
            ("CADOCS_PING_SECS", "6"),
            ("CADOCS_PING_TIMEOUT_SECS", "7"),
        ])
        .unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/cadocs"));
        assert_eq!(config.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.log, "debug");
        assert_eq!(config.http.public_url.origin(), "https://wiki.example.org");
        assert_eq!(
            config.http.trusted_proxies,
            TrustedProxies::parse("10.0.0.1").unwrap()
        );
        assert_eq!(config.collab.flush_interval, Duration::from_millis(100));
        assert_eq!(config.collab.compact_updates, 10);
        assert_eq!(config.collab.room_idle, Duration::from_secs(5));
        assert_eq!(config.collab.room_queue_bytes, 1024);
        assert_eq!(config.collab.room_memory, 16 * 1024 * 1024);
        assert_eq!(config.collab.max_sockets, 3);
        assert_eq!(config.collab.max_frame_bytes, 512);
        assert_eq!(config.collab.send_queue, 4);
        assert_eq!(config.collab.doc_reads, 5);
        assert_eq!(config.jobs.trash_retention, Duration::from_secs(9 * 86_400));
        assert_eq!(config.revisions.pause, Duration::from_secs(11));
        assert_eq!(config.revisions.max, Duration::from_secs(12));
        assert_eq!(
            config.jobs.revision_retention,
            Duration::from_secs(13 * 86_400)
        );
        assert_eq!(config.socket.ping, Duration::from_secs(6));
        assert_eq!(config.socket.ping_timeout, Duration::from_secs(7));
    }

    #[test]
    fn no_mail_is_sent_unless_a_relay_is_named() {
        assert_eq!(config(&[]).unwrap().mail, None);
        // The other settings alone turn nothing on, so unsetting the host is the switch.
        let off = config(&[
            ("CADOCS_SMTP_HOST", ""),
            ("CADOCS_SMTP_FROM", "wiki@example.org"),
            ("CADOCS_SMTP_TLS", "none"),
        ]);
        assert_eq!(off.unwrap().mail, None);
    }

    #[test]
    fn a_relay_defaults_to_starttls_and_its_port() {
        let relay = |tls: Option<&str>| {
            let mut vars = vec![
                ("CADOCS_SMTP_HOST", "mail.example.org"),
                ("CADOCS_SMTP_FROM", "Team wiki <wiki@example.org>"),
            ];
            vars.extend(tls.map(|tls| ("CADOCS_SMTP_TLS", tls)));
            config(&vars).unwrap().mail.unwrap()
        };
        let default = relay(None);
        assert_eq!(default.host, "mail.example.org");
        assert_eq!(default.from, "Team wiki <wiki@example.org>");
        assert_eq!(default.credentials, None);
        assert_eq!((default.security, default.port), (Security::StartTls, 587));
        let starttls = relay(Some("starttls"));
        assert_eq!(
            (starttls.security, starttls.port),
            (Security::StartTls, 587)
        );
        let tls = relay(Some("tls"));
        assert_eq!((tls.security, tls.port), (Security::Tls, 465));
        let none = relay(Some("none"));
        assert_eq!((none.security, none.port), (Security::None, 25));
    }

    #[test]
    fn an_empty_mail_setting_is_an_unset_one() {
        // What a compose file passes for a variable it does not define.
        let mail = config(&[
            ("CADOCS_SMTP_HOST", "mail.example.org"),
            ("CADOCS_SMTP_FROM", "wiki@example.org"),
            ("CADOCS_SMTP_TLS", ""),
            ("CADOCS_SMTP_PORT", ""),
            ("CADOCS_SMTP_USERNAME", ""),
            ("CADOCS_SMTP_PASSWORD", ""),
            ("CADOCS_SMTP_PASSWORD_FILE", ""),
        ])
        .unwrap()
        .mail
        .unwrap();
        assert_eq!((mail.security, mail.port), (Security::StartTls, 587));
        assert_eq!(mail.credentials, None);
    }

    #[test]
    fn every_mail_setting_is_read() {
        let mail = config(&[
            ("CADOCS_SMTP_HOST", "10.0.0.5"),
            ("CADOCS_SMTP_PORT", "2525"),
            ("CADOCS_SMTP_TLS", "tls"),
            ("CADOCS_SMTP_USERNAME", "cadocs"),
            ("CADOCS_SMTP_PASSWORD", "s3cret"),
            ("CADOCS_SMTP_FROM", "wiki@example.org"),
        ])
        .unwrap()
        .mail
        .unwrap();
        assert_eq!(mail.host, "10.0.0.5");
        assert_eq!(mail.port, 2525);
        assert_eq!(mail.security, Security::Tls);
        assert_eq!(
            mail.credentials,
            Some(("cadocs".to_owned(), "s3cret".to_owned()))
        );
    }

    #[test]
    fn the_mail_password_can_come_from_a_file() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "s3cret\n").unwrap();
        let path = file.path().to_str().unwrap();
        let relay = [
            ("CADOCS_SMTP_HOST", "mail.example.org"),
            ("CADOCS_SMTP_FROM", "wiki@example.org"),
            ("CADOCS_SMTP_USERNAME", "cadocs"),
        ];
        let mut vars = relay.to_vec();
        vars.push(("CADOCS_SMTP_PASSWORD_FILE", path));
        let mail = config(&vars).unwrap().mail.unwrap();
        assert_eq!(mail.credentials.unwrap().1, "s3cret");

        vars.push(("CADOCS_SMTP_PASSWORD", "other"));
        let error = config(&vars).unwrap_err().to_string();
        assert!(error.contains("not both"), "{error}");
    }

    #[test]
    fn a_mail_setting_that_cannot_work_is_refused_and_named() {
        let host = ("CADOCS_SMTP_HOST", "mail.example.org");
        let from = ("CADOCS_SMTP_FROM", "wiki@example.org");
        for (vars, named) in [
            (vec![host], "CADOCS_SMTP_FROM is required"),
            (
                vec![host, ("CADOCS_SMTP_FROM", "the wiki")],
                "CADOCS_SMTP_FROM",
            ),
            (
                vec![("CADOCS_SMTP_HOST", "smtp://mail.example.org"), from],
                "CADOCS_SMTP_HOST",
            ),
            (
                vec![("CADOCS_SMTP_HOST", "mail.example.org:587"), from],
                "CADOCS_SMTP_HOST",
            ),
            (
                vec![host, from, ("CADOCS_SMTP_TLS", "ssl")],
                "CADOCS_SMTP_TLS",
            ),
            (
                vec![host, from, ("CADOCS_SMTP_PORT", "smtp")],
                "CADOCS_SMTP_PORT",
            ),
            (
                vec![host, from, ("CADOCS_SMTP_PORT", "0")],
                "CADOCS_SMTP_PORT",
            ),
            (
                vec![host, from, ("CADOCS_SMTP_USERNAME", "cadocs")],
                "CADOCS_SMTP_PASSWORD",
            ),
            (
                vec![host, from, ("CADOCS_SMTP_PASSWORD", "s3cret")],
                "CADOCS_SMTP_USERNAME",
            ),
        ] {
            let error = config(&vars).unwrap_err().to_string();
            assert!(error.contains(named), "{vars:?}: {error}");
        }
    }

    #[test]
    fn a_mail_password_never_goes_out_in_clear() {
        let error = config(&[
            ("CADOCS_SMTP_HOST", "mail.example.org"),
            ("CADOCS_SMTP_FROM", "wiki@example.org"),
            ("CADOCS_SMTP_TLS", "none"),
            ("CADOCS_SMTP_USERNAME", "cadocs"),
            ("CADOCS_SMTP_PASSWORD", "s3cret"),
        ])
        .unwrap_err()
        .to_string();
        assert!(error.contains("in clear"), "{error}");
        assert!(!error.contains("s3cret"), "{error}");
    }

    #[test]
    fn root_falls_back_to_a_user_only_when_one_is_set() {
        let set = config(&[("CADOCS_RUN_AS", "65532:65532")]).unwrap();
        let user = RunAs {
            uid: 65532,
            gid: 65532,
        };
        assert_eq!(set.run_as, Some(user));
        assert_eq!(config(&[("CADOCS_RUN_AS", "")]).unwrap().run_as, None);

        let error = config(&[("CADOCS_RUN_AS", "0:0")]).unwrap_err();
        assert!(error.to_string().contains("CADOCS_RUN_AS"), "{error}");
    }

    #[test]
    fn any_user_creates_a_workspace_unless_set_to_admins() {
        let all = config(&[]).unwrap().workspace_creation;
        assert_eq!(all, WorkspaceCreation::All);
        let set = config(&[("CADOCS_WORKSPACE_CREATION", "all")]).unwrap();
        assert_eq!(set.workspace_creation, WorkspaceCreation::All);
        let set = config(&[("CADOCS_WORKSPACE_CREATION", "admins")]).unwrap();
        assert_eq!(set.workspace_creation, WorkspaceCreation::Admins);
        for refused in ["", "Admins", "none"] {
            let error = config(&[("CADOCS_WORKSPACE_CREATION", refused)]).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("CADOCS_WORKSPACE_CREATION"), "{message}");
        }
    }

    const KEYCLOAK: [(&str, &str); 4] = [
        ("CADOCS_OIDC_PROVIDERS", "keycloak"),
        (
            "CADOCS_OIDC_KEYCLOAK_ISSUER",
            "https://sso.example.org/realms/main",
        ),
        ("CADOCS_OIDC_KEYCLOAK_CLIENT_ID", "wiki"),
        ("CADOCS_OIDC_KEYCLOAK_CLIENT_SECRET", "s3cret"),
    ];

    #[test]
    fn no_oidc_provider_unless_one_is_named() {
        assert!(config(&[]).unwrap().oidc.is_empty());
        assert!(
            config(&[("CADOCS_OIDC_PROVIDERS", "")])
                .unwrap()
                .oidc
                .is_empty()
        );
        // Its other settings alone turn nothing on.
        assert!(config(&KEYCLOAK[1..]).unwrap().oidc.is_empty());
    }

    #[test]
    fn every_oidc_setting_is_read() {
        let file = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(file.path(), "other secret\n").unwrap();
        let mut vars = KEYCLOAK.to_vec();
        vars[0].1 = "keycloak, authentik2";
        vars.extend([
            (
                "CADOCS_OIDC_AUTHENTIK2_ISSUER",
                "http://127.0.0.1:9000/application/o/wiki/",
            ),
            ("CADOCS_OIDC_AUTHENTIK2_CLIENT_ID", "abc"),
            (
                "CADOCS_OIDC_AUTHENTIK2_CLIENT_SECRET_FILE",
                file.path().to_str().unwrap(),
            ),
        ]);
        let providers = config(&vars).unwrap().oidc;
        // The secret never reaches a log through `Debug`.
        assert!(
            !format!("{providers:?}").contains("s3cret"),
            "{providers:?}"
        );
        let read: Vec<_> = providers
            .iter()
            .map(|p| (&*p.id, &*p.issuer, &*p.client_id, &*p.client_secret))
            .collect();
        assert_eq!(
            read,
            [
                (
                    "keycloak",
                    "https://sso.example.org/realms/main",
                    "wiki",
                    "s3cret"
                ),
                (
                    "authentik2",
                    "http://127.0.0.1:9000/application/o/wiki/",
                    "abc",
                    "other secret"
                ),
            ]
        );
    }

    #[test]
    fn an_oidc_setting_that_cannot_work_is_refused_and_named() {
        let with = |name: &'static str, value: &'static str| {
            let mut vars = KEYCLOAK.to_vec();
            vars.retain(|(key, _)| *key != name);
            if !value.is_empty() {
                vars.push((name, value));
            }
            config(&vars).unwrap_err().to_string()
        };
        for (name, value, named) in [
            ("CADOCS_OIDC_PROVIDERS", "Keycloak", "CADOCS_OIDC_PROVIDERS"),
            (
                "CADOCS_OIDC_PROVIDERS",
                "key-cloak",
                "CADOCS_OIDC_PROVIDERS",
            ),
            (
                "CADOCS_OIDC_PROVIDERS",
                "keycloak,,",
                "CADOCS_OIDC_PROVIDERS",
            ),
            ("CADOCS_OIDC_PROVIDERS", "keycloak,keycloak", "twice"),
            (
                "CADOCS_OIDC_KEYCLOAK_ISSUER",
                "",
                "CADOCS_OIDC_KEYCLOAK_ISSUER is required",
            ),
            (
                "CADOCS_OIDC_KEYCLOAK_ISSUER",
                "sso.example.org",
                "CADOCS_OIDC_KEYCLOAK_ISSUER",
            ),
            // A code and a token in clear, unless they never leave this machine.
            (
                "CADOCS_OIDC_KEYCLOAK_ISSUER",
                "http://sso.example.org",
                "https",
            ),
            (
                "CADOCS_OIDC_KEYCLOAK_ISSUER",
                "https://sso.example.org/?a=b",
                "query",
            ),
            (
                "CADOCS_OIDC_KEYCLOAK_CLIENT_ID",
                "",
                "CADOCS_OIDC_KEYCLOAK_CLIENT_ID is required",
            ),
            (
                "CADOCS_OIDC_KEYCLOAK_CLIENT_SECRET",
                "",
                "CADOCS_OIDC_KEYCLOAK_CLIENT_SECRET",
            ),
        ] {
            let error = with(name, value);
            assert!(error.contains(named), "{name}={value:?}: {error}");
        }
    }

    #[test]
    fn registration_is_open_unless_closed() {
        assert_eq!(config(&[]).unwrap().registration, Registration::Open);
        let set = config(&[("CADOCS_REGISTRATION", "open")]).unwrap();
        assert_eq!(set.registration, Registration::Open);
        let set = config(&[("CADOCS_REGISTRATION", "closed")]).unwrap();
        assert_eq!(set.registration, Registration::Closed);
        for refused in ["", "Closed", "off"] {
            let error = config(&[("CADOCS_REGISTRATION", refused)]).unwrap_err();
            let message = error.to_string();
            assert!(message.contains("CADOCS_REGISTRATION"), "{message}");
        }
    }

    #[test]
    fn revision_timing_is_checked() {
        let defaults = config(&[]).unwrap();
        assert_eq!(defaults.revisions.pause, Duration::from_secs(120));
        assert_eq!(defaults.revisions.max, Duration::from_secs(600));
        assert_eq!(
            defaults.jobs.revision_retention,
            Duration::from_secs(30 * 86_400)
        );
        let at_once = config(&[
            ("CADOCS_REVISION_PAUSE_SECS", "0"),
            ("CADOCS_REVISION_MAX_SECS", "0"),
        ]);
        assert!(at_once.is_ok(), "a revision after each flush is allowed");
        let huge = u64::MAX.to_string();
        for refused in [
            &[
                ("CADOCS_REVISION_PAUSE_SECS", "60"),
                ("CADOCS_REVISION_MAX_SECS", "30"),
            ][..],
            &[("CADOCS_REVISION_KEEP_DAYS", "0")],
            &[("CADOCS_REVISION_MAX_SECS", huge.as_str())],
        ] {
            let error = config(refused).unwrap_err();
            assert!(error.to_string().contains("CADOCS_REVISION"), "{error}");
        }
    }

    #[test]
    fn the_room_memory_budget_is_at_least_a_megabyte() {
        for refused in ["0", "1048577"] {
            let error = config(&[("CADOCS_ROOM_MEMORY_MB", refused)]).unwrap_err();
            assert!(
                format!("{error:#}").contains("CADOCS_ROOM_MEMORY_MB"),
                "{error:#}"
            );
        }
    }

    #[test]
    fn a_retired_setting_is_ignored_and_named() {
        let old = config(&[("CADOCS_MAX_ROOMS", "many")]).unwrap();
        assert_eq!(old.ignored, ["CADOCS_MAX_ROOMS"]);
        assert!(config(&[]).unwrap().ignored.is_empty());
    }

    #[test]
    fn an_invalid_value_names_its_variable() {
        let error = config(&[("CADOCS_MAX_SOCKETS", "many")]).unwrap_err();
        assert!(error.to_string().contains("CADOCS_MAX_SOCKETS"), "{error}");
        let error = config(&[("CADOCS_TRUSTED_PROXIES", "many")]).unwrap_err();
        assert!(
            error.to_string().contains("CADOCS_TRUSTED_PROXIES"),
            "{error}"
        );
    }

    #[test]
    fn values_too_large_for_timers_and_channels_are_refused() {
        let huge = u64::MAX.to_string();
        for name in [
            "CADOCS_FLUSH_INTERVAL_MS",
            "CADOCS_ROOM_IDLE_SECS",
            "CADOCS_PING_SECS",
            "CADOCS_PING_TIMEOUT_SECS",
            "CADOCS_MAX_SOCKETS",
            "CADOCS_SEND_QUEUE",
            "CADOCS_DOC_READS",
            "CADOCS_TRASH_DAYS",
        ] {
            assert!(config(&[(name, &huge)]).is_err(), "{name}");
        }
    }

    #[test]
    fn values_that_would_panic_or_kill_every_socket_are_refused() {
        assert!(config(&[("CADOCS_PING_SECS", "0")]).is_err());
        assert!(config(&[("CADOCS_SOCKET_RECHECK_SECS", "0")]).is_err());
        assert!(config(&[("CADOCS_SOCKET_RECHECK_SECS", "18446744073709551615")]).is_err());
        assert!(config(&[("CADOCS_FLUSH_INTERVAL_MS", "0")]).is_err());
        assert!(config(&[("CADOCS_SEND_QUEUE", "0")]).is_err());
        assert!(config(&[("CADOCS_DOC_READS", "0")]).is_err());
        assert!(config(&[("CADOCS_TRASH_DAYS", "0")]).is_err());
        assert!(
            config(&[
                ("CADOCS_PING_SECS", "30"),
                ("CADOCS_PING_TIMEOUT_SECS", "30")
            ])
            .is_err()
        );
    }

    #[test]
    fn attachments_stay_on_disk_unless_s3_is_chosen() {
        assert!(matches!(config(&[]).unwrap().files, FileStoreConfig::Local));
        assert!(matches!(
            config(&[("CADOCS_FILE_STORE", "local")]).unwrap().files,
            FileStoreConfig::Local
        ));
        let error = config(&[("CADOCS_FILE_STORE", "nfs")]).unwrap_err();
        assert!(
            format!("{error:#}").contains("CADOCS_FILE_STORE"),
            "{error:#}"
        );
    }

    #[cfg(not(feature = "s3"))]
    #[test]
    fn a_build_without_s3_says_which_feature_it_lacks() {
        let error = config(&[("CADOCS_FILE_STORE", "s3")]).unwrap_err();
        assert!(format!("{error:#}").contains("--features s3"), "{error:#}");
    }

    #[cfg(feature = "s3")]
    mod s3 {
        use cadocs_files::{Downloads, S3Config};

        use super::*;

        const REQUIRED: [(&str, &str); 5] = [
            ("CADOCS_FILE_STORE", "s3"),
            ("CADOCS_S3_ENDPOINT", "https://s3.example.org"),
            ("CADOCS_S3_BUCKET", "wiki"),
            ("CADOCS_S3_ACCESS_KEY_ID", "cadocs"),
            ("CADOCS_S3_SECRET_ACCESS_KEY", "secret"),
        ];

        fn read_s3(vars: &[(&str, &str)]) -> anyhow::Result<S3Config> {
            match config(&[vars, &REQUIRED[..]].concat())?.files {
                FileStoreConfig::S3(s3) => Ok(s3),
                FileStoreConfig::Local => panic!("not S3"),
            }
        }

        #[test]
        fn s3_needs_its_endpoint_bucket_and_keys_and_defaults_the_rest() {
            let s3 = read_s3(&[]).unwrap();
            assert_eq!(s3.endpoint, "https://s3.example.org");
            assert_eq!(s3.bucket, "wiki");
            assert_eq!(s3.access_key_id, "cadocs");
            assert_eq!(s3.secret_access_key, "secret");
            assert_eq!(s3.region, "us-east-1");
            assert_eq!(s3.prefix, "");
            assert_eq!(s3.downloads, Downloads::Proxy);
            assert_eq!(s3.link_lifetime, Duration::from_secs(3600));
            assert_eq!(s3.ca_file, None);
            // The secret never reaches a log through `Debug`.
            assert!(!format!("{s3:?}").contains("secret"), "{s3:?}");

            for (missing, _) in &REQUIRED[1..] {
                let vars: Vec<_> = REQUIRED
                    .iter()
                    .filter(|(k, _)| k != missing)
                    .copied()
                    .collect();
                let error = config(&vars).unwrap_err();
                assert!(format!("{error:#}").contains(missing), "{error:#}");
            }
        }

        #[test]
        fn every_s3_setting_is_read() {
            let s3 = read_s3(&[
                ("CADOCS_S3_REGION", "garage"),
                ("CADOCS_S3_PREFIX", "cadocs/"),
                ("CADOCS_S3_DOWNLOADS", "redirect"),
                ("CADOCS_S3_LINK_SECS", "600"),
                ("CADOCS_S3_CA_FILE", "/etc/ssl/internal.pem"),
            ])
            .unwrap();
            assert_eq!(s3.region, "garage");
            assert_eq!(s3.prefix, "cadocs/");
            assert_eq!(s3.downloads, Downloads::Redirect);
            assert_eq!(s3.link_lifetime, Duration::from_secs(600));
            assert_eq!(s3.ca_file, Some(PathBuf::from("/etc/ssl/internal.pem")));

            for (name, value) in [
                ("CADOCS_S3_DOWNLOADS", "direct"),
                ("CADOCS_S3_LINK_SECS", "1"),
                ("CADOCS_S3_LINK_SECS", "604801"),
                ("CADOCS_S3_ENDPOINT", "s3.example.org"),
                ("CADOCS_S3_PREFIX", "/cadocs"),
                ("CADOCS_S3_PREFIX", "a/../b"),
            ] {
                let error = read_s3(&[(name, value)]).unwrap_err();
                assert!(
                    format!("{error:#}").contains(name),
                    "{name}={value}: {error:#}"
                );
            }
        }

        #[test]
        fn the_secret_can_come_from_a_file() {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("secret");
            std::fs::write(&path, "from-file\n").unwrap();
            let path = path.to_str().unwrap();
            let vars: Vec<_> = REQUIRED[..4]
                .iter()
                .copied()
                .chain([("CADOCS_S3_SECRET_ACCESS_KEY_FILE", path)])
                .collect();
            match config(&vars).unwrap().files {
                FileStoreConfig::S3(s3) => assert_eq!(s3.secret_access_key, "from-file"),
                FileStoreConfig::Local => panic!("not S3"),
            }

            let both = read_s3(&[("CADOCS_S3_SECRET_ACCESS_KEY_FILE", path)]).unwrap_err();
            assert!(format!("{both:#}").contains("_FILE"), "{both:#}");
        }
    }
}
