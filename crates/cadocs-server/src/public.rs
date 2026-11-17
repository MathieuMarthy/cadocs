//! Public pages of a share (V1), under `/s/{token}`: HTML rendered on the server from the
//! document, read through the bounded reader, with no script anywhere (ADR 0008, ADR 0009).
//!
//! Every closed door answers the same `404`: an unknown token, a revoked or expired share, a page
//! or space in the trash, a page or file the share does not cover. A policy with
//! `script-src 'none'` keeps a script from running even if one slipped through the renderer.

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::Arc;

use axum::extract::State;
use axum::http::header::{CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, ETAG, HeaderName};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::middleware::from_fn_with_state;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Extension, Router};
use cadocs_collab::Authorizer;
use cadocs_content::Links;
use cadocs_content::html::to_html_with;
use cadocs_content::yjs::read_page;
use cadocs_store::{DocStore, ShareStore, SharedPage, SharedView, now_ms};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC, utf8_percent_encode};
use sha2::{Digest, Sha256};

use crate::AppState;
use crate::api::attachments::{matches_etag, send};
use crate::api::{self, ApiError, PathParam};

/// How many children a public page lists below its content.
pub const CHILDREN_LIMIT: u32 = 200;
/// What one client may ask of public pages at once: a page and its images come in one burst.
pub const PUBLIC_BURST: u32 = 120;
/// Then one request every this long.
pub const PUBLIC_REFILL: std::time::Duration = std::time::Duration::from_millis(100);

const X_ROBOTS_TAG: HeaderName = HeaderName::from_static("x-robots-tag");
/// Revalidated on each view, so a revoked share closes at once.
const CACHE: &str = "no-cache";
/// What a path segment keeps unescaped.
const SEGMENT: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'_')
    .remove(b'.')
    .remove(b'~');

/// The only style of a public page, allowed by its hash: no `'unsafe-inline'`, and no request.
const STYLE: &str = "\
:root{color-scheme:light dark;--bg:#f5f5f4;--ink:#141414;--muted:#5a5a58;--line:#cdcdca;\
--panel:#ebebea;--accent:#b3390d}\
@media (prefers-color-scheme:dark){:root{--bg:#0e0e0e;--ink:#e6e6e4;--muted:#999996;\
--line:#2b2b2b;--panel:#151515;--accent:#ff8a5c}}\
*{box-sizing:border-box}\
body{margin:0;background:var(--bg);color:var(--ink);\
font:16px/1.6 \"IBM Plex Sans\",system-ui,sans-serif}\
main{max-width:46rem;margin:0 auto;padding:2rem 1rem 4rem}\
a{color:var(--accent)}\
h1,h2,h3{line-height:1.25}\
.title{font-size:2rem;margin:0 0 1.5rem}\
.trail{color:var(--muted);font-size:.875rem}\
.trail a{color:inherit}\
pre,code{font-family:\"IBM Plex Mono\",ui-monospace,monospace;font-size:.9em}\
pre{background:var(--panel);border:1px solid var(--line);padding:.75rem 1rem;overflow-x:auto}\
blockquote{margin:1rem 0;padding-left:1rem;border-left:3px solid var(--line);color:var(--muted)}\
table{border-collapse:collapse;display:block;overflow-x:auto}\
th,td{border:1px solid var(--line);padding:.25rem .5rem;vertical-align:top}\
td p,th p{margin:0}\
[data-align=center]{text-align:center}[data-align=right]{text-align:right}\
img{max-width:100%;height:auto}figure{margin:1rem 0}\
.tasks{list-style:none;padding-left:0}.task{display:flex;gap:.5rem}.task>div>p{margin:0}\
.callout{border:1px solid var(--line);border-left:4px solid var(--accent);padding:.5rem 1rem;\
margin:1rem 0}\
.toc ol{list-style:none;padding-left:0}\
.toc [data-depth=\"1\"]{padding-left:1rem}.toc [data-depth=\"2\"]{padding-left:2rem}\
.page-link-missing{color:var(--muted)}\
.plugin{border:1px dashed var(--line);padding:.5rem 1rem}\
.children{margin-top:3rem;border-top:1px solid var(--line)}\
.children h2{font-size:1rem;color:var(--muted)}";

/// The headers every public answer carries, fixed at startup.
pub struct PublicHeaders {
    policy: HeaderValue,
}

