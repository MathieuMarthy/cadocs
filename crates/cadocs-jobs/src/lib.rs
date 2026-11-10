//! Background work: one worker over the `jobs` table (ADR 0006).

mod cleanup;

use std::error::Error;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use cadocs_store::{Job, JobStore, NewJob, StoreError, now_ms};
use tokio::sync::Notify;
use tokio::task::JoinSet;
use tokio::time::{MissedTickBehavior, interval, sleep};
use tracing::warn;
use uuid::Uuid;

pub use cleanup::{Cleanup, PURGE_EXPIRED, PURGE_ORPHAN_FILES, PURGE_TRASH};

/// Whatever went wrong, for `last_error`.
pub type JobError = Box<dyn Error + Send + Sync>;

const MAX_RETRY_DELAY: Duration = Duration::from_secs(3600);
/// The shortest sleep, so that a store refusing claims does not spin on a job that is due.
const MIN_NAP: Duration = Duration::from_millis(10);
const PANICKED: &str = "the job panicked";

#[derive(Debug, Clone, Copy)]
pub struct JobConfig {
    /// Delay before the first retry. It doubles with each further one.
    pub retry_delay: Duration,
    /// Runs of a job before it stays as `failed`.
    pub max_attempts: u32,
    /// The longest the worker sleeps, for a job no `Notify` announced.
    pub poll: Duration,
    /// Time between two runs of a recurring job.
    pub recurring_every: Duration,
    /// How long a page or a space stays in the trash.
    pub trash_retention: Duration,
}

impl Default for JobConfig {
    fn default() -> Self {
        Self {
            retry_delay: Duration::from_secs(30),
            max_attempts: 5,
            poll: Duration::from_secs(60),
            recurring_every: Duration::from_secs(3600),
            trash_retention: Duration::from_secs(30 * 24 * 3600),
        }
    }
}

/// What a job does. It must be idempotent: a crash can run a job twice.
pub trait Handler: Send + Sync + 'static {
    /// Kinds to run at start, then every `recurring_every`, with their kind as their key.
    const RECURRING: &'static [&'static str] = &[];

    fn run(&self, job: &Job) -> impl Future<Output = Result<(), JobError>> + Send;
}

/// Adds jobs and wakes the worker.
pub struct JobQueue<S> {
    store: Arc<S>,
    wake: Arc<Notify>,
}

impl<S> Clone for JobQueue<S> {
    fn clone(&self) -> Self {
        Self {
            store: self.store.clone(),
            wake: self.wake.clone(),
        }
    }
}

impl<S: JobStore> JobQueue<S> {
    pub fn new(store: Arc<S>) -> Self {
        Self {
            store,
            wake: Arc::default(),
        }
    }

    /// Nothing happens if a job with this key is waiting. `payload` is JSON.
    pub async fn enqueue(
        &self,
        kind: &str,
        dedup_key: Option<&str>,
        payload: &str,
        run_at: i64,
    ) -> Result<(), StoreError> {
        let job = NewJob {
            id: Uuid::now_v7().to_string(),
            kind: kind.to_owned(),
            dedup_key: dedup_key.map(str::to_owned),
            payload: payload.to_owned(),
            run_at,
            now: now_ms(),
        };
        self.store.enqueue(&job).await?;
        self.wake.notify_one();
        Ok(())
    }
}

/// The one task that runs jobs, one at a time.
pub struct Worker<S, H> {
    queue: JobQueue<S>,
    handler: Arc<H>,
    config: JobConfig,
}

impl<S: JobStore, H: Handler> Worker<S, H> {
    pub fn new(queue: JobQueue<S>, handler: Arc<H>, config: JobConfig) -> Self {
        Self {
            queue,
            handler,
            config,
        }
    }

    pub async fn run(self) {
        self.recover().await;
        let mut recurring = interval(self.config.recurring_every);
        recurring.set_missed_tick_behavior(MissedTickBehavior::Delay);
        loop {
            self.run_due().await;
            let nap = self.nap().await;
            tokio::select! {
                () = self.queue.wake.notified() => {}
                () = sleep(nap) => {}
                _ = recurring.tick() => self.enqueue_recurring().await,
            }
        }
    }

    /// Nothing runs before this succeeds: a claim left by a crash would stay `running`.
    async fn recover(&self) {
        let store = &self.queue.store;
        while let Err(error) = store.reset_running_jobs(self.config.max_attempts).await {
            warn!(%error, "cannot reset the jobs of the last run");
            sleep(self.config.poll).await;
        }
    }

    async fn enqueue_recurring(&self) {
        for kind in H::RECURRING {
            let queued = self.queue.enqueue(kind, Some(kind), "{}", now_ms());
            if let Err(error) = queued.await {
                warn!(kind, %error, "cannot queue a recurring job");
            }
        }
    }

    async fn run_due(&self) {
        loop {
            match self.queue.store.claim_job(now_ms()).await {
                Ok(Some(job)) => self.execute(job).await,
                Ok(None) => return,
                Err(error) => {
                    warn!(%error, "cannot claim a job");
                    return;
                }
            }
        }
    }

    async fn execute(&self, job: Job) {
        let result = self.outcome(&job).await;
        let store = &self.queue.store;
        let settled = match &result {
            Ok(()) => store.finish_job(&job.id).await,
            Err(error) if job.attempts >= self.config.max_attempts => {
                warn!(id = %job.id, kind = %job.kind, %error, "a job ran out of attempts");
                store.fail_job(&job.id, error).await
            }
            Err(error) => {
                warn!(id = %job.id, kind = %job.kind, %error, "a job failed");
                let delay = retry_delay(self.config.retry_delay, job.attempts);
                let run_at = now_ms().saturating_add(delay.as_millis() as i64);
                store.retry_job(&job.id, run_at, error).await
            }
        };
        // The claim stays until the next start, which runs the job again.
        if let Err(error) = settled {
            warn!(id = %job.id, %error, "cannot record the end of a job");
        }
    }

    /// In a task of its own, so a panic costs one attempt and not the worker. A `JoinSet` aborts
    /// that task if the worker itself is stopped.
    async fn outcome(&self, job: &Job) -> Result<(), String> {
        let handler = self.handler.clone();
        let claimed = job.clone();
        let mut run = JoinSet::new();
        run.spawn(async move {
            let result = handler.run(&claimed).await;
            result.map_err(|error| error.to_string())
        });
        match run.join_next().await {
            Some(Ok(result)) => result,
            _ => Err(PANICKED.to_owned()),
        }
    }

    async fn nap(&self) -> Duration {
        let next = match self.queue.store.next_job_at().await {
            Ok(next) => next,
            Err(error) => {
                warn!(%error, "cannot read the next job");
                None
            }
        };
        let poll = self.config.poll;
        next.map_or(poll, |at| {
            let wait = Duration::from_millis(at.saturating_sub(now_ms()).max(0) as u64);
            wait.clamp(MIN_NAP, poll.max(MIN_NAP))
        })
    }
}

/// After the run number `attempts`, the first one being 1.
fn retry_delay(first: Duration, attempts: u32) -> Duration {
    let factor = 1u32 << attempts.saturating_sub(1).min(16);
    first.saturating_mul(factor).min(MAX_RETRY_DELAY)
}
