//! The recurring jobs: what expired goes away.

use std::sync::Arc;
use std::time::Duration;

use cadocs_collab::{DocRegistry, Event, PageEvent, SpaceEvent};
use cadocs_store::{DocStore, Job, JobStore, now_ms};
use tokio::sync::Notify;

use crate::{Handler, JobError};

pub const PURGE_EXPIRED: &str = "purge_expired";
pub const PURGE_TRASH: &str = "purge_trash";

/// How long a failed job stays for inspection.
const FAILED_JOB_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// Rows read at a time. Each purge is a transaction of its own, so a room's flush can pass between
/// two of them.
const BATCH: u32 = 100;

pub struct Cleanup<S> {
    store: Arc<S>,
    registry: DocRegistry<S>,
    /// Wakes the recheck of every socket.
    access: Arc<Notify>,
    trash_retention: Duration,
}

impl<S> Cleanup<S> {
    pub fn new(
        store: Arc<S>,
        registry: DocRegistry<S>,
        access: Arc<Notify>,
        trash_retention: Duration,
    ) -> Self {
        Self {
            store,
            registry,
            access,
            trash_retention,
        }
    }
}

impl<S: DocStore + JobStore> Cleanup<S> {
    async fn purge_expired(&self) -> Result<(), JobError> {
        let now = now_ms();
        let failed_before = before(now, FAILED_JOB_RETENTION);
        self.store.purge_expired(now, failed_before).await?;
        Ok(())
    }

    /// Publishes what a purge by hand publishes. A crash between the two loses the event, which is
    /// only a hint: the page was already out of the tree.
    async fn purge_trash(&self) -> Result<(), JobError> {
        let before = before(now_ms(), self.trash_retention);
        loop {
            let pages = self.store.expired_pages(before, BATCH).await?;
            if pages.is_empty() {
                break;
            }
            for page in pages {
                if self.store.purge_expired_page(&page.id, before).await? {
                    self.access.notify_one();
                    let event = Event::page(PageEvent::Purged, &page.site.space_id, &page.id);
                    self.registry.publish(&page.site.workspace_id, event);
                }
            }
        }
        loop {
            let spaces = self.store.expired_spaces(before, BATCH).await?;
            if spaces.is_empty() {
                return Ok(());
            }
            for space in spaces {
                if self.store.purge_expired_space(&space.id, before).await? {
                    self.access.notify_one();
                    let event = Event::space(SpaceEvent::Purged, &space.id);
                    self.registry.publish(&space.workspace_id, event);
                }
            }
        }
    }
}

fn before(now: i64, retention: Duration) -> i64 {
    now.saturating_sub(retention.as_millis() as i64)
}

impl<S: DocStore + JobStore> Handler for Cleanup<S> {
    const RECURRING: &'static [&'static str] = &[PURGE_EXPIRED, PURGE_TRASH];

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        match job.kind.as_str() {
            PURGE_EXPIRED => self.purge_expired().await,
            PURGE_TRASH => self.purge_trash().await,
            kind => Err(format!("unknown job kind {kind}").into()),
        }
    }
}
