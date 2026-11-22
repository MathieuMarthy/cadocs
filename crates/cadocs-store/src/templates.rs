//! Templates of a space ([`DATA_MODEL.md`]): the state of a page, kept to start others from.
//! Readers of the space list and use them, editors save and delete them.
//!
//! [`DATA_MODEL.md`]: ../../../docs/DATA_MODEL.md#templates

use std::future::Future;

use crate::StoreError;

/// The most templates a space holds, which bounds the list.
pub const MAX_TEMPLATES: i64 = 200;

#[derive(Debug, Clone)]
pub struct NewTemplate {
    pub id: String,
    pub space_id: String,
    pub title: String,
    /// The Yjs state, compressed as a revision's is.
    pub state: Vec<u8>,
    pub actor: String,
    pub now: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pub id: String,
    pub space_id: String,
    pub title: String,
    pub created_by: Option<String>,
    pub created_at: i64,
}

/// A template to start a page from, with the stored state.
#[derive(Debug, Clone)]
pub struct TemplateToUse {
    pub template: Template,
    pub state: Vec<u8>,
}

pub trait TemplateStore: Send + Sync + 'static {
    /// Refused with `SpaceNotFound`, or `NotAllowed` for a reader who is not an editor, and with
    /// `TooManyTemplates` past [`MAX_TEMPLATES`].
    fn create_template(
        &self,
        new: &NewTemplate,
    ) -> impl Future<Output = Result<Template, StoreError>> + Send;

    /// At most `limit` templates of the space, oldest first, after the template `after`. Refused
    /// with `SpaceNotFound` for whoever cannot read the space.
    fn list_templates(
        &self,
        space_id: &str,
        actor: &str,
        after: Option<&str>,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<Template>, StoreError>> + Send;

    /// The template, for a user who reads its space and edits `target`, a space of the same
    /// workspace, or its own space when `None`. Refused with `TemplateNotFound`, `NotAllowed`, or
    /// `OtherWorkspace`.
    fn template_to_use(
        &self,
        template_id: &str,
        target: Option<&str>,
        actor: &str,
    ) -> impl Future<Output = Result<TemplateToUse, StoreError>> + Send;

    /// Refused with `TemplateNotFound` for whoever cannot read its space, or `NotAllowed` for a
    /// reader who is not an editor.
    fn delete_template(
        &self,
        template_id: &str,
        actor: &str,
    ) -> impl Future<Output = Result<(), StoreError>> + Send;
}
