//! HTML for public pages and exports, built from the AST with escaping at every step (ADR 0008).
//! Content comes from other users: no attribute value or text reaches the output unescaped, link
//! addresses go through [`crate::url::allowed`], and nothing writes a `style` or event attribute.
//! A stored link `class`, `target` or `rel` is not rendered.

use std::fmt::Write;

use crate::{ATTACHMENT_PATH, PageTitles};
use crate::ast::*;
use crate::headings::numbered;
use crate::url::allowed;

pub fn to_html(document: &Document, titles: &dyn PageTitles) -> String {
    let mut out = String::new();
    let mut context = Context {
        titles,
        headings: numbered(&document.blocks)
            .into_iter()
            .map(|h| h.label)
            .collect(),
        toc: &document.blocks,
        next_heading: 0,
    };
    blocks(&mut context, &document.blocks, &mut out);
    out
}

/// Escapes text and attribute values alike.
pub(crate) fn escape(text: &str, out: &mut String) {
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

/// Percent-encodes everything but unreserved characters, for a page id in an address.
pub(crate) fn encode_component(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.~".contains(&byte) {
            out.push(byte as char);
        } else {
            let _ = write!(out, "%{byte:02X}");
        }
    }
    out
}

/// A value safe in a class name or a `data-` attribute keyword, or nothing.
fn keyword(value: &str) -> Option<&str> {
    let ok = !value.is_empty()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"-_+#".contains(&b));
    ok.then_some(value)
}

struct Context<'a> {
    titles: &'a dyn PageTitles,
    /// The labels of the headings, in document order.
    headings: Vec<String>,
    toc: &'a [Block],
    next_heading: usize,
}

fn blocks(context: &mut Context, list: &[Block], out: &mut String) {
    for block in list {
        self::block(context, block, out);
    }
}

fn items(context: &mut Context, list: &[ListItem], out: &mut String) {
    for item in list {
        out.push_str("<li>");
        blocks(context, &item.blocks, out);
        out.push_str("</li>");
    }
}

