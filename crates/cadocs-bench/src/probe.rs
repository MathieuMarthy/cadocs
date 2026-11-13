//! Latencies of what one user does on a seeded tree: read a page, open it, browse the tree,
//! search. Requests go one at a time, so each sample is the server's own time on an idle machine.

use std::time::Duration;

use anyhow::ensure;
use tokio::time::Instant;

use crate::http::{Response, request};
use crate::load::Client;
use crate::rng::Rng;
use crate::seed::{SPACE, WORKSPACE, doc_id, session_token};

/// In every benchmark document, so a search for it matches every page.
pub const COMMON_WORD: &str = "server";
/// Apart from the load clients and the authors of the documents.
const PROBE_CLIENTS: u64 = 1 << 40;

#[derive(Debug, Clone)]
pub struct LatencyConfig {
    pub url: String,
    /// Seeded pages, each holding a document.
    pub pages: usize,
    /// The fanout of the seeded tree.
    pub fanout: usize,
    /// Per request kind. Opening takes distinct pages, so at most `pages`.
    pub samples: usize,
    /// The seed of the documents, which numbers their titles.
    pub seed: u64,
}

#[derive(Debug, Clone)]
pub struct Latency {
    pub name: &'static str,
    pub samples: Vec<Duration>,
}

impl Latency {
    /// Nearest rank: the smallest sample with at least `q` of them at or below it.
    pub fn quantile(&self, q: f64) -> Duration {
        let mut sorted = self.samples.clone();
        sorted.sort();
        let rank = (q * sorted.len() as f64).ceil() as usize;
        sorted[rank.clamp(1, sorted.len()) - 1]
    }

    pub fn max(&self) -> Duration {
        self.samples.iter().copied().max().unwrap_or_default()
    }
}

/// `count` distinct indexes below `bound`, in a random order.
fn distinct(rng: &mut Rng, bound: usize, count: usize) -> Vec<usize> {
    let mut all: Vec<usize> = (0..bound).collect();
    for i in (1..all.len()).rev() {
        all.swap(i, rng.below(i + 1));
    }
    all.truncate(count);
    all
}

async fn get(url: &str, path: &str) -> anyhow::Result<(Duration, Response)> {
    let cookie = format!("session={}", session_token(0));
    let started = Instant::now();
    let response = request(url, "GET", path, &[("cookie", &cookie)], b"").await?;
    let elapsed = started.elapsed();
    ensure!(response.status == 200, "GET {path}: {}", response.status);
    Ok((elapsed, response))
}

async fn time(url: &str, path: &str) -> anyhow::Result<Duration> {
    Ok(get(url, path).await?.0)
}

/// Searches for the whole words of `q`, and counts the results. A trailing space ends the last
/// word, which would otherwise be a prefix, as for someone still typing.
pub async fn search(url: &str, q: &str) -> anyhow::Result<(Duration, usize)> {
    let path = format!("/api/v1/workspaces/{WORKSPACE}/search?q={q}%20");
    let (elapsed, response) = get(url, &path).await?;
    // Each result has one `space_id`; the JSON body is sent whole, with its length.
    let body = String::from_utf8_lossy(&response.body);
    Ok((elapsed, body.matches("\"space_id\"").count()))
}

/// Each request kind in turn, `samples` times, one request at a time.
pub async fn latencies(config: &LatencyConfig) -> anyhow::Result<Vec<Latency>> {
    ensure!(config.samples > 0, "no samples");
    ensure!(
        config.samples <= config.pages,
        "more samples than pages to open"
    );
    ensure!(config.fanout >= 2, "a fanout of at least 2");
    // A page below this has children: its first child, `(page + 1) * fanout`, is seeded.
    let parents = (config.pages - 1) / config.fanout;
    ensure!(parents > 0, "the tree has no nested page");
    let mut rng = Rng::new(config.seed);
    let url = config.url.as_str();
    let mut measured = Vec::new();
    let mut measure = |name, samples| measured.push(Latency { name, samples });

    let mut samples = Vec::new();
    for _ in 0..config.samples {
        let page = doc_id(rng.below(config.pages));
        samples.push(time(url, &format!("/api/v1/pages/{page}")).await?);
    }
    measure("page", samples);

    // Distinct pages, so that each one loads its room from the database.
    let mut samples = Vec::new();
    for (sample, page) in distinct(&mut rng, config.pages, config.samples)
        .into_iter()
        .enumerate()
    {
        let started = Instant::now();
        let client = Client::connect(url, page, PROBE_CLIENTS + sample as u64, 0).await?;
        samples.push(started.elapsed());
        client.close().await;
    }
    measure("open", samples);

    let mut samples = Vec::new();
    for _ in 0..config.samples {
        samples.push(time(url, &format!("/api/v1/spaces/{SPACE}/tree")).await?);
    }
    measure("tree-root", samples);

    let mut samples = Vec::new();
    for _ in 0..config.samples {
        let parent = doc_id(rng.below(parents));
        let path = format!("/api/v1/spaces/{SPACE}/tree?parent={parent}");
        samples.push(time(url, &path).await?);
    }
    measure("tree-children", samples);

    let mut samples = Vec::new();
    for _ in 0..config.samples {
        let (elapsed, hits) = search(url, COMMON_WORD).await?;
        ensure!(hits > 1, "{COMMON_WORD} matched {hits} pages");
        samples.push(elapsed);
    }
    measure("search-common", samples);

    // A title number: one page holds it.
    let mut samples = Vec::new();
    for _ in 0..config.samples {
        let number = config.seed + rng.below(config.pages) as u64;
        let (elapsed, hits) = search(url, &number.to_string()).await?;
        ensure!(hits == 1, "{number} matched {hits} pages");
        samples.push(elapsed);
    }
    measure("search-rare", samples);
    Ok(measured)
}
