//! The queue of `docs/ARCHITECTURE.md`: claims, retries, crashes, on a real database.

use std::collections::VecDeque;
use std::future::pending;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use cadocs_jobs::{Handler, JobConfig, JobError, JobQueue, Worker};
use cadocs_store::{Job, JobStore, SqliteStore, now_ms};
use sqlx::sqlite::SqliteConnectOptions;
use sqlx::{Row, SqlitePool};
use tempfile::TempDir;
use tokio::task::JoinHandle;
use tokio::time::{sleep, timeout};

const KIND: &str = "test";
const KEY: &str = "test:1";
const MAX_ATTEMPTS: u32 = 3;
const SOON: Duration = Duration::from_secs(10);

#[derive(Clone, Copy)]
enum Step {
    Fail,
    Panic,
    Hang,
}

/// Plays `steps`, one per run, then succeeds.
#[derive(Default)]
struct Script {
    steps: Mutex<VecDeque<Step>>,
    runs: AtomicUsize,
    /// When each run started.
    started: Mutex<Vec<i64>>,
}

impl Script {
    fn of(steps: &[Step]) -> Arc<Self> {
        Arc::new(Self {
            steps: Mutex::new(steps.iter().copied().collect()),
            ..Self::default()
        })
    }

    fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }
}

impl Handler for Script {
    async fn run(&self, _job: &Job) -> Result<(), JobError> {
        self.started.lock().unwrap().push(now_ms());
        self.runs.fetch_add(1, Ordering::SeqCst);
        let step = self.steps.lock().unwrap().pop_front();
        match step {
            None => Ok(()),
            Some(Step::Fail) => Err("this run failed".into()),
            Some(Step::Panic) => panic!("this run panicked"),
            Some(Step::Hang) => pending().await,
        }
    }
}

/// Asks for the recurring job at each period.
#[derive(Default)]
struct Recurring(AtomicUsize);

impl Handler for Recurring {
    const RECURRING: &'static [&'static str] = &[KIND];

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        assert_eq!(job.kind, KIND);
        self.0.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
}

struct Queue {
    queue: JobQueue<SqliteStore>,
    db: SqlitePool,
    path: PathBuf,
    _dir: TempDir,
}

fn config() -> JobConfig {
    JobConfig {
        retry_delay: Duration::from_millis(40),
        max_attempts: MAX_ATTEMPTS,
        poll: Duration::from_secs(60),
        recurring_every: Duration::from_secs(3600),
        ..JobConfig::default()
    }
}

async fn queue() -> Queue {
    let dir = TempDir::new().unwrap();
    let path = dir.path().join("cadocs.db");
    let store = Arc::new(SqliteStore::open(&path).await.unwrap());
    let db = SqlitePool::connect_with(SqliteConnectOptions::new().filename(&path))
        .await
        .unwrap();
    Queue {
        queue: JobQueue::new(store),
        db,
        path,
        _dir: dir,
    }
}

impl Queue {
    fn start<H: Handler>(&self, handler: Arc<H>) -> JoinHandle<()> {
        tokio::spawn(Worker::new(self.queue.clone(), handler, config()).run())
    }

    /// The worker of a new process on the same database.
    async fn restart<H: Handler>(&self, handler: Arc<H>) -> JoinHandle<()> {
        let store = Arc::new(SqliteStore::open(&self.path).await.unwrap());
        tokio::spawn(Worker::new(JobQueue::new(store), handler, config()).run())
    }

    async fn enqueue(&self) {
        let key = Some(KEY);
        self.queue.enqueue(KIND, key, "{}", now_ms()).await.unwrap();
    }

    async fn count(&self, status: &str) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs WHERE status = ?1")
            .bind(status)
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn rows(&self) -> i64 {
        sqlx::query_scalar("SELECT count(*) FROM jobs")
            .fetch_one(&self.db)
            .await
            .unwrap()
    }

    async fn wait_for_status(&self, status: &str, count: i64) {
        let reached = async {
            while self.count(status).await != count {
                sleep(Duration::from_millis(5)).await;
            }
        };
        timeout(SOON, reached).await.expect("never reached");
    }

    async fn wait_until_empty(&self) {
        let reached = async {
            while self.rows().await != 0 {
                sleep(Duration::from_millis(5)).await;
            }
        };
        timeout(SOON, reached)
            .await
            .expect("the queue never emptied");
    }
}

