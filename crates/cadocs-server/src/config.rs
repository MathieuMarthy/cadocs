//! Settings from the environment, as listed in `docs/OPERATIONS.md`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, ensure};
use axum::http::{HeaderMap, Uri, header};
use cadocs_collab::CollabConfig;
use cadocs_jobs::JobConfig;
use cadocs_store::RevisionTiming;

use crate::client_ip::TrustedProxies;

/// Far above any sensible value, and small enough for timers and semaphores not to overflow.
const MAX_SECS: u64 = 365 * 24 * 60 * 60;
const MAX_COUNT: usize = 1 << 24;
const SECS_PER_DAY: u64 = 24 * 60 * 60;
const DEFAULT_SESSION_DAYS: u64 = 30;
const MAX_SESSION_DAYS: u64 = 3650;
const DEFAULT_TRASH_DAYS: u64 = 30;
const DEFAULT_REVISION_DAYS: u64 = 30;
const DEFAULT_MAX_UPLOAD_MB: u64 = 25;
/// 1 TiB: far past any wiki, and still far from overflowing once in bytes.
const MAX_MB: u64 = 1024 * 1024;
const MIB: u64 = 1024 * 1024;
const MAX_TRASH_DAYS: u64 = 3650;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    pub log: String,
    pub http: HttpConfig,
    pub collab: CollabConfig,
    pub socket: SocketConfig,
    pub jobs: JobConfig,
    /// When an edit's revision is taken.
    pub revisions: RevisionTiming,
    pub attachments: AttachmentLimits,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AttachmentLimits {
    pub max_upload_bytes: u64,
    /// What each workspace's attachments may take, `None` for no limit.
    pub workspace_quota_bytes: Option<u64>,
}

impl Default for AttachmentLimits {
    fn default() -> Self {
        Self {
            max_upload_bytes: DEFAULT_MAX_UPLOAD_MB * MIB,
            workspace_quota_bytes: None,
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
                max_rooms: read.value("CADOCS_MAX_ROOMS", collab.max_rooms)?,
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
        Ok(AttachmentLimits {
            max_upload_bytes: upload * MIB,
            workspace_quota_bytes: (quota > 0).then_some(quota * MIB),
        })
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

        let set = config(&[
            ("CADOCS_MAX_UPLOAD_MB", "2"),
            ("CADOCS_WORKSPACE_QUOTA_MB", "10"),
        ]);
        let limits = set.unwrap().attachments;
        assert_eq!(limits.max_upload_bytes, 2 * 1024 * 1024);
        assert_eq!(limits.workspace_quota_bytes, Some(10 * 1024 * 1024));

        for (name, refused) in [
            ("CADOCS_MAX_UPLOAD_MB", "0"),
            ("CADOCS_MAX_UPLOAD_MB", "1048577"),
            ("CADOCS_WORKSPACE_QUOTA_MB", "1048577"),
            ("CADOCS_WORKSPACE_QUOTA_MB", "-1"),
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
        assert_eq!(config.bind, "127.0.0.1:3000".parse().unwrap());
        assert_eq!(config.log, "info");
        assert_eq!(config.http.trusted_proxies, TrustedProxies::default());
        assert_eq!(config.collab.flush_interval, Duration::from_millis(500));
        assert_eq!(config.collab.compact_updates, 500);
        assert_eq!(config.collab.room_idle, Duration::from_secs(60));
        assert_eq!(config.collab.room_queue_bytes, 4_194_304);
        assert_eq!(config.collab.max_rooms, 256);
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
            ("CADOCS_MAX_ROOMS", "2"),
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
        assert_eq!(config.collab.max_rooms, 2);
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
    fn an_invalid_value_names_its_variable() {
        let error = config(&[("CADOCS_MAX_ROOMS", "many")]).unwrap_err();
        assert!(error.to_string().contains("CADOCS_MAX_ROOMS"), "{error}");
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
}
