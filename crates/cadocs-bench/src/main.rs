//! `cadocs-bench seed` fills a database, `cadocs-bench load` runs the simulated editors,
//! `latency` and `logins` time requests. Defaults are the benchmark scenario of
//! `docs/ARCHITECTURE.md`, and `bench/rss.sh` drives them.

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use cadocs_bench::http::http_origin;
use cadocs_bench::load::{self, LoadConfig};
use cadocs_bench::login::{LoginConfig, login_peak};
use cadocs_bench::probe::{Latency, LatencyConfig, latencies};
use cadocs_bench::seed::{Seed, seed};
use cadocs_store::SqliteStore;
use tokio::process::Command;

const USAGE: &str = "usage:
  cadocs-bench seed --db <file> --docs <n> [--pages <n>] [--users <n>] [--fanout <n>]
                    [--index true] [--seed <n>] [--keep-log true]
  cadocs-bench load --url <ws://host:port> --docs <n> [--clients-per-doc <n>] [--users <n>]
                    [--events true] [--updates-per-sec <n>] [--awareness-per-sec <n>]
                    [--duration-secs <n>] [--hold-secs <n>] [--then <shell command>]
  cadocs-bench latency --url <ws://host:port> --pages <n> --fanout <n> [--samples <n>]
                    [--seed <n>]
  cadocs-bench logins --url <ws://host:port> --users <n> [--origin <url>] [--forwarded true]";

struct Args(HashMap<String, String>);

impl Args {
    fn parse(args: impl Iterator<Item = String>) -> anyhow::Result<Self> {
        let mut values = HashMap::new();
        let mut args = args;
        while let Some(flag) = args.next() {
            let Some(name) = flag.strip_prefix("--") else {
                bail!("unexpected argument {flag}\n{USAGE}");
            };
            let value = args
                .next()
                .with_context(|| format!("{flag} needs a value"))?;
            values.insert(name.to_owned(), value);
        }
        Ok(Self(values))
    }

    fn text(&self, name: &str) -> anyhow::Result<&str> {
        self.0
            .get(name)
            .map(String::as_str)
            .with_context(|| format!("--{name} is required\n{USAGE}"))
    }

    fn number<T: FromStr>(&self, name: &str, default: Option<T>) -> anyhow::Result<T> {
        match (self.0.get(name), default) {
            (Some(value), _) => value
                .parse()
                .ok()
                .with_context(|| format!("--{name}: invalid value {value}")),
            (None, Some(default)) => Ok(default),
            (None, None) => bail!("--{name} is required\n{USAGE}"),
        }
    }
}

async fn run_seed(args: &Args) -> anyhow::Result<()> {
    let store = SqliteStore::open(Path::new(args.text("db")?)).await?;
    let docs = args.number("docs", None)?;
    let defaults = Seed::docs(docs);
    let config = Seed {
        pages: args.number("pages", Some(docs))?,
        users: args.number("users", Some(defaults.users))?,
        fanout: args
            .0
            .get("fanout")
            .map(|_| args.number("fanout", None))
            .transpose()?,
        index: args.number("index", Some(false))?,
        seed: args.number("seed", Some(defaults.seed))?,
        compact_updates: (!args.number("keep-log", Some(false))?)
            .then_some(defaults.compact_updates)
            .flatten(),
        ..defaults
    };
    seed(&store, &config).await
}

async fn run_load(args: &Args) -> anyhow::Result<()> {
    let config = LoadConfig {
        url: args.text("url")?.to_owned(),
        docs: args.number("docs", None)?,
        clients_per_doc: args.number("clients-per-doc", Some(2))?,
        users: args.number("users", Some(1))?,
        events: args.number("events", Some(false))?,
        updates_per_sec: args.number("updates-per-sec", Some(2))?,
        awareness_per_sec: args.number("awareness-per-sec", Some(1))?,
        duration: Duration::from_secs(args.number("duration-secs", Some(60))?),
        hold: Duration::from_secs(args.number("hold-secs", Some(30))?),
    };
    let then = args.0.get("then").cloned();
    let report = load::run(&config, async move {
        if let Some(command) = then {
            let status = Command::new("sh").arg("-c").arg(&command).status().await?;
            ensure!(status.success(), "`{command}` failed: {status}");
        }
        Ok(())
    })
    .await?;
    eprintln!("{report:?}");
    Ok(())
}

fn millis(duration: std::time::Duration) -> f64 {
    duration.as_secs_f64() * 1_000.0
}

/// One line per request kind, in milliseconds: `latency/<name> samples p50 p95 max`.
fn print_latency(latency: &Latency) {
    println!(
        "{:<24} {:>8} {:>8.1} {:>8.1} {:>8.1}",
        format!("latency/{}", latency.name),
        latency.samples.len(),
        millis(latency.quantile(0.5)),
        millis(latency.quantile(0.95)),
        millis(latency.max()),
    );
}

async fn run_latency(args: &Args) -> anyhow::Result<()> {
    let config = LatencyConfig {
        url: args.text("url")?.to_owned(),
        pages: args.number("pages", None)?,
        fanout: args.number("fanout", None)?,
        samples: args.number("samples", Some(200))?,
        seed: args.number("seed", Some(1))?,
    };
    println!(
        "{:<24} {:>8} {:>8} {:>8} {:>8}",
        "request", "samples", "p50 ms", "p95 ms", "max ms"
    );
    for latency in latencies(&config).await? {
        print_latency(&latency);
    }
    Ok(())
}

async fn run_logins(args: &Args) -> anyhow::Result<()> {
    let url = args.text("url")?.to_owned();
    let config = LoginConfig {
        origin: args
            .0
            .get("origin")
            .cloned()
            .unwrap_or_else(|| http_origin(&url)),
        url,
        users: args.number("users", None)?,
        forwarded: args.number("forwarded", Some(false))?,
    };
    let report = login_peak(&config).await?;
    let latency = Latency {
        name: "login",
        samples: report.latencies.clone(),
    };
    println!(
        "logins: {} ok, {} busy (503), {} refused (429)",
        report.ok, report.busy, report.refused
    );
    // A refused login is a result to record, not a failed run.
    print_latency(&latency);
    Ok(())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let args = Args::parse(args)?;
    match command.as_str() {
        "seed" => run_seed(&args).await,
        "load" => run_load(&args).await,
        "latency" => run_latency(&args).await,
        "logins" => run_logins(&args).await,
        _ => bail!("{USAGE}"),
    }
}
