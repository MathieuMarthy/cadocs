//! Just enough HTTP/1.1 for the benchmark: one request per connection, closed by the server.
//! An HTTP client crate would bring far more than these few requests need.

use anyhow::{Context, bail};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;

#[derive(Debug)]
pub struct Response {
    pub status: u16,
    /// As sent: a chunked body keeps its framing, which only matters to a byte count.
    pub body: Vec<u8>,
}

/// `host:port` of a `ws://host:port` address.
fn authority(url: &str) -> anyhow::Result<&str> {
    url.strip_prefix("ws://")
        .map(|rest| rest.trim_end_matches('/'))
        .with_context(|| format!("{url} is not a ws:// address"))
}

/// The `Origin` a browser on the server's own address sends.
pub fn http_origin(url: &str) -> String {
    format!("http://{}", authority(url).unwrap_or(url))
}

/// Sends one request to the server of `url`, a `ws://host:port` address.
pub async fn request(
    url: &str,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> anyhow::Result<Response> {
    let host = authority(url)?;
    let mut stream = TcpStream::connect(host)
        .await
        .with_context(|| format!("cannot connect to {host}"))?;
    let mut head = format!(
        "{method} {path} HTTP/1.1\r\nhost: {host}\r\nconnection: close\r\ncontent-length: {}\r\n",
        body.len()
    );
    for (name, value) in headers {
        head.push_str(&format!("{name}: {value}\r\n"));
    }
    head.push_str("\r\n");
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;

    let mut received = Vec::new();
    stream.read_to_end(&mut received).await?;
    let Some(end) = received.windows(4).position(|window| window == b"\r\n\r\n") else {
        bail!("{method} {path}: no complete answer");
    };
    let status = std::str::from_utf8(&received[..end])?
        .split(' ')
        .nth(1)
        .and_then(|code| code.parse().ok())
        .with_context(|| format!("{method} {path}: no status"))?;
    Ok(Response {
        status,
        body: received[end + 4..].to_vec(),
    })
}
