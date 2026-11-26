//! Limits per client address, per mail address, and per account. Each one has one date, when it will have its
//! whole burst back.

use std::collections::HashMap;
use std::hash::{BuildHasher, Hash, RandomState};
use std::net::IpAddr;
use std::sync::{Arc, Mutex, PoisonError};
use std::time::{Duration, Instant};

/// About 300 KiB at most, reached only under a flood of addresses.
const MAX_TRACKED: usize = 4096;
/// A full table of addresses is logged this often at most, so a flood does not fill the log.
const FULL_REPORT_EVERY: Duration = Duration::from_secs(60);

/// What both limiters are made of: a date per key.
#[derive(Debug)]
struct Buckets<K> {
    refill: Duration,
    /// How far ahead of now a client's date may go.
    burst: Duration,
    clients: Mutex<HashMap<K, Instant>>,
}

#[derive(Debug)]
pub struct RateLimiter {
    buckets: Buckets<IpAddr>,
}

impl RateLimiter {
    /// Allows `burst` requests at once, then one more per `refill`.
    pub fn new(burst: u32, refill: Duration) -> Self {
        Self {
            buckets: Buckets::new(burst, refill),
        }
    }

    /// Counts one request from `client`, or says how long it must wait.
    pub fn check(&self, client: IpAddr) -> Result<(), Duration> {
        self.check_at(client, Instant::now())
    }

    fn check_at(&self, client: IpAddr, now: Instant) -> Result<(), Duration> {
        self.buckets.check_at(key(client), now)
    }
}

/// Bounds the messages one address gets, whoever asks for them (ADR 0026). It keeps a keyed hash
/// of each address, 8 bytes, so a flood of long addresses costs no more than one of clients, and
/// nobody can pick two addresses that share a bucket.
#[derive(Debug)]
pub struct AddressLimiter {
    buckets: Buckets<u64>,
    hasher: RandomState,
    /// When a full table was last logged.
    reported: Mutex<Option<Instant>>,
}

impl AddressLimiter {
    /// Allows `burst` messages at once, then one more per `refill`.
    pub fn new(burst: u32, refill: Duration) -> Self {
        Self {
            buckets: Buckets::new(burst, refill),
            hasher: RandomState::new(),
            reported: Mutex::default(),
        }
    }

    /// Counts one message to `address`, in the form it is stored in, and says whether it may go.
    /// A refusal is never shown to a client, so a full table, which refuses every new address, is
    /// logged: nothing else would tell the operator.
    pub fn check(&self, address: &str) -> bool {
        let (allowed, report) = self.count_at(address, Instant::now());
        if report {
            tracing::warn!("too many addresses are asking for mail, some get none for now");
        }
        allowed
    }

    /// Whether the message may go, and whether a full table is to be logged now.
    fn count_at(&self, address: &str, now: Instant) -> (bool, bool) {
        if self.check_at(address, now).is_ok() {
            return (true, false);
        }
        if self.buckets.tracked() < MAX_TRACKED {
            return (false, false);
        }
        let mut reported = self.reported.lock().unwrap_or_else(PoisonError::into_inner);
        let due = reported.is_none_or(|at| now.duration_since(at) >= FULL_REPORT_EVERY);
        if due {
            *reported = Some(now);
        }
        (false, due)
    }

    fn check_at(&self, address: &str, now: Instant) -> Result<(), Duration> {
        self.buckets.check_at(self.hasher.hash_one(address), now)
    }
}

/// Bounds the guesses at the second factor of one account, from any client: a code from an app
/// cannot die after a few tries as a mailed one does. Only someone who knows the password counts
/// here, so a full table, which makes newcomers wait, takes a flood of right passwords.
#[derive(Debug)]
pub struct AccountLimiter {
    buckets: Buckets<String>,
}

impl AccountLimiter {
    /// Allows `burst` guesses at once, then one more per `refill`.
    pub fn new(burst: u32, refill: Duration) -> Self {
        Self {
            buckets: Buckets::new(burst, refill),
        }
    }

    /// Counts one guess for the account `user_id`, or says how long it must wait.
    pub fn check(&self, user_id: &str) -> Result<(), Duration> {
        self.buckets.check_at(user_id.to_owned(), Instant::now())
    }
}

impl<K: Hash + Eq> Buckets<K> {
    fn new(burst: u32, refill: Duration) -> Self {
        Self {
            refill,
            burst: refill * burst,
            clients: Mutex::default(),
        }
    }

