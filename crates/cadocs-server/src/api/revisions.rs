//! Revisions of a page: listed and shown to its readers, restored by its editors. A restore goes
//! through the room, so open clients receive it as an edit, and the content it replaces is kept
//! as a revision first.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::middleware::from_fn_with_state;
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_content::PageTitles;
use cadocs_content::ast::Document;
use cadocs_content::html::to_html;
use cadocs_jobs::{self as revisions, RevisionError, decode_bounded};
use cadocs_store::{AccountStore, DocStore, RevisionStore, RevisionSummary};
use serde::Serialize;
use tracing::warn;

use super::{ApiError, ErrorCode, Page, PageQuery, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + RevisionStore,
    A: Authorizer,
{
    Router::new()
        .route("/pages/{page}/revisions", get(list))
        .route("/pages/{page}/revisions/{n}", get(view))
        .route("/pages/{page}/revisions/{n}/restore", post(restore))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

#[derive(Serialize)]
struct Author {
    id: String,
    name: String,
}

#[derive(Serialize)]
struct SummaryBody {
    number: i64,
    title: String,
    authors: Vec<Author>,
    created_at: i64,
}

impl From<RevisionSummary> for SummaryBody {
    fn from(summary: RevisionSummary) -> Self {
        Self {
            number: summary.number,
            title: summary.title,
            authors: summary
                .authors
                .into_iter()
                .map(|author| Author {
                    id: author.id,
                    name: author.name,
                })
                .collect(),
            created_at: summary.created_at,
        }
    }
}

#[derive(Serialize)]
struct RevisionBody {
    #[serde(flatten)]
    summary: SummaryBody,
    /// Rendered by the server, with the titles of the linked pages the caller can open.
    html: String,
}

impl From<RevisionError> for ApiError {
    fn from(error: RevisionError) -> Self {
        match error {
            RevisionError::Store(error) => error.into(),
            RevisionError::Collab(error) => error.into(),
            RevisionError::Corrupt => ApiError::internal(error),
        }
    }
}

fn number(raw: &str) -> Result<i64, ApiError> {
    raw.parse()
        .ok()
        .filter(|number| *number > 0)
        .ok_or_else(|| ApiError::new(ErrorCode::ValidationFailed).with_field("n", "invalid"))
}

async fn list<S: RevisionStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(page): PathParam,
    query: PageQuery,
) -> Result<Json<Page<SummaryBody>>, ApiError> {
    let before = match &query.cursor {
        Some(cursor) => Some(number(cursor).map_err(|_| {
            ApiError::new(ErrorCode::ValidationFailed).with_field("cursor", "invalid")
        })?),
        None => None,
    };
    let rows = state
        .store
        .revisions(&page, &current.user.id, before, query.fetch())
        .await?;
    Ok(Json(query.cut(rows, |row| row.number.to_string())))
}

async fn view<S: DocStore + RevisionStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam((page, n)): PathParam<(String, String)>,
) -> Result<Json<RevisionBody>, ApiError> {
    let user = &current.user.id;
    let revision = state.store.revision(&page, number(&n)?, user).await?;
    let decoded = decode_bounded(&state.registry, &revision.state).await?;
    let titles = Readable::read(&*state.store, user, &page, &decoded.document).await?;
    let html = to_html(&decoded.document, &titles);
    Ok(Json(RevisionBody {
        summary: revision.summary.into(),
        html,
    }))
}

/// The titles and names a reader of a page may see in its content: those of the pages it links to
/// that they can open, and of the members of its workspace it mentions.
pub(super) struct Readable {
    titles: HashMap<String, String>,
    names: HashMap<String, String>,
}

impl Readable {
    pub(super) async fn read<S: RevisionStore>(
        store: &S,
        user: &str,
        page: &str,
        document: &Document,
    ) -> Result<Self, ApiError> {
        let titles = store.readable_titles(user, &document.page_links()).await?;
        let names = store.member_names(user, page, &document.mentions()).await?;
        Ok(Self {
            titles: titles.into_iter().collect(),
            names: names.into_iter().collect(),
        })
    }
}

impl PageTitles for Readable {
    fn title(&self, id: &str) -> Option<String> {
        self.titles.get(id).cloned()
    }

    fn name(&self, user_id: &str) -> Option<String> {
        self.names.get(user_id).cloned()
    }
}

/// The current content becomes a revision before it is replaced (`cadocs_jobs::restore`).
async fn restore<S: DocStore + RevisionStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam((page, n)): PathParam<(String, String)>,
) -> Result<StatusCode, ApiError> {
    let number = number(&n)?;
    let user = &current.user.id;
    let title_replaced =
        revisions::restore(&*state.store, &state.registry, &page, number, user).await?;
    if !title_replaced {
        warn!(%page, "a restore left a title that holds more than text");
    }
    Ok(StatusCode::NO_CONTENT)
}
