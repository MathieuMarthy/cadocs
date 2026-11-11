//! The recurring jobs: what expired goes away, and the database gives its free pages back.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use cadocs_collab::{DocRegistry, Event, PageEvent, SpaceEvent};
use cadocs_files::FileStore;
use cadocs_store::{AttachmentStore, DocStore, Job, JobStore, now_ms};
use futures_util::StreamExt;
use tokio::sync::Notify;

use crate::{Handler, JobError};

pub const PURGE_EXPIRED: &str = "purge_expired";
pub const PURGE_TRASH: &str = "purge_trash";
pub const PURGE_ORPHAN_FILES: &str = "purge_orphan_files";
pub const MAINTAIN_DATABASE: &str = "maintain_database";

/// How long a failed job stays for inspection.
const FAILED_JOB_RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// Rows read at a time. Each purge is a transaction of its own, so a room's flush can pass between
/// two of them.
const BATCH: u32 = 100;
/// How old a file with no row must be to go. An upload moves its file into place, then records
/// it: a younger file may be about to get its row.
const STRAY_FILE_AGE: Duration = Duration::from_secs(3600);

pub struct Cleanup<S> {
    store: Arc<S>,
    registry: DocRegistry<S>,
    /// Wakes the recheck of every socket.
    access: Arc<Notify>,
    files: Arc<dyn FileStore>,
    trash_retention: Duration,
}

impl<S> Cleanup<S> {
    pub fn new(
        store: Arc<S>,
        registry: DocRegistry<S>,
        access: Arc<Notify>,
        files: Arc<dyn FileStore>,
        trash_retention: Duration,
    ) -> Self {
        Self {
            store,
            registry,
            access,
            files,
            trash_retention,
        }
    }
}

impl<S: DocStore + JobStore + AttachmentStore> Cleanup<S> {
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

impl<S: AttachmentStore> Cleanup<S> {
    /// Attachments no page references, then files with no row: left by a crash between an
    /// upload's rename and its record, by a deleted workspace, or by a failed delete. Each row goes
    /// before its file, so a crash between the two leaves a file with no row, which the next run
    /// finds.
    async fn purge_orphan_files(&self) -> Result<(), JobError> {
        loop {
            let orphans = self.store.orphan_attachments(BATCH).await?;
            if orphans.is_empty() {
                break;
            }
            for id in orphans {
                if self.store.forget_orphan(&id).await? {
                    self.files.delete(&id).await?;
                }
            }
        }
        let old = SystemTime::now()
            .checked_sub(STRAY_FILE_AGE)
            .unwrap_or(SystemTime::UNIX_EPOCH);
        let mut listing = self.files.list();
        while let Some(batch) = listing.next().await {
            let stale: Vec<String> = batch?
                .into_iter()
                .filter(|file| file.modified < old)
                .map(|file| file.id)
                .collect();
            let known: HashSet<String> = self
                .store
                .known_attachments(&stale)
                .await?
                .into_iter()
                .collect();
            for id in stale.iter().filter(|id| !known.contains(*id)) {
                self.files.delete(id).await?;
            }
        }
        Ok(())
    }
}

fn before(now: i64, retention: Duration) -> i64 {
    now.saturating_sub(retention.as_millis() as i64)
}

impl<S: DocStore + JobStore + AttachmentStore> Handler for Cleanup<S> {
    /// The database maintenance comes after the purges, so it frees the pages they emptied.
    const RECURRING: &'static [&'static str] = &[
        PURGE_EXPIRED,
        PURGE_TRASH,
        PURGE_ORPHAN_FILES,
        MAINTAIN_DATABASE,
    ];

    async fn run(&self, job: &Job) -> Result<(), JobError> {
        match job.kind.as_str() {
            PURGE_EXPIRED => self.purge_expired().await,
            PURGE_TRASH => self.purge_trash().await,
            PURGE_ORPHAN_FILES => self.purge_orphan_files().await,
            MAINTAIN_DATABASE => Ok(self.store.maintain_database().await?),
            kind => Err(format!("unknown job kind {kind}").into()),
        }
    }
}
