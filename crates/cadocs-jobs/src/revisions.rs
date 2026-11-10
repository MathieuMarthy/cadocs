//! Revisions of a page (docs/DATA_MODEL.md#documents): full Yjs states, compressed, taken by a job
//! after edits and before a restore, and read back as the AST for rendering or restoring.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use std::time::Duration;

use cadocs_collab::{CollabError, DocRegistry};
use cadocs_content::ast::Document;
use cadocs_content::yjs::{read, read_page, read_title, replace_page};
use cadocs_store::{DocStore, Job, NewRevision, RevisionStore, StoreError, now_ms};
use serde_json::Value;
use uuid::Uuid;
use yrs::updates::decoder::Decode;
use yrs::{Doc, ReadTxn, StateVector, Transact, Update};

use crate::JobError;

/// Fast, and states are small: a higher level saves little and costs more memory.
const ZSTD_LEVEL: i32 = 3;

#[derive(Debug)]
pub enum RevisionError {
    Store(StoreError),
    Collab(CollabError),
    /// A stored state that cannot be read back.
    Corrupt,
}

impl fmt::Display for RevisionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Store(error) => error.fmt(f),
            Self::Collab(error) => error.fmt(f),
            Self::Corrupt => f.write_str("a revision cannot be read back"),
        }
    }
}

impl std::error::Error for RevisionError {}

impl From<StoreError> for RevisionError {
    fn from(error: StoreError) -> Self {
        Self::Store(error)
    }
}

impl From<CollabError> for RevisionError {
    fn from(error: CollabError) -> Self {
        Self::Collab(error)
    }
}

/// A revision read back: its title and content, and the size of its state once decompressed.
#[derive(Debug, Clone, PartialEq)]
pub struct Decoded {
    pub title: String,
    pub document: Document,
    pub state_bytes: usize,
}

/// Reads back a stored state. yrs can panic on a broken update, and a state holds what clients
/// wrote, so that is caught.
pub fn decode(state: &[u8]) -> Result<Decoded, RevisionError> {
    let raw = zstd::stream::decode_all(state).map_err(|_| RevisionError::Corrupt)?;
    let read = catch_unwind(AssertUnwindSafe(|| {
        let update = Update::decode_v1(&raw).ok()?;
        let doc = Doc::new();
        doc.transact_mut().apply_update(update).ok()?;
        Some(read_page(&doc))
    }));
    let (title, document) = read.ok().flatten().ok_or(RevisionError::Corrupt)?;
    Ok(Decoded {
        title,
        document,
        state_bytes: raw.len(),
    })
}

/// Decodes under a turn of `doc_reads`: a revision weighs as much as a document read outside a
/// room, so readers clicking through a history cannot raise RAM past that bound.
pub async fn decode_bounded<S: DocStore>(
    registry: &DocRegistry<S>,
    state: &[u8],
) -> Result<Decoded, RevisionError> {
    registry.bounded(|| decode(state)).await?
}

/// A page as one transaction read it: what a revision compares and stores.
struct Current {
    title: String,
    document: Document,
    state: Vec<u8>,
}

fn current<T: ReadTxn>(txn: &T) -> Current {
    Current {
        title: read_title(txn),
        document: read(txn),
        state: txn.encode_state_as_update_v1(&StateVector::default()),
    }
}

/// Takes a revision of the page as it is now, flushed or not, unless its title and content are
/// those of the latest one. Answers the new revision's number. A page with no revision compares
/// with an empty one, and a page that is gone gets none.
pub async fn capture<S: DocStore + RevisionStore>(
    store: &S,
    registry: &DocRegistry<S>,
    page_id: &str,
) -> Result<Option<i64>, RevisionError> {
    let read_at = now_ms();
    let page = registry
        .read(page_id, |doc| current(&doc.transact()))
        .await?;
    keep(store, registry, page_id, page, read_at).await
}

