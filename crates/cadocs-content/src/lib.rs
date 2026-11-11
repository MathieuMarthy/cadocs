//! Page content without a browser (ADR 0008): the yrs `XmlFragment` the editor writes, read into
//! a small AST and written back from it, and renderers from the AST to plain text, HTML and
//! Markdown, with Markdown import.
//!
//! The schema is defined once, in [`schema`], and a test compares it with the editor's.

pub mod ast;
pub mod html;
pub mod markdown;
pub mod schema;
pub mod text;
pub mod url;
pub mod yjs;

mod headings;

/// Where the server serves an attachment, followed by its id. Renderers link there, and
/// Markdown import reads an image or a file from it.
pub const ATTACHMENT_PATH: &str = "/api/v1/attachments/";
/// Where the front end opens a page, followed by its id.
pub const PAGE_PATH: &str = "/?page=";

/// Where Markdown points links to attachments and pages, and how it reads them back. The defaults
/// are the server's own addresses, relative to its root ([`Relative`]). An export elsewhere, a zip
/// of files for instance, chooses others.
pub trait Links {
    fn attachment(&self, id: &str) -> String {
        format!("{ATTACHMENT_PATH}{id}")
    }

    fn page(&self, id: &str) -> String {
        format!("{PAGE_PATH}{id}")
    }

    /// The attachment an address points to, if it is one.
    fn attachment_id(&self, href: &str) -> Option<String> {
        relative_attachment_id(href)
    }

    /// The page an address points to, if it is one.
    fn page_id(&self, href: &str) -> Option<String> {
        relative_page_id(href)
    }
}

/// The server's own addresses.
pub struct Relative;

impl Links for Relative {}

/// The id in `/api/v1/attachments/<id>`.
pub fn relative_attachment_id(href: &str) -> Option<String> {
    let id = href.strip_prefix(ATTACHMENT_PATH)?;
    (!id.is_empty() && !id.contains(['/', '?', '#'])).then(|| id.to_string())
}

/// The id in `/?page=<id>`.
pub fn relative_page_id(href: &str) -> Option<String> {
    let id = href.strip_prefix(PAGE_PATH)?;
    (!id.is_empty()).then(|| id.to_string())
}

/// Looks up the title of a page that content links to, as the reader may see it: `None` for a
/// page the reader cannot open or that does not exist, which renderers show without a title.
pub trait PageTitles {
    fn title(&self, id: &str) -> Option<String>;
}

impl<F: Fn(&str) -> Option<String>> PageTitles for F {
    fn title(&self, id: &str) -> Option<String> {
        self(id)
    }
}
