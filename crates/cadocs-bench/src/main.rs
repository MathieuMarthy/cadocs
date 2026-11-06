//! `cadocs-bench seed` fills a database, `cadocs-bench load` runs the simulated editors. Defaults
//! are the benchmark scenario of `docs/ARCHITECTURE.md`, and `bench/rss.sh` drives both.

use std::collections::HashMap;
use std::path::Path;
use std::str::FromStr;
use std::time::Duration;

use anyhow::{Context, bail, ensure};
use cadocs_bench::load::{self, LoadConfig};
use cadocs_bench::seed::{COMPACT_UPDATES, seed};
use cadocs_store::SqliteStore;
use tokio::process::Command;

const USAGE: &str = "usage:
  cadocs-bench seed --db <file> --docs <n> [--seed <n>] [--keep-log true]
  cadocs-bench load --url <ws://host:port> --docs <n> [--clients-per-doc <n>]
                    [--updates-per-sec <n>] [--awareness-per-sec <n>]
                    [--duration-secs <n>] [--hold-secs <n>] [--then <shell command>]";

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
    let compact = (!args.number("keep-log", Some(false))?).then_some(COMPACT_UPDATES);
    seed(&store, docs, args.number("seed", Some(1))?, compact).await
}

async fn run_load(args: &Args) -> anyhow::Result<()> {
    let config = LoadConfig {
        url: args.text("url")?.to_owned(),
        docs: args.number("docs", None)?,
        clients_per_doc: args.number("clients-per-doc", Some(2))?,
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let command = args.next().unwrap_or_default();
    let args = Args::parse(args)?;
    match command.as_str() {
        "seed" => run_seed(&args).await,
        "load" => run_load(&args).await,
        _ => bail!("{USAGE}"),
    }
}
