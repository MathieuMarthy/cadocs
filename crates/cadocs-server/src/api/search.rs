//! Full-text search (ADR 0007). Results come from the spaces the caller can read, filtered in the
//! SQL query, and their excerpts are made here from the real words.

use axum::extract::{RawQuery, State};
use axum::middleware::from_fn_with_state;
use axum::routing::get;
use axum::{Extension, Json, Router};
use cadocs_collab::Authorizer;
use cadocs_search::{Analyzer, Language, Segment};
use cadocs_store::{AccountStore, DocStore, SearchHit, SearchStore, WorkspaceStore};
use percent_encoding::percent_decode_str;
use serde::Serialize;

use super::{ApiError, ErrorCode, PathParam};
use crate::AppState;
use crate::session::{self, CurrentSession};

/// Results of one search: the best ones, with no further page.
const RESULTS: u32 = 20;
/// Characters of a query.
const MAX_QUERY_CHARS: usize = 200;

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + AccountStore + WorkspaceStore + SearchStore,
    A: Authorizer,
{
    Router::new()
        .route("/workspaces/{ws}/search", get(search))
        .route_layer(from_fn_with_state(state.clone(), session::require::<S, A>))
}

#[derive(Serialize)]
struct Results {
    items: Vec<Hit>,
}

#[derive(Serialize)]
struct Hit {
    id: String,
    space_id: String,
    title: String,
    excerpt: Vec<Piece>,
}

/// A piece of an excerpt, as text: the client shows `match` pieces marked, never as HTML.
#[derive(Serialize)]
struct Piece {
    text: String,
    #[serde(rename = "match")]
    matched: bool,
}

impl From<Segment> for Piece {
    fn from(segment: Segment) -> Self {
        Self {
            text: segment.text,
            matched: segment.matched,
        }
    }
}

struct SearchQuery {
    q: String,
    space: Option<String>,
}

/// `q` and `space`. `+` is a space, as `URLSearchParams` writes it.
fn parse(query: Option<&str>) -> Result<SearchQuery, ApiError> {
    let invalid = |problem| {
        ApiError::new(ErrorCode::ValidationFailed)
            .with_message("A search needs 1 to 200 characters")
            .with_field("q", problem)
    };
    let mut q = None;
    let mut space = None;
    for pair in query.unwrap_or_default().split('&') {
        let Some((name, value)) = pair.split_once('=') else {
            continue;
        };
        let value = value.replace('+', " ");
        let decoded = percent_decode_str(&value).decode_utf8();
        match name {
            // Only the start: a space after the last word says that it is whole.
            "q" => {
                q = Some(
                    decoded
                        .map_err(|_| invalid("invalid"))?
                        .trim_start()
                        .to_owned(),
                )
            }
            "space" => space = decoded.ok().map(|space| space.into_owned()),
            _ => {}
        }
    }
    let q = q
        .filter(|q| !q.is_empty())
        .ok_or_else(|| invalid("required"))?;
    if q.chars().count() > MAX_QUERY_CHARS {
        return Err(invalid("too_long"));
    }
    if q.chars().any(|c| c.is_control() && !c.is_whitespace()) {
        return Err(invalid("invalid"));
    }
    Ok(SearchQuery { q, space })
}

async fn search<S: WorkspaceStore + SearchStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(current): Extension<CurrentSession>,
    PathParam(workspace): PathParam,
    RawQuery(query): RawQuery,
) -> Result<Json<Results>, ApiError> {
    let SearchQuery { q, space } = parse(query.as_deref())?;
    let user = &current.user.id;
    let found = state.store.workspace(&workspace, user).await?;
    let found = found.ok_or_else(|| ApiError::new(ErrorCode::NotFound))?;
    let language = Language::parse(&found.language)
        .unwrap_or_else(|| Language::parse("en").expect("English is a language"));
    let analyzer = Analyzer::new(language);
    let Some(query) = analyzer.query(&q) else {
        return Ok(Json(Results { items: Vec::new() }));
    };
    let hits = state
        .store
        .search(&workspace, user, query.fts(), space.as_deref(), RESULTS)
        .await?;
    let items = hits
        .into_iter()
        .map(|SearchHit { page, plain_text }| Hit {
            excerpt: analyzer
                .excerpt(&query, &plain_text)
                .into_iter()
                .map(Piece::from)
                .collect(),
            id: page.id,
            space_id: page.space_id,
            title: page.title,
        })
        .collect();
    Ok(Json(Results { items }))
}