impl PublicHeaders {
    /// `files_origin` is where the file store sends browsers for images, if anywhere.
    pub fn new(files_origin: Option<&str>) -> Self {
        let images = match files_origin {
            Some(origin) => format!("'self' {origin}"),
            None => "'self'".to_owned(),
        };
        let style = base64(&Sha256::digest(STYLE.as_bytes()));
        let policy = format!(
            "default-src 'none'; script-src 'none'; img-src {images}; \
             style-src 'sha256-{style}'; object-src 'none'; base-uri 'none'; \
             form-action 'none'; frame-ancestors 'none'"
        );
        Self {
            policy: HeaderValue::try_from(policy).expect("an origin from a parsed URL"),
        }
    }
}

pub fn routes<S, A>(state: &AppState<S, A>) -> Router<AppState<S, A>>
where
    S: DocStore + ShareStore,
    A: Authorizer,
{
    let limit = (state.public_requests.clone(), state.http.clone());
    let headers = Arc::new(PublicHeaders::new(state.files.download_origin().as_deref()));
    Router::new()
        .route("/s/{token}", get(shared::<S, A>))
        .route("/s/{token}/p/{slug}", get(descendant::<S, A>))
        .route("/s/{token}/files/{id}", get(file::<S, A>))
        .route_layer(from_fn_with_state(limit, api::rate_limit))
        .layer(Extension(headers))
}

async fn shared<S: DocStore + ShareStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(public): Extension<Arc<PublicHeaders>>,
    PathParam(token): PathParam,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    page(&state, &public, &token, None, &headers).await
}

async fn descendant<S: DocStore + ShareStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(public): Extension<Arc<PublicHeaders>>,
    PathParam((token, slug)): PathParam<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    page(&state, &public, &token, Some(&slug), &headers).await
}

async fn file<S: ShareStore, A>(
    State(state): State<AppState<S, A>>,
    Extension(public): Extension<Arc<PublicHeaders>>,
    PathParam((token, id)): PathParam<(String, String)>,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    let now = now_ms();
    let Some(found) = state.store.shared_attachment(&token, &id, now).await? else {
        return Ok(not_found(&public));
    };
    // Never indexed, even for an indexable share: an engine indexes the page, not each file.
    let mut response = send(&*state.files, &found, &headers, CACHE).await?;
    response
        .headers_mut()
        .insert(X_ROBOTS_TAG, HeaderValue::from_static("noindex"));
    Ok(response)
}

async fn page<S: DocStore + ShareStore, A>(
    state: &AppState<S, A>,
    public: &PublicHeaders,
    token: &str,
    slug: Option<&str>,
    headers: &HeaderMap,
) -> Result<Response, ApiError> {
    let now = now_ms();
    let view = state
        .store
        .shared_view(token, slug, now, CHILDREN_LIMIT)
        .await?;
    let Some(view) = view else {
        return Ok(not_found(public));
    };
    // A revalidation renders too: the room may hold edits no date shows yet, and a `304` must
    // never keep content an editor removed. It saves the body, not the read.
    let (title, document) = state.registry.read(&view.page.id, read_page).await?;
    let covered = state
        .store
        .shared_pages(token, &document.page_links(), now)
        .await?;
    let links = ShareLinks::new(token, &view.share.page_id, &covered);
    let titles: HashMap<&str, &str> = covered
        .iter()
        .map(|page| (page.id.as_str(), page.title.as_str()))
        .collect();
    let titles = |id: &str| {
        titles
            .get(id)
            .map(|title| title_or_untitled(title).to_owned())
    };
    let content = to_html_with(&document, &titles, &links);
    let html = document_html(&view, &links, &title, &content);
    let etag = etag(&html, view.share.indexable);
    let mut fixed = vec![
        (
            ETAG,
            HeaderValue::try_from(etag.as_str()).map_err(ApiError::internal)?,
        ),
        (CACHE_CONTROL, HeaderValue::from_static(CACHE)),
        (CONTENT_SECURITY_POLICY, public.policy.clone()),
    ];
    if !view.share.indexable {
        fixed.push((X_ROBOTS_TAG, HeaderValue::from_static("noindex")));
    }
    if matches_etag(headers, &etag) {
        return Ok((StatusCode::NOT_MODIFIED, headers_of(fixed)).into_response());
    }
    fixed.push((
        CONTENT_TYPE,
        HeaderValue::from_static("text/html; charset=utf-8"),
    ));
    Ok((headers_of(fixed), html).into_response())
}

fn headers_of(list: Vec<(HeaderName, HeaderValue)>) -> HeaderMap {
    list.into_iter().collect()
}

