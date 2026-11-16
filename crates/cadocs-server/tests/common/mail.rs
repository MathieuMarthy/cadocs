//! A server whose mail goes to a relay the test reads (ADR 0026).

use std::time::{Duration, Instant};

use cadocs_mail::fake::{Behavior, FakeRelay, Received};

use super::{Api, ORIGIN, start_tuned};

/// Set up, with `settings` on top of those of the relay.
pub async fn with_relay_and(
    behavior: Behavior,
    proxies: &str,
    settings: &[(&str, &str)],
) -> (Api, FakeRelay) {
    let (api, relay) = before_setup(behavior, proxies, settings).await;
    assert_eq!(api.setup().await.status, 201);
    (api, relay)
}

/// Not set up yet.
pub async fn before_setup(
    behavior: Behavior,
    proxies: &str,
    settings: &[(&str, &str)],
) -> (Api, FakeRelay) {
    let relay = FakeRelay::start(behavior).await;
    let port = relay.port().to_string();
    let mut all = vec![
        ("CADOCS_SMTP_HOST", "127.0.0.1"),
        ("CADOCS_SMTP_PORT", port.as_str()),
        ("CADOCS_SMTP_TLS", "none"),
        ("CADOCS_SMTP_FROM", "Team wiki <wiki@example.org>"),
    ];
    all.extend_from_slice(settings);
    (start_tuned(ORIGIN, proxies, &all).await, relay)
}

pub async fn with_relay(behavior: Behavior, proxies: &str) -> (Api, FakeRelay) {
    with_relay_and(behavior, proxies, &[]).await
}

pub async fn with_mail() -> (Api, FakeRelay) {
    with_relay(Behavior::default(), "").await
}

/// The first `count` messages, once they arrived.
pub async fn messages(relay: &FakeRelay, count: usize) -> Vec<Received> {
    let limit = Instant::now() + Duration::from_secs(5);
    loop {
        let messages = relay.messages();
        if messages.len() >= count {
            return messages;
        }
        assert!(Instant::now() < limit, "{count} messages never arrived");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

/// Long enough for a message that should not exist to show up.
pub async fn settle() {
    tokio::time::sleep(Duration::from_millis(300)).await;
}

/// The text of a message, with the soft line breaks of quoted-printable taken out.
pub fn text(message: &Received) -> String {
    message.data.replace("=\r\n", "")
}

/// The body of a message, after its headers.
pub fn body(message: &Received) -> String {
    let text = text(message);
    let (_, body) = text.split_once("\r\n\r\n").expect("headers and a body");
    body.to_owned()
}