    fn tracked(&self) -> usize {
        let clients = self.clients.lock().unwrap_or_else(PoisonError::into_inner);
        clients.len()
    }

    fn check_at(&self, key: K, now: Instant) -> Result<(), Duration> {
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

/// Turns of work held for as long as an answer is being sent: `total` at once, at most
/// `per_client` for one address, so one client alone cannot take them all.
pub struct Turns {
    total: usize,
    per_client: usize,
    taken: Mutex<HashMap<IpAddr, usize>>,
}

/// One turn, given back when dropped.
pub struct Turn {
    turns: Arc<Turns>,
    client: IpAddr,
}

impl Turns {
    pub fn new(total: usize, per_client: usize) -> Self {
        Self {
            total,
            per_client,
            taken: Mutex::default(),
        }
    }

    /// A turn for `client`, if one is free for it.
    pub fn take(self: &Arc<Self>, client: IpAddr) -> Option<Turn> {
        let client = key(client);
        let mut taken = self.taken.lock().unwrap_or_else(PoisonError::into_inner);
        let all: usize = taken.values().sum();
        let mine = taken.entry(client).or_default();
        if all >= self.total || *mine >= self.per_client {
            if *mine == 0 {
                taken.remove(&client);
            }
            return None;
        }
        *mine += 1;
        Some(Turn {
            turns: self.clone(),
            client,
        })
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        let mut taken = self
            .turns
            .taken
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if let Some(mine) = taken.get_mut(&self.client) {
            *mine -= 1;
            if *mine == 0 {
                taken.remove(&self.client);
            }
        }
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

    #[test]
    fn a_client_takes_its_share_of_turns_and_gives_them_back() {
        let turns = Arc::new(Turns::new(3, 2));
        let one: IpAddr = "192.0.2.1".parse().unwrap();
        let other: IpAddr = "2001:db8:1:2::1".parse().unwrap();
        let same_site: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        let a = turns.take(one).unwrap();
        let b = turns.take(one).unwrap();
        assert!(turns.take(one).is_none(), "its share is two");
        let c = turns.take(other).unwrap();
        assert!(turns.take(same_site).is_none(), "three in all");
        drop(c);
        let c = turns.take(same_site).unwrap();
        assert!(turns.take(other).is_none(), "a /48 is one client");
        drop((a, b, c));
        assert!(turns.taken.lock().unwrap().is_empty());
    }

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
        assert_eq!(limiter.buckets.clients.lock().unwrap().len(), MAX_TRACKED);
        assert!(limiter.check_at(numbered(0), now).is_err());
    }

    #[test]
    fn each_address_has_its_own_bucket() {
        let limiter = AddressLimiter::new(2, REFILL);
        let now = Instant::now();
        limiter.check_at("ada@example.org", now).unwrap();
        limiter.check_at("ada@example.org", now).unwrap();
        assert_eq!(limiter.check_at("ada@example.org", now), Err(REFILL));
        assert_eq!(limiter.check_at("grace@example.org", now), Ok(()));
        assert_eq!(limiter.check_at("ada@example.org", now + REFILL), Ok(()));
    }

    #[test]
    fn a_full_address_table_is_reported_once_a_minute() {
        // A refill long enough for the table to stay full through the test.
        let limiter = AddressLimiter::new(1, FULL_REPORT_EVERY * 10);
        let now = Instant::now();
        for n in 0..MAX_TRACKED {
            let counted = limiter.count_at(&format!("user{n}@example.org"), now);
            assert_eq!(counted, (true, false));
        }
        assert_eq!(limiter.count_at("a@example.org", now), (false, true));
        let soon = now + Duration::from_secs(5);
        assert_eq!(limiter.count_at("b@example.org", soon), (false, false));
        let later = now + FULL_REPORT_EVERY;
        assert_eq!(limiter.count_at("c@example.org", later), (false, true));
    }

    #[test]
    fn an_address_past_its_burst_is_not_a_full_table() {
        let limiter = AddressLimiter::new(1, REFILL);
        let now = Instant::now();
        assert_eq!(limiter.count_at("ada@example.org", now), (true, false));
        assert_eq!(limiter.count_at("ada@example.org", now), (false, false));
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
        assert_eq!(limiter.buckets.clients.lock().unwrap().len(), 1);
    }
}