async fn keep<S: DocStore + RevisionStore>(
    store: &S,
    registry: &DocRegistry<S>,
    page_id: &str,
    page: Current,
    read_at: i64,
) -> Result<Option<i64>, RevisionError> {
    let latest = match store.latest_revision(page_id).await? {
        // One that cannot be read back is no reason to keep no other.
        Some(revision) => decode_bounded(registry, &revision.state).await.ok(),
        None => Some(Decoded {
            title: String::new(),
            document: Document::default(),
            state_bytes: 0,
        }),
    };
    if latest.is_some_and(|latest| latest.title == page.title && latest.document == page.document) {
        return Ok(None);
    }
    let compressed =
        zstd::bulk::compress(&page.state, ZSTD_LEVEL).map_err(|_| RevisionError::Corrupt)?;
    drop(page.state);
    let id = Uuid::now_v7().to_string();
    let revision = NewRevision {
        id: &id,
        page_id,
        title: &page.title,
        state: &compressed,
        read_at,
    };
    Ok(store.add_revision(&revision).await?)
}

/// Restores revision `number` of a page for `user`, through its room: open clients receive it as
/// an edit. Checked before the room loads, so a refused user cannot load rooms, then again after,
/// as a load can take long. The current content is kept first, durably. The room's transaction
/// also reads what it replaces, so edits that arrived since are kept too, once it is done.
/// Answers `false` when the title held more than text and stayed as it was.
pub async fn restore<S: DocStore + RevisionStore>(
    store: &S,
    registry: &DocRegistry<S>,
    page_id: &str,
    number: i64,
    user: &str,
) -> Result<bool, RevisionError> {
    let revision = store.revision_to_restore(page_id, number, user).await?;
    let restored = decode_bounded(registry, &revision.state).await?;
    drop(revision);
    registry.open(page_id).await?;
    store.revision_to_restore(page_id, number, user).await?;
    capture(store, registry, page_id).await?;
    let read_at = now_ms();
    let (title_replaced, replaced) = registry
        .rewrite(page_id, user, restored.state_bytes, |txn| {
            let replaced = current(&*txn);
            let title_replaced = replace_page(txn, &restored.title, &restored.document);
            (title_replaced, replaced)
        })
        .await?;
    drop(restored);
    keep(store, registry, page_id, replaced, read_at).await?;
    Ok(title_replaced)
}

/// Runs `CREATE_REVISION` and `THIN_REVISIONS` jobs.
pub struct Revisions<S> {
    store: Arc<S>,
    registry: DocRegistry<S>,
    /// How long every revision is kept, before thinning to one per day.
    retention: Duration,
}

impl<S> Revisions<S> {
    pub fn new(store: Arc<S>, registry: DocRegistry<S>, retention: Duration) -> Self {
        Self {
            store,
            registry,
            retention,
        }
    }
}

impl<S: DocStore + RevisionStore> Revisions<S> {
    pub async fn create(&self, job: &Job) -> Result<(), JobError> {
        let payload: Value = serde_json::from_str(&job.payload)?;
        let page_id = payload["page"]
            .as_str()
            .ok_or("a revision job names no page")?;
        capture(&*self.store, &self.registry, page_id).await?;
        Ok(())
    }

    pub async fn thin(&self) -> Result<(), JobError> {
        let retention = i64::try_from(self.retention.as_millis()).unwrap_or(i64::MAX);
        self.store
            .thin_revisions(now_ms().saturating_sub(retention))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use cadocs_content::ast::{Block, Inline, Marks};
    use cadocs_content::yjs::replace_page;

    use super::*;

    #[test]
    fn a_state_reads_back_through_compression() {
        let document = Document {
            blocks: vec![Block::Paragraph(vec![Inline::Text {
                text: "kept".into(),
                marks: Marks::default(),
            }])],
        };
        let doc = Doc::new();
        replace_page(&mut doc.transact_mut(), "Title", &document);
        let raw = doc
            .transact()
            .encode_state_as_update_v1(&StateVector::default());
        let stored = zstd::bulk::compress(&raw, ZSTD_LEVEL).unwrap();

        let decoded = decode(&stored).unwrap();
        assert_eq!(decoded.title, "Title");
        assert_eq!(decoded.document, document);
        assert_eq!(decoded.state_bytes, raw.len());
    }

    #[test]
    fn a_broken_state_is_corrupt() {
        assert!(matches!(decode(b"nonsense"), Err(RevisionError::Corrupt)));
        let garbage = zstd::bulk::compress(&[255, 255, 255, 1, 2], ZSTD_LEVEL).unwrap();
        assert!(matches!(decode(&garbage), Err(RevisionError::Corrupt)));
    }
}
