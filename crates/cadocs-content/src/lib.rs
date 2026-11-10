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
