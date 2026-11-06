//! Limits per client address. Each client has one date, when it will have its whole burst back.

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

/// About 300 KiB at most, reached only under a flood of addresses.
const MAX_TRACKED: usize = 4096;

#[derive(Debug)]
pub struct RateLimiter {
    refill: Duration,
    /// How far ahead of now a client's date may go.
    burst: Duration,
    clients: Mutex<HashMap<IpAddr, Instant>>,
}

impl RateLimiter {
    /// Allows `burst` requests at once, then one more per `refill`.
    pub fn new(burst: u32, refill: Duration) -> Self {
        Self {
            refill,
            burst: refill * burst,
            clients: Mutex::default(),
        }
    }

    /// Counts one request from `client`, or says how long it must wait.
    pub fn check(&self, client: IpAddr) -> Result<(), Duration> {
        self.check_at(client, Instant::now())
    }

    fn check_at(&self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        let key = key(client);
        let mut clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);

        if clients.len() >= MAX_TRACKED && !clients.contains_key(&key) {
            clients.retain(|_, full_at| *full_at > now);
            if clients.len() >= MAX_TRACKED {
                // Forgetting someone would hand them a fresh burst, so the newcomer waits.
                return Err(self.refill);
            }
        }

        let full_at = clients.entry(key).or_insert(now);
        let next = (*full_at).max(now) + self.refill;
        let ahead = next - now;
        if ahead > self.burst {
            return Err(ahead - self.burst);
        }
        *full_at = next;
        Ok(())
    }
}

/// One site can get a whole IPv6 /48. Counting /64s would give it 65,536 clients, enough to fill
/// the table and make every newcomer wait.
fn key(client: IpAddr) -> IpAddr {
    match client.to_canonical() {
        IpAddr::V6(ip) => IpAddr::V6((u128::from(ip) & (u128::MAX << 80)).into()),
        ip => ip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const REFILL: Duration = Duration::from_secs(10);

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    fn numbered(n: usize) -> IpAddr {
        IpAddr::from((0x0A00_0000 + n as u32).to_be_bytes())
    }

    #[test]
    fn a_burst_passes_then_the_client_waits() {
        let limiter = RateLimiter::new(3, REFILL);
        let (client, now) = (ip("198.51.100.1"), Instant::now());
        for _ in 0..3 {
            assert_eq!(limiter.check_at(client, now), Ok(()));
        }
        assert_eq!(limiter.check_at(client, now), Err(REFILL));
    }

    #[test]
    fn the_wait_shrinks_as_time_passes() {
        let limiter = RateLimiter::new(1, REFILL);
        let (client, now) = (ip("198.51.100.1"), Instant::now());
        limiter.check_at(client, now).unwrap();
        let later = now + Duration::from_secs(4);
        let wait = limiter.check_at(client, later).unwrap_err();
        assert!(
            wait <= Duration::from_secs(6) && wait > Duration::from_secs(5),
            "{wait:?}"
        );
    }

    #[test]
    fn a_refused_request_does_not_cost_a_token() {
        let limiter = RateLimiter::new(1, REFILL);
        let (client, now) = (ip("198.51.100.1"), Instant::now());
        limiter.check_at(client, now).unwrap();
        for second in 1..10 {
            let at = now + Duration::from_secs(second);
            assert!(limiter.check_at(client, at).is_err());
        }
        assert_eq!(limiter.check_at(client, now + REFILL), Ok(()));
    }

    #[test]
    fn tokens_stop_at_the_burst() {
        let limiter = RateLimiter::new(2, REFILL);
        let (client, now) = (ip("198.51.100.1"), Instant::now());
        limiter.check_at(client, now).unwrap();
        let later = now + REFILL * 100;
        limiter.check_at(client, later).unwrap();
        limiter.check_at(client, later).unwrap();
        assert!(limiter.check_at(client, later).is_err());
    }

    #[test]
    fn clients_do_not_share_a_bucket() {
        let limiter = RateLimiter::new(1, REFILL);
        let now = Instant::now();
        limiter.check_at(ip("198.51.100.1"), now).unwrap();
        assert_eq!(limiter.check_at(ip("198.51.100.2"), now), Ok(()));
    }

    #[test]
    fn one_ipv6_site_is_one_client() {
        let limiter = RateLimiter::new(1, REFILL);
        let now = Instant::now();
        limiter.check_at(ip("2001:db8:1:2::1"), now).unwrap();
        assert!(limiter.check_at(ip("2001:db8:1:2::ffff"), now).is_err());
        assert!(limiter.check_at(ip("2001:db8:1:ffff::1"), now).is_err());
        assert_eq!(limiter.check_at(ip("2001:db8:2::1"), now), Ok(()));
    }

    #[test]
    fn the_table_never_outgrows_its_limit() {
        let limiter = RateLimiter::new(1, REFILL);
        let now = Instant::now();
        for n in 0..MAX_TRACKED {
            limiter.check_at(numbered(n), now).unwrap();
        }
        assert_eq!(limiter.check_at(numbered(MAX_TRACKED), now), Err(REFILL));
        assert_eq!(limiter.clients.lock().unwrap().len(), MAX_TRACKED);
        assert!(limiter.check_at(numbered(0), now).is_err());
    }

    #[test]
    fn refilled_clients_make_room_for_new_ones() {
        let limiter = RateLimiter::new(1, REFILL);
        let now = Instant::now();
        for n in 0..MAX_TRACKED {
            limiter.check_at(numbered(n), now).unwrap();
        }
        let later = now + REFILL;
        assert_eq!(limiter.check_at(numbered(MAX_TRACKED), later), Ok(()));
        assert_eq!(limiter.clients.lock().unwrap().len(), 1);
    }
}
