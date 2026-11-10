//! The search index of a page (ADR 0007): its text, stemmed, and the attachments it shows.

use std::sync::Arc;

use cadocs_collab::DocRegistry;
use cadocs_content::text::to_text;
use cadocs_content::yjs::read_page;
use cadocs_search::{Analyzer, Language};
use cadocs_store::{DocStore, Job, PageIndex, SearchStore};
use serde_json::Value;
use tracing::warn;

use crate::JobError;

/// Runs `REINDEX_PAGE` jobs.
pub struct Index<S> {
    store: Arc<S>,
    registry: DocRegistry<S>,
}

impl<S> Index<S> {
    pub fn new(store: Arc<S>, registry: DocRegistry<S>) -> Self {
        Self { store, registry }
    }
}

impl<S: DocStore + SearchStore> Index<S> {
    /// A page that is gone has nothing to index, and its index went with it.
    pub async fn reindex(&self, job: &Job) -> Result<(), JobError> {
        let payload: Value = serde_json::from_str(&job.payload)?;
        let page_id = payload["page"]
            .as_str()
            .ok_or("a reindex job names no page")?;
        let Some(target) = self.store.index_target(page_id).await? else {
            return Ok(());
        };
        let language = Language::parse(&target.language).unwrap_or_else(|| {
            warn!(language = %target.language, "unknown language, stemmed as English");
            Language::parse("en").expect("English is a language")
        });
        // Only the reading happens under the read turn: the rest is plain work on what it gave.
        let (title, document) = self.registry.read(page_id, read_page).await?;
        let plain_text = to_text(&document);
        let analyzer = Analyzer::new(language);
        let title = analyzer.index_text(&title);
        let body = analyzer.index_text(&plain_text);
        let attachments = document.attachments();
        self.store
            .index_page(&PageIndex {
                page_id,
                title: &title,
                body: &body,
                plain_text: &plain_text,
                attachments: &attachments,
            })
            .await?;
        Ok(())
    }
}