async fn wait_for_runs(script: &Script, runs: usize) {
    let reached = async {
        while script.runs() < runs {
            sleep(Duration::from_millis(5)).await;
        }
    };
    timeout(SOON, reached).await.expect("never ran");
}

#[tokio::test]
async fn a_new_job_runs_at_once_and_goes_away() {
    let queue = queue().await;
    let script = Script::of(&[]);
    let _worker = queue.start(script.clone());
    sleep(Duration::from_millis(50)).await;

    queue.enqueue().await;

    queue.wait_until_empty().await;
    assert_eq!(script.runs(), 1);
}

#[tokio::test]
async fn a_job_waits_for_its_time_with_nothing_to_wake_the_worker() {
    let queue = queue().await;
    let script = Script::of(&[]);
    let run_at = now_ms() + 150;
    queue.queue.enqueue(KIND, None, "{}", run_at).await.unwrap();

    let _worker = queue.start(script.clone());
    wait_for_runs(&script, 1).await;

    assert!(script.started.lock().unwrap()[0] >= run_at);
}

#[tokio::test]
async fn the_payload_reaches_the_handler() {
    struct Echo(Mutex<Option<Job>>);
    impl Handler for Echo {
        async fn run(&self, job: &Job) -> Result<(), JobError> {
            *self.0.lock().unwrap() = Some(job.clone());
            Ok(())
        }
    }
    let queue = queue().await;
    let echo = Arc::new(Echo(Mutex::default()));
    queue
        .queue
        .enqueue(KIND, None, r#"{"a":1}"#, now_ms())
        .await
        .unwrap();

    let _worker = queue.start(echo.clone());
    queue.wait_until_empty().await;

    let job = echo.0.lock().unwrap().clone().unwrap();
    assert_eq!(
        (job.kind.as_str(), job.payload.as_str(), job.attempts),
        (KIND, r#"{"a":1}"#, 1)
    );
}

// Retries

#[tokio::test]
async fn a_failing_job_is_retried_later_and_later_until_it_passes() {
    let queue = queue().await;
    let script = Script::of(&[Step::Fail, Step::Fail]);
    queue.enqueue().await;

    let _worker = queue.start(script.clone());
    queue.wait_until_empty().await;

    assert_eq!(script.runs(), 3);
    let started = script.started.lock().unwrap();
    let first = started[1] - started[0];
    let second = started[2] - started[1];
    assert!(first >= 40, "{first}");
    assert!(second >= 80, "{second}");
}

#[tokio::test]
async fn a_job_out_of_attempts_stays_as_failed_with_its_error() {
    let queue = queue().await;
    let script = Script::of(&[Step::Fail; MAX_ATTEMPTS as usize]);
    queue.enqueue().await;

    let _worker = queue.start(script.clone());
    queue.wait_for_status("failed", 1).await;
    sleep(Duration::from_millis(200)).await;

    assert_eq!(script.runs(), MAX_ATTEMPTS as usize, "it runs no more");
    let row = sqlx::query("SELECT attempts, last_error FROM jobs")
        .fetch_one(&queue.db)
        .await
        .unwrap();
    assert_eq!(row.get::<i64, _>("attempts"), i64::from(MAX_ATTEMPTS));
    assert_eq!(row.get::<String, _>("last_error"), "this run failed");
}

#[tokio::test]
async fn a_panic_is_a_failure_and_the_worker_survives_it() {
    let queue = queue().await;
    let script = Script::of(&[Step::Panic]);
    queue.enqueue().await;

    let _worker = queue.start(script.clone());
    queue.wait_until_empty().await;

    assert_eq!(script.runs(), 2);
}

#[tokio::test]
async fn a_failed_job_blocks_nothing_behind_it() {
    let queue = queue().await;
    let script = Script::of(&[Step::Fail; MAX_ATTEMPTS as usize]);
    queue.enqueue().await;
    let _worker = queue.start(script.clone());
    queue.wait_for_status("failed", 1).await;

    queue.enqueue().await;

    queue.wait_for_status("pending", 0).await;
    wait_for_runs(&script, MAX_ATTEMPTS as usize + 1).await;
}

// Crashes

#[tokio::test]
async fn a_job_cut_by_a_crash_runs_again_at_the_next_start() {
    let queue = queue().await;
    let script = Script::of(&[Step::Hang]);
    queue.enqueue().await;
    let worker = queue.start(script.clone());
    queue.wait_for_status("running", 1).await;

    worker.abort();
    let _ = worker.await;
    assert_eq!(
        queue.count("running").await,
        1,
        "a crash leaves the claim behind"
    );

    let _worker = queue.restart(script.clone()).await;
    queue.wait_until_empty().await;
    assert_eq!(script.runs(), 2);
}

#[tokio::test]
async fn stopping_the_worker_stops_the_job_in_progress() {
    struct Holds(Arc<()>);
    impl Handler for Holds {
        async fn run(&self, _job: &Job) -> Result<(), JobError> {
            let _held = self.0.clone();
            pending().await
        }
    }
    let queue = queue().await;
    let token = Arc::new(());
    queue.enqueue().await;
    let worker = queue.start(Arc::new(Holds(token.clone())));
    queue.wait_for_status("running", 1).await;
    sleep(Duration::from_millis(20)).await;
    assert_eq!(Arc::strong_count(&token), 3);

    worker.abort();
    let _ = worker.await;
    sleep(Duration::from_millis(20)).await;

    assert_eq!(Arc::strong_count(&token), 1, "nothing runs on");
}

#[tokio::test]
async fn a_job_that_keeps_killing_the_server_ends_as_failed() {
    let queue = queue().await;
    queue.enqueue().await;
    let script = Script::of(&[Step::Hang; MAX_ATTEMPTS as usize]);
    let mut worker = queue.start(script.clone());
    for run in 1..=MAX_ATTEMPTS as usize {
        wait_for_runs(&script, run).await;
        worker.abort();
        let _ = worker.await;
        worker = queue.restart(script.clone()).await;
    }

    queue.wait_for_status("failed", 1).await;

    assert_eq!(script.runs(), MAX_ATTEMPTS as usize);
    let error: String = sqlx::query_scalar("SELECT last_error FROM jobs")
        .fetch_one(&queue.db)
        .await
        .unwrap();
    assert!(error.contains("interrupted"), "{error}");
}

// One pending job per key

#[tokio::test]
async fn a_key_holds_one_pending_job() {
    let queue = queue().await;
    queue.enqueue().await;
    queue.enqueue().await;

    assert_eq!(queue.rows().await, 1);
}

#[tokio::test]
async fn a_change_during_a_run_queues_another_run() {
    let queue = queue().await;
    let script = Script::of(&[Step::Hang]);
    queue.enqueue().await;
    let worker = queue.start(script.clone());
    queue.wait_for_status("running", 1).await;

    queue.enqueue().await;

    assert_eq!(
        queue.rows().await,
        2,
        "the run in progress may have missed it"
    );
    worker.abort();
}

#[tokio::test]
async fn a_retry_gives_way_to_a_pending_job_of_its_key() {
    let queue = queue().await;
    let store = SqliteStore::open(&queue.path).await.unwrap();
    queue.enqueue().await;
    let claimed = store.claim_job(now_ms()).await.unwrap().unwrap();
    queue.enqueue().await;

    store
        .retry_job(&claimed.id, now_ms(), "failed")
        .await
        .unwrap();

    assert_eq!(queue.rows().await, 1);
    assert_eq!(queue.count("pending").await, 1);
}

#[tokio::test]
async fn a_crashed_job_gives_way_to_a_pending_job_of_its_key() {
    let queue = queue().await;
    let store = SqliteStore::open(&queue.path).await.unwrap();
    queue.enqueue().await;
    store.claim_job(now_ms()).await.unwrap().unwrap();
    queue.enqueue().await;
    let script = Script::of(&[]);

    let _worker = queue.restart(script.clone()).await;
    queue.wait_until_empty().await;

    assert_eq!(script.runs(), 1);
}

// Recurring jobs

#[tokio::test]
async fn a_recurring_job_runs_at_start_and_at_each_period() {
    let queue = queue().await;
    let handler = Arc::new(Recurring::default());
    let every = JobConfig {
        recurring_every: Duration::from_millis(50),
        ..config()
    };

    let _worker = tokio::spawn(Worker::new(queue.queue.clone(), handler.clone(), every).run());

    let reached = async {
        while handler.0.load(Ordering::SeqCst) < 3 {
            sleep(Duration::from_millis(5)).await;
        }
    };
    timeout(SOON, reached).await.expect("it stopped recurring");
}
