//! Plain text, for search and previews: one line per text block, marks left out.
//!
//! A page link adds nothing: its title belongs to another page, which a reader of this one may
//! not be allowed to see, and search must not find this page by it.

use crate::ast::{Block, Document, Inline};

pub fn to_text(document: &Document) -> String {
    let mut out = String::new();
    blocks(&document.blocks, &mut out);
    out.truncate(out.trim_end().len());
    out
}

fn line(out: &mut String, text: &str) {
    out.push_str(text);
    out.push('\n');
}

pub(crate) fn inline_text(content: &[Inline]) -> String {
    let mut text = String::new();
    for inline in content {
        match inline {
            Inline::Text { text: chunk, .. } => text.push_str(chunk),
            Inline::HardBreak => text.push('\n'),
            Inline::PageLink { .. } => {}
        }
    }
    text
}

fn blocks(list: &[Block], out: &mut String) {
    for block in list {
        match block {
            Block::Paragraph(content) | Block::Heading { content, .. } => {
                line(out, &inline_text(content))
            }
            Block::BulletList(items) | Block::OrderedList { items, .. } => {
                items.iter().for_each(|item| blocks(&item.blocks, out))
            }
            Block::TaskList(items) => items.iter().for_each(|item| blocks(&item.blocks, out)),
            Block::CodeBlock { code, .. } => line(out, code),
            Block::Blockquote(inner) | Block::Callout { blocks: inner, .. } => blocks(inner, out),
            Block::Details { summary, content } => {
                line(out, &inline_text(summary));
                blocks(content, out);
            }
            Block::Table(rows) => rows
                .iter()
                .flat_map(|row| &row.cells)
                .for_each(|cell| blocks(&cell.blocks, out)),
            Block::Plugin { text, .. } => line(out, text),
            Block::HorizontalRule | Block::TableOfContents => {}
        }
    }
}
