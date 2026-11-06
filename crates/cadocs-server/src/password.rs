//! Argon2id at OWASP settings. One hash takes about 19 MB, so a semaphore bounds how many run at
//! once (`docs/ARCHITECTURE.md`).

use std::sync::Arc;
use std::time::Duration;

use argon2::Argon2;
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use tokio::sync::{Notify, Semaphore};
use tokio::task::spawn_blocking;
use tokio::time::timeout;

/// One at a time: the target machine has one or a few cores, and each hash is a RAM peak.
const CONCURRENT_HASHES: usize = 1;
/// Past this a request gives up, so that a flood of logins cannot queue without end.
const MAX_WAIT: Duration = Duration::from_secs(10);

/// Verified when an account has no password, so that it takes as long as a real check. Its
/// parameters must stay those of `Argon2::default()`: a test checks it.
const DECOY_HASH: &str = "$argon2id$v=19$m=19456,t=2,p=1$+eXI9vhLCiyxELoMmwWCDg$VYG+0aN+vdoiDKhNaW+f7Zj0JWyWj/aU9h4+HYFRk20";

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("too many password hashes are waiting")]
    Busy,
    #[error("password hashing failed: {0}")]
    Failed(String),
}

#[derive(Debug)]
pub struct Passwords {
    permits: Arc<Semaphore>,
    max_wait: Duration,
    finished: Arc<Notify>,
}

impl Default for Passwords {
    fn default() -> Self {
        Self::new(CONCURRENT_HASHES, MAX_WAIT)
    }
}

impl Passwords {
    fn new(concurrent: usize, max_wait: Duration) -> Self {
        Self {
            permits: Arc::new(Semaphore::new(concurrent)),
            max_wait,
            finished: Arc::default(),
        }
    }

    pub async fn hash(&self, password: String) -> Result<String, PasswordError> {
        self.run(move || {
            let hash = Argon2::default().hash_password(password.as_bytes());
            hash.map(|hash| hash.to_string())
                .map_err(|error| PasswordError::Failed(error.to_string()))
        })
        .await?
    }

    /// Says whether `password` matches `hash`. With no hash the answer is no, in the same time.
    pub async fn verify(
        &self,
        password: String,
        hash: Option<String>,
    ) -> Result<bool, PasswordError> {
        self.run(move || {
            let stored = hash.as_deref().unwrap_or(DECOY_HASH);
            let parsed = PasswordHash::new(stored)
                .map_err(|error| PasswordError::Failed(error.to_string()))?;
            let matches = Argon2::default().verify_password(password.as_bytes(), &parsed);
            Ok(matches.is_ok() && hash.is_some())
        })
        .await?
    }

    /// Resolves once a hash has ended since the last call. Several hashes may count as one.
    pub async fn hash_finished(&self) {
        self.finished.notified().await;
    }

    /// The blocking task owns the permit: a client that hangs up cannot free it while the memory
    /// of its hash is still in use.
    async fn run<T: Send + 'static>(
        &self,
        work: impl FnOnce() -> T + Send + 'static,
    ) -> Result<T, PasswordError> {
        let permit = timeout(self.max_wait, self.permits.clone().acquire_owned())
            .await
            .map_err(|_| PasswordError::Busy)?
            .map_err(|_| PasswordError::Busy)?;
        let finished = self.finished.clone();
        spawn_blocking(move || {
            let _permit = permit;
            let result = work();
            finished.notify_one();
            result
        })
        .await
        .map_err(|error| PasswordError::Failed(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use argon2::Params;

    use super::*;

    #[tokio::test]
    async fn a_hash_verifies_its_password_only() {
        let passwords = Passwords::default();
        let hash = passwords.hash("right".to_owned()).await.unwrap();
        assert!(hash.starts_with("$argon2id$"));
        let check = |password: &str| passwords.verify(password.to_owned(), Some(hash.clone()));
        assert!(check("right").await.unwrap());
        assert!(!check("wrong").await.unwrap());
    }

    #[tokio::test]
    async fn no_password_matches_a_missing_hash() {
        let passwords = Passwords::default();
        let result = passwords.verify("decoy".to_owned(), None).await;
        assert!(!result.unwrap());
    }

    #[test]
    fn the_decoy_costs_as_much_as_a_real_hash() {
        let decoy = PasswordHash::new(DECOY_HASH).unwrap();
        let params = Params::try_from(&decoy).unwrap();
        let default = Params::default();
        assert_eq!(decoy.algorithm.as_str(), "argon2id");
        assert_eq!(params.m_cost(), default.m_cost());
        assert_eq!(params.t_cost(), default.t_cost());
        assert_eq!(params.p_cost(), default.p_cost());
    }

    #[tokio::test]
    async fn a_finished_hash_is_announced() {
        let passwords = Passwords::default();
        passwords.hash("password".to_owned()).await.unwrap();
        timeout(Duration::from_secs(5), passwords.hash_finished())
            .await
            .expect("the memory of a hash must get a chance to go back");
    }

    #[tokio::test]
    async fn past_the_wait_a_hash_gives_up() {
        let passwords = Passwords::new(1, Duration::from_millis(20));
        let held = passwords.permits.clone().acquire_owned().await.unwrap();
        let result = passwords.hash("password".to_owned()).await;
        assert!(matches!(result, Err(PasswordError::Busy)), "{result:?}");
        drop(held);
        assert!(passwords.hash("password".to_owned()).await.is_ok());
    }

    #[tokio::test]
    async fn no_more_hashes_run_than_the_semaphore_allows() {
        let passwords = Arc::new(Passwords::new(1, Duration::from_secs(30)));
        let running = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let tasks = (0..4).map(|_| {
            let (passwords, running, peak) = (passwords.clone(), running.clone(), peak.clone());
            tokio::spawn(async move {
                use std::sync::atomic::Ordering::SeqCst;
                passwords
                    .run(move || {
                        peak.fetch_max(running.fetch_add(1, SeqCst) + 1, SeqCst);
                        std::thread::sleep(Duration::from_millis(5));
                        running.fetch_sub(1, SeqCst);
                    })
                    .await
            })
        });
        for task in tasks.collect::<Vec<_>>() {
            task.await.unwrap().unwrap();
        }
        assert_eq!(peak.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
