//! A login peak: every seeded user logs in at the same moment.

use std::time::Duration;

use anyhow::bail;
use tokio::task::JoinSet;
use tokio::time::Instant;

use crate::http::request;
use crate::seed::{BENCH_PASSWORD, user_email};

#[derive(Debug, Clone)]
pub struct LoginConfig {
    pub url: String,
    /// The server's `CADOCS_PUBLIC_URL`, which a login must come from.
    pub origin: String,
    pub users: usize,
    /// Each user comes from its own address in `X-Forwarded-For`, as behind a proxy listed in
    /// `CADOCS_TRUSTED_PROXIES`. Otherwise all share the guess limit of one address.
    pub forwarded: bool,
}

#[derive(Debug, Default, Clone)]
pub struct LoginReport {
    pub ok: usize,
    /// `503`: the hash waited too long for its turn.
    pub busy: usize,
    /// `429`: the address used up its password guesses.
    pub refused: usize,
    pub latencies: Vec<Duration>,
}

fn address(user: usize) -> String {
    format!("10.{}.{}.{}", (user >> 16) & 255, (user >> 8) & 255, user & 255)
}

async fn login(config: LoginConfig, user: usize) -> anyhow::Result<(u16, Duration)> {
    let body = format!(
        r#"{{"email":"{}","password":"{BENCH_PASSWORD}"}}"#,
        user_email(user)
    );
    let forwarded = address(user);
    let mut headers = vec![
        ("content-type", "application/json"),
        ("origin", config.origin.as_str()),
    ];
    if config.forwarded {
        headers.push(("x-forwarded-for", &forwarded));
    }
    let started = Instant::now();
    let response = request(
        &config.url,
        "POST",
        "/api/v1/auth/login",
        &headers,
        body.as_bytes(),
    )
    .await?;
    Ok((response.status, started.elapsed()))
}

/// Sends every login at once and waits for all the answers. Any answer other than a success,
/// `429` or `503` fails the peak: a wrong password would measure nothing.
pub async fn login_peak(config: &LoginConfig) -> anyhow::Result<LoginReport> {
    let mut logins = JoinSet::new();
    for user in 0..config.users {
        logins.spawn(login(config.clone(), user));
    }
    let mut report = LoginReport::default();
    while let Some(answer) = logins.join_next().await {
        let (status, latency) = answer??;
        match status {
            200 => report.ok += 1,
            429 => report.refused += 1,
            503 => report.busy += 1,
            other => bail!("a login answered {other}"),
        }
        report.latencies.push(latency);
    }
    Ok(report)
}
