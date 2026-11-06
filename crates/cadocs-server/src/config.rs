//! Settings from the environment, as listed in `docs/OPERATIONS.md`.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, ensure};
use cadocs_collab::CollabConfig;

/// Far above any sensible value, and small enough for timers and semaphores not to overflow.
const MAX_SECS: u64 = 365 * 24 * 60 * 60;
const MAX_COUNT: usize = 1 << 24;

#[derive(Debug, Clone)]
pub struct Config {
    pub data_dir: PathBuf,
    pub bind: SocketAddr,
    pub log: String,
    pub collab: CollabConfig,
    pub socket: SocketConfig,
}

/// Keep-alive of a socket.
#[derive(Debug, Clone, Copy)]
pub struct SocketConfig {
    pub ping: Duration,
    /// Silence before a socket closes. Also bounds how long one send may take.
    pub ping_timeout: Duration,
}

impl Default for SocketConfig {
    fn default() -> Self {
        Self {
            ping: Duration::from_secs(30),
            ping_timeout: Duration::from_secs(90),
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
            },
            socket: SocketConfig {
                ping: read.secs("CADOCS_PING_SECS", socket.ping)?,
                ping_timeout: read.secs("CADOCS_PING_TIMEOUT_SECS", socket.ping_timeout)?,
            },
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
            !self.socket.ping.is_zero(),
            "CADOCS_PING_SECS must not be 0"
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

    fn secs(&self, name: &str, default: Duration) -> anyhow::Result<Duration> {
        self.value(name, default.as_secs()).map(Duration::from_secs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config(vars: &[(&str, &str)]) -> anyhow::Result<Config> {
        Config::from_lookup(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| value.to_string())
        })
    }

    #[test]
    fn defaults_match_the_operations_guide() {
        let config = config(&[]).unwrap();
        assert_eq!(config.data_dir, PathBuf::from("./data"));
        assert_eq!(config.bind, "127.0.0.1:3000".parse().unwrap());
        assert_eq!(config.log, "info");
        assert_eq!(config.collab.flush_interval, Duration::from_millis(500));
        assert_eq!(config.collab.compact_updates, 500);
        assert_eq!(config.collab.room_idle, Duration::from_secs(60));
        assert_eq!(config.collab.room_queue_bytes, 4_194_304);
        assert_eq!(config.collab.max_rooms, 256);
        assert_eq!(config.collab.max_sockets, 512);
        assert_eq!(config.collab.max_frame_bytes, 8_388_608);
        assert_eq!(config.collab.send_queue, 256);
        assert_eq!(config.socket.ping, Duration::from_secs(30));
        assert_eq!(config.socket.ping_timeout, Duration::from_secs(90));
    }

    #[test]
    fn every_setting_is_read() {
        let config = config(&[
            ("CADOCS_DATA_DIR", "/var/lib/cadocs"),
            ("CADOCS_BIND", "127.0.0.1:8080"),
            ("CADOCS_LOG", "debug"),
            ("CADOCS_FLUSH_INTERVAL_MS", "100"),
            ("CADOCS_COMPACT_UPDATES", "10"),
            ("CADOCS_ROOM_IDLE_SECS", "5"),
            ("CADOCS_ROOM_QUEUE_BYTES", "1024"),
            ("CADOCS_MAX_ROOMS", "2"),
            ("CADOCS_MAX_SOCKETS", "3"),
            ("CADOCS_MAX_FRAME_BYTES", "512"),
            ("CADOCS_SEND_QUEUE", "4"),
            ("CADOCS_PING_SECS", "6"),
            ("CADOCS_PING_TIMEOUT_SECS", "7"),
        ])
        .unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/var/lib/cadocs"));
        assert_eq!(config.bind, "127.0.0.1:8080".parse().unwrap());
        assert_eq!(config.log, "debug");
        assert_eq!(config.collab.flush_interval, Duration::from_millis(100));
        assert_eq!(config.collab.compact_updates, 10);
        assert_eq!(config.collab.room_idle, Duration::from_secs(5));
        assert_eq!(config.collab.room_queue_bytes, 1024);
        assert_eq!(config.collab.max_rooms, 2);
        assert_eq!(config.collab.max_sockets, 3);
        assert_eq!(config.collab.max_frame_bytes, 512);
        assert_eq!(config.collab.send_queue, 4);
        assert_eq!(config.socket.ping, Duration::from_secs(6));
        assert_eq!(config.socket.ping_timeout, Duration::from_secs(7));
    }

    #[test]
    fn an_invalid_value_names_its_variable() {
        let error = config(&[("CADOCS_MAX_ROOMS", "many")]).unwrap_err();
        assert!(error.to_string().contains("CADOCS_MAX_ROOMS"), "{error}");
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
        ] {
            assert!(config(&[(name, &huge)]).is_err(), "{name}");
        }
    }

    #[test]
    fn values_that_would_panic_or_kill_every_socket_are_refused() {
        assert!(config(&[("CADOCS_PING_SECS", "0")]).is_err());
        assert!(config(&[("CADOCS_FLUSH_INTERVAL_MS", "0")]).is_err());
        assert!(config(&[("CADOCS_SEND_QUEUE", "0")]).is_err());
        assert!(
            config(&[
                ("CADOCS_PING_SECS", "30"),
                ("CADOCS_PING_TIMEOUT_SECS", "30")
            ])
            .is_err()
        );
    }
}
