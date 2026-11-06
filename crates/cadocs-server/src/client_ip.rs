//! Who a request really comes from, behind the proxies listed in `CADOCS_TRUSTED_PROXIES`.

use std::net::IpAddr;

use anyhow::{Context, ensure};
use axum::http::HeaderMap;

const FORWARDED_FOR: &str = "x-forwarded-for";

/// One address, or a block such as `172.18.0.0/16`: a proxy in Docker changes address on restart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IpBlock {
    network: IpAddr,
    prefix: u32,
}

impl IpBlock {
    fn parse(text: &str) -> anyhow::Result<Self> {
        let (address, prefix) = match text.split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text, None),
        };
        let network = address
            .parse::<IpAddr>()
            .with_context(|| format!("{text:?} is not an address"))?
            .to_canonical();
        let bits = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix
                .parse()
                .with_context(|| format!("{text:?} has an invalid prefix"))?,
            None => bits,
        };
        ensure!(prefix <= bits, "{text:?} has an invalid prefix");
        Ok(Self { network, prefix })
    }

    /// `ip` must be canonical.
    fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip) {
            (IpAddr::V4(network), IpAddr::V4(ip)) => {
                let mask = u32::MAX.checked_shl(32 - self.prefix).unwrap_or(0);
                u32::from(network) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(network), IpAddr::V6(ip)) => {
                let mask = u128::MAX.checked_shl(128 - self.prefix).unwrap_or(0);
                u128::from(network) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrustedProxies(Vec<IpBlock>);

impl TrustedProxies {
    /// Reads a comma-separated list. An empty one trusts nobody.
    pub fn parse(list: &str) -> anyhow::Result<Self> {
        if list.trim().is_empty() {
            return Ok(Self::default());
        }
        let blocks = list.split(',').map(|block| IpBlock::parse(block.trim()));
        Ok(Self(blocks.collect::<anyhow::Result<_>>()?))
    }

    fn contains(&self, ip: IpAddr) -> bool {
        self.0.iter().any(|block| block.contains(ip))
    }

    /// The address to hold responsible for a request that arrived from `peer`.
    ///
    /// Each proxy appends the address it saw, so only the end of `X-Forwarded-For` can be believed:
    /// the walk goes from the right and stops at the first hop no trusted proxy vouches for.
    /// Anything further left was written by the client.
    pub fn client_ip(&self, peer: IpAddr, headers: &HeaderMap) -> IpAddr {
        let mut client = peer.to_canonical();
        if !self.contains(client) {
            return client;
        }
        let hops = headers
            .get_all(FORWARDED_FOR)
            .iter()
            .rev()
            .flat_map(|value| value.as_bytes().rsplit(|byte| *byte == b','));
        for hop in hops {
            // By bytes: a client's junk on the left must not hide what the proxy appended.
            let hop = str::from_utf8(hop).unwrap_or_default().trim();
            let Ok(ip) = hop.parse::<IpAddr>() else {
                break;
            };
            client = ip.to_canonical();
            if !self.contains(client) {
                break;
            }
        }
        client
    }
}

#[cfg(test)]
mod tests {
    use axum::http::HeaderValue;

    use super::*;

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn forwarded(values: &[&str]) -> HeaderMap {
        let mut headers = HeaderMap::new();
        for value in values {
            headers.append(FORWARDED_FOR, HeaderValue::from_str(value).unwrap());
        }
        headers
    }

    fn proxies(list: &str) -> TrustedProxies {
        TrustedProxies::parse(list).unwrap()
    }

    #[test]
    fn without_proxies_the_header_is_ignored() {
        let client = proxies("").client_ip(ip("203.0.113.9"), &forwarded(&["198.51.100.1"]));
        assert_eq!(client, ip("203.0.113.9"));
    }

    #[test]
    fn an_untrusted_peer_cannot_set_the_header() {
        let client =
            proxies("10.0.0.1").client_ip(ip("203.0.113.9"), &forwarded(&["198.51.100.1"]));
        assert_eq!(client, ip("203.0.113.9"));
    }

    #[test]
    fn a_trusted_peer_gives_the_client() {
        let client = proxies("10.0.0.1").client_ip(ip("10.0.0.1"), &forwarded(&["198.51.100.1"]));
        assert_eq!(client, ip("198.51.100.1"));
    }

    #[test]
    fn what_the_client_wrote_itself_is_skipped() {
        // The client sent `1.1.1.1`, and the proxy appended the address it saw.
        let client =
            proxies("10.0.0.1").client_ip(ip("10.0.0.1"), &forwarded(&["1.1.1.1, 198.51.100.1"]));
        assert_eq!(client, ip("198.51.100.1"));
    }

    #[test]
    fn a_chain_of_proxies_is_walked_from_the_right() {
        let client = proxies("10.0.0.0/8").client_ip(
            ip("10.0.0.1"),
            &forwarded(&["1.1.1.1, 198.51.100.1", "10.2.0.1, 10.3.0.1"]),
        );
        assert_eq!(client, ip("198.51.100.1"));
    }

    #[test]
    fn a_header_of_trusted_hops_only_gives_the_first_hop() {
        let client = proxies("10.0.0.0/8").client_ip(ip("10.0.0.1"), &forwarded(&["10.2.0.1"]));
        assert_eq!(client, ip("10.2.0.1"));
    }

    #[test]
    fn a_trusted_peer_without_the_header_is_the_client() {
        let client = proxies("10.0.0.1").client_ip(ip("10.0.0.1"), &HeaderMap::new());
        assert_eq!(client, ip("10.0.0.1"));
    }

    #[test]
    fn an_unreadable_hop_stops_the_walk() {
        let client = proxies("10.0.0.0/8").client_ip(
            ip("10.0.0.1"),
            &forwarded(&["198.51.100.1, garbage, 10.2.0.1"]),
        );
        assert_eq!(client, ip("10.2.0.1"));
    }

    #[test]
    fn a_header_that_is_not_text_is_ignored() {
        let mut headers = HeaderMap::new();
        headers.insert(FORWARDED_FOR, HeaderValue::from_bytes(b"\xff").unwrap());
        let client = proxies("10.0.0.1").client_ip(ip("10.0.0.1"), &headers);
        assert_eq!(client, ip("10.0.0.1"));
    }

    #[test]
    fn bytes_a_client_sent_do_not_hide_the_hop_the_proxy_appended() {
        let mut headers = HeaderMap::new();
        let value = HeaderValue::from_bytes(b"\xff, 198.51.100.1").unwrap();
        headers.insert(FORWARDED_FOR, value);
        let client = proxies("10.0.0.1").client_ip(ip("10.0.0.1"), &headers);
        assert_eq!(client, ip("198.51.100.1"));
    }

    #[test]
    fn mapped_addresses_count_as_ipv4() {
        // What a dual-stack listener reports for an IPv4 peer.
        let client = proxies("10.0.0.1")
            .client_ip(ip("::ffff:10.0.0.1"), &forwarded(&["::ffff:198.51.100.1"]));
        assert_eq!(client, ip("198.51.100.1"));
    }

    #[test]
    fn blocks_cover_their_addresses_only() {
        let list = proxies("192.168.1.128/25, fd00::/8");
        let hop = forwarded(&["198.51.100.1"]);
        assert_eq!(
            list.client_ip(ip("192.168.1.200"), &hop),
            ip("198.51.100.1")
        );
        assert_eq!(
            list.client_ip(ip("192.168.1.127"), &hop),
            ip("192.168.1.127")
        );
        assert_eq!(list.client_ip(ip("fd12::1"), &hop), ip("198.51.100.1"));
        assert_eq!(list.client_ip(ip("fe00::1"), &hop), ip("fe00::1"));
    }

    #[test]
    fn a_zero_prefix_covers_its_whole_family() {
        let list = proxies("0.0.0.0/0");
        let hop = forwarded(&["198.51.100.1"]);
        assert_eq!(list.client_ip(ip("203.0.113.9"), &hop), ip("198.51.100.1"));
        assert_eq!(list.client_ip(ip("2001:db8::1"), &hop), ip("2001:db8::1"));
    }

    #[test]
    fn invalid_lists_are_refused() {
        for list in [
            "nonsense",
            "10.0.0.1/33",
            "fd00::/129",
            "10.0.0.1/x",
            "10.0.0.1,,",
        ] {
            assert!(TrustedProxies::parse(list).is_err(), "{list}");
        }
    }
}
