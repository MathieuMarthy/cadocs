//! The job queue, and what the cleanup jobs ask of the database. No method here checks a role: only
//! the worker calls them.

use std::future::Future;

use crate::{PageSite, StoreError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewJob {
    pub id: String,
    pub kind: String,
    /// At most one job with this key waits at a time. A job in progress does not count: it may
    /// have read what the new one is about.
    pub dedup_key: Option<String>,
    /// JSON.
    pub payload: String,
    pub run_at: i64,
    pub now: i64,
}

/// A claimed job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub payload: String,
    /// This run included.
    pub attempts: u32,
}

/// A page past its time in the trash, with no such page above it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredPage {
    pub id: String,
    pub site: PageSite,
}

/// A space past its time in the trash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExpiredSpace {
    pub id: String,
    pub workspace_id: String,
}

pub trait JobStore: Send + Sync + 'static {
    /// Adds a job, unless one with its key is waiting.
    fn enqueue(&self, job: &NewJob) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Takes the job due first, and counts this attempt, so a job that kills the process runs out
    /// of attempts too.
    fn claim_job(&self, now: i64) -> impl Future<Output = Result<Option<Job>, StoreError>> + Send;

    /// When the next waiting job is due, on the writer: a poll must not keep a reader open.
    fn next_job_at(&self) -> impl Future<Output = Result<Option<i64>, StoreError>> + Send;

    fn finish_job(&self, id: &str) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Puts a claimed job back in the queue, or drops it if another with its key waits by now.
    fn retry_job(
        &self,
        id: &str,
        run_at: i64,
        error: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Keeps a job as `failed`, for inspection.
    fn fail_job(
        &self,
        id: &str,
        error: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// To call at startup: only one process exists, so no job is really running. One that used its
    /// `max_attempts` becomes `failed`, and the others wait again, or give way to one with their
    /// key.
    fn reset_running_jobs(
        &self,
        max_attempts: u32,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// Deletes sessions and links expired at `now`, and jobs that failed before `failed_before`.
    fn purge_expired(
        &self,
        now: i64,
        failed_before: i64,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;

    /// At most `limit` pages trashed at `before` or earlier. A page under one of them is left out:
    /// it goes with it, as in a purge by hand.
    fn expired_pages(
        &self,
        before: i64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ExpiredPage>, StoreError>> + Send;

    /// Deletes a page and all under it, unless it was restored or trashed again since `before`.
    /// Answers whether it did.
    fn purge_expired_page(
        &self,
        page_id: &str,
        before: i64,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;

    /// At most `limit` spaces trashed at `before` or earlier.
    fn expired_spaces(
        &self,
        before: i64,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<ExpiredSpace>, StoreError>> + Send;

    /// Deletes a space and its pages, unless it was restored or trashed again since `before`.
    /// Answers whether it did.
    fn purge_expired_space(
        &self,
        space_id: &str,
        before: i64,
    ) -> impl Future<Output = Result<bool, StoreError>> + Send;
}