/// The same answer for every closed door, under the public policy.
fn not_found(public: &PublicHeaders) -> Response {
    let html = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>Not found</title>\
         <style>{STYLE}</style></head><body><main><h1 class=\"title\">Not found</h1>\
         <p>This link does not exist, has expired or was revoked.</p></main></body></html>"
    );
    let headers = [
        (
            CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        ),
        (CONTENT_SECURITY_POLICY, public.policy.clone()),
        (X_ROBOTS_TAG, HeaderValue::from_static("noindex")),
        (CACHE_CONTROL, HeaderValue::from_static(CACHE)),
    ];
    (StatusCode::NOT_FOUND, headers, html).into_response()
}

/// The hash of the body as served, with the one header that depends on the share alone: the
/// same bytes always get the same tag, and any change, flushed or not, a new one.
fn etag(html: &str, indexable: bool) -> String {
    let mut hash = Sha256::new();
    hash.update([u8::from(indexable)]);
    hash.update(html.as_bytes());
    let digest = hash.finalize();
    let hex: String = digest[..16].iter().map(|b| format!("{b:02x}")).collect();
    format!("\"{hex}\"")
}

/// Addresses under the share, for its pages and their files.
struct ShareLinks<'a> {
    base: String,
    root: &'a str,
    slugs: HashMap<&'a str, &'a str>,
}

impl<'a> ShareLinks<'a> {
    fn new(token: &str, root: &'a str, covered: &'a [SharedPage]) -> Self {
        Self {
            base: format!("/s/{}", utf8_percent_encode(token, SEGMENT)),
            root,
            slugs: covered
                .iter()
                .map(|page| (page.id.as_str(), page.slug.as_str()))
                .collect(),
        }
    }

    fn to_page(&self, id: &str, slug: &str) -> String {
        if id == self.root {
            self.base.clone()
        } else {
            format!("{}/p/{}", self.base, utf8_percent_encode(slug, SEGMENT))
        }
    }
}

impl Links for ShareLinks<'_> {
    fn attachment(&self, id: &str) -> String {
        format!("{}/files/{}", self.base, utf8_percent_encode(id, SEGMENT))
    }

    /// Only called for a covered page, the only ones with a title.
    fn page(&self, id: &str) -> String {
        match self.slugs.get(id) {
            Some(slug) => self.to_page(id, slug),
            None => self.base.clone(),
        }
    }
}

fn title_or_untitled(title: &str) -> &str {
    if title.trim().is_empty() {
        "Untitled"
    } else {
        title
    }
}

fn document_html(view: &SharedView, links: &ShareLinks, title: &str, content: &str) -> String {
    let title = title_or_untitled(title);
    let mut out = String::with_capacity(content.len() + STYLE.len() + 1024);
    out.push_str(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <meta name=\"viewport\" content=\"width=device-width, initial-scale=1\"><title>",
    );
    escape(title, &mut out);
    let _ = write!(out, "</title><style>{STYLE}</style></head><body><main>");
    if !view.trail.is_empty() {
        out.push_str(r#"<nav class="trail" aria-label="breadcrumb">"#);
        for (index, page) in view.trail.iter().enumerate() {
            if index > 0 {
                out.push_str(" / ");
            }
            link(&links.to_page(&page.id, &page.slug), &page.title, &mut out);
        }
        out.push_str("</nav>");
    }
    out.push_str(r#"<article><h1 class="title">"#);
    escape(title, &mut out);
    out.push_str("</h1>");
    out.push_str(content);
    out.push_str("</article>");
    if !view.children.is_empty() {
        out.push_str(r#"<nav class="children" aria-label="pages"><h2>Pages</h2><ul>"#);
        for page in &view.children {
            out.push_str("<li>");
            link(&links.to_page(&page.id, &page.slug), &page.title, &mut out);
            out.push_str("</li>");
        }
        out.push_str("</ul></nav>");
    }
    out.push_str("</main></body></html>");
    out
}

fn link(href: &str, title: &str, out: &mut String) {
    out.push_str(r#"<a href=""#);
    escape(href, out);
    out.push_str(r#"">"#);
    escape(title_or_untitled(title), out);
    out.push_str("</a>");
}

/// As the content renderer escapes text and attributes alike.
fn escape(text: &str, out: &mut String) {
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            c => out.push(c),
        }
    }
}

/// Standard base64 with padding, for the hash in the policy: one use does not justify a crate.
fn base64(bytes: &[u8]) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let word = chunk.iter().fold(0u32, |w, b| (w << 8) | u32::from(*b));
        let word = word << (8 * (3 - chunk.len()));
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((word >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::base64;

    #[test]
    fn base64_pads_as_the_standard_does() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }
}