fn block(context: &mut Context, block: &Block, out: &mut String) {
    match block {
        Block::Paragraph(content) => {
            out.push_str("<p>");
            inlines(context, content, out);
            out.push_str("</p>");
        }
        Block::Heading { level, content } => {
            let level = (*level).clamp(1, 6);
            let label = context.headings.get(context.next_heading).cloned();
            context.next_heading += 1;
            let _ = write!(out, "<h{level}");
            if let Some(label) = label {
                let _ = write!(out, r#" id="h-{label}""#);
            }
            out.push('>');
            inlines(context, content, out);
            let _ = write!(out, "</h{level}>");
        }
        Block::BulletList(list) => {
            out.push_str("<ul>");
            items(context, list, out);
            out.push_str("</ul>");
        }
        Block::OrderedList {
            start,
            kind,
            items: list,
        } => {
            out.push_str("<ol");
            if *start != 1 {
                let _ = write!(out, r#" start="{start}""#);
            }
            if let Some(kind) = kind
                .as_deref()
                .filter(|k| ["1", "a", "A", "i", "I"].contains(k))
            {
                let _ = write!(out, r#" type="{kind}""#);
            }
            out.push('>');
            items(context, list, out);
            out.push_str("</ol>");
        }
        Block::TaskList(list) => {
            out.push_str(r#"<ul class="tasks">"#);
            for item in list {
                let _ = write!(
                    out,
                    r#"<li class="task" data-checked="{}"><input type="checkbox"{} disabled> <div>"#,
                    item.checked,
                    if item.checked { " checked" } else { "" }
                );
                blocks(context, &item.blocks, out);
                out.push_str("</div></li>");
            }
            out.push_str("</ul>");
        }
        Block::CodeBlock { language, code } => {
            out.push_str("<pre><code");
            if let Some(language) = language.as_deref().and_then(keyword) {
                let _ = write!(out, r#" class="language-{language}""#);
            }
            out.push('>');
            escape(code, out);
            out.push_str("</code></pre>");
        }
        Block::Blockquote(inner) => {
            out.push_str("<blockquote>");
            blocks(context, inner, out);
            out.push_str("</blockquote>");
        }
        Block::HorizontalRule => out.push_str("<hr>"),
        Block::Callout {
            kind,
            blocks: inner,
        } => {
            let kind = keyword(kind).unwrap_or("note");
            let _ = write!(out, r#"<aside class="callout" data-kind="{kind}">"#);
            blocks(context, inner, out);
            out.push_str("</aside>");
        }
        Block::Details { summary, content } => {
            out.push_str("<details><summary>");
            inlines(context, summary, out);
            out.push_str("</summary>");
            blocks(context, content, out);
            out.push_str("</details>");
        }
        Block::Table(rows) => {
            out.push_str("<table><tbody>");
            for row in rows {
                out.push_str("<tr>");
                for cell in &row.cells {
                    let tag = if cell.header { "th" } else { "td" };
                    let _ = write!(out, "<{tag}");
                    if cell.colspan > 1 {
                        let _ = write!(out, r#" colspan="{}""#, cell.colspan);
                    }
                    if cell.rowspan > 1 {
                        let _ = write!(out, r#" rowspan="{}""#, cell.rowspan);
                    }
                    if let Some(align) = cell
                        .align
                        .as_deref()
                        .filter(|a| ["left", "center", "right"].contains(a))
                    {
                        let _ = write!(out, r#" data-align="{align}""#);
                    }
                    out.push('>');
                    blocks(context, &cell.blocks, out);
                    let _ = write!(out, "</{tag}>");
                }
                out.push_str("</tr>");
            }
            out.push_str("</tbody></table>");
        }
        Block::TableOfContents => toc(context, out),
        Block::Image { id, alt } => {
            let _ = write!(
                out,
                r#"<figure class="image"><img src="{ATTACHMENT_PATH}{}" alt=""#,
                encode_component(id)
            );
            escape(alt.as_deref().unwrap_or_default(), out);
            out.push_str(r#""></figure>"#);
        }
        Block::File { id, name } => {
            let _ = write!(
                out,
                r#"<p class="file"><a class="file" href="{ATTACHMENT_PATH}{}" download>"#,
                encode_component(id)
            );
            escape(name, out);
            out.push_str("</a></p>");
        }
        Block::Plugin { kind, text, .. } => {
            out.push_str(r#"<div class="plugin" data-kind=""#);
            escape(kind, out);
            out.push_str(r#"">"#);
            for (index, line) in text.split('\n').enumerate() {
                if index > 0 {
                    out.push_str("<br>");
                }
                escape(line, out);
            }
            out.push_str("</div>");
        }
    }
}

fn toc(context: &mut Context, out: &mut String) {
    out.push_str(r#"<nav class="toc" aria-label="table of contents"><ol>"#);
    let headings = numbered(context.toc);
    for heading in &headings {
        let _ = write!(
            out,
            r##"<li data-depth="{}"><a href="#h-{}"><span class="toc-number">{}</span> "##,
            heading.depth, heading.label, heading.label
        );
        // Marks and links would nest links: the list shows the text alone.
        escape(&crate::text::inline_text(heading.content), out);
        out.push_str("</a></li>");
    }
    out.push_str("</ol></nav>");
}

fn inlines(context: &Context, content: &[Inline], out: &mut String) {
    for inline in content {
        match inline {
            Inline::Text { text, marks } => {
                let href = marks
                    .link
                    .as_ref()
                    .and_then(|link| allowed(&link.href).map(|h| (h, link)));
                if let Some((href, link)) = href {
                    out.push_str(r#"<a href=""#);
                    escape(href, out);
                    out.push('"');
                    if let Some(title) = &link.title {
                        out.push_str(r#" title=""#);
                        escape(title, out);
                        out.push('"');
                    }
                    out.push_str(r#" rel="noopener noreferrer nofollow">"#);
                }
                let tags: Vec<&str> = [
                    (marks.bold, "strong"),
                    (marks.italic, "em"),
                    (marks.strike, "s"),
                    (marks.underline, "u"),
                    (marks.code, "code"),
                ]
                .into_iter()
                .filter_map(|(on, tag)| on.then_some(tag))
                .collect();
                for tag in &tags {
                    let _ = write!(out, "<{tag}>");
                }
                escape(text, out);
                for tag in tags.iter().rev() {
                    let _ = write!(out, "</{tag}>");
                }
                if href.is_some() {
                    out.push_str("</a>");
                }
            }
            Inline::HardBreak => out.push_str("<br>"),
            Inline::PageLink { id } => match context.titles.title(id) {
                Some(title) => {
                    let _ = write!(
                        out,
                        r#"<a class="page-link" href="/?page={}">"#,
                        encode_component(id)
                    );
                    escape(&title, out);
                    out.push_str("</a>");
                }
                None => out
                    .push_str(r#"<span class="page-link page-link-missing">page not found</span>"#),
            },
        }
    }
}
