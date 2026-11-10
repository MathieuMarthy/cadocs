//! Heading numbers, as the editor shows them (`headingNumbers` in `web/src/editor/source.ts`):
//! every heading in document order, ranked among the levels the page uses, `01`, `01.1`...

use crate::ast::{Block, Inline};

pub(crate) struct Numbered<'a> {
    pub label: String,
    /// How many parents the heading has in the numbering, 0 for `01`.
    pub depth: usize,
    pub content: &'a [Inline],
}

fn collect<'a>(blocks: &'a [Block], found: &mut Vec<(u8, &'a [Inline])>) {
    for block in blocks {
        match block {
            Block::Heading { level, content } => found.push((*level, content)),
            Block::BulletList(items) | Block::OrderedList { items, .. } => {
                items.iter().for_each(|item| collect(&item.blocks, found))
            }
            Block::TaskList(items) => items.iter().for_each(|item| collect(&item.blocks, found)),
            Block::Blockquote(blocks) | Block::Callout { blocks, .. } => collect(blocks, found),
            Block::Details { content, .. } => collect(content, found),
            Block::Table(rows) => rows
                .iter()
                .flat_map(|row| &row.cells)
                .for_each(|cell| collect(&cell.blocks, found)),
            Block::Paragraph(_)
            | Block::CodeBlock { .. }
            | Block::HorizontalRule
            | Block::TableOfContents
            | Block::Plugin { .. } => {}
        }
    }
}

/// Every heading of `blocks` with its number, in document order.
pub(crate) fn numbered(blocks: &[Block]) -> Vec<Numbered<'_>> {
    let mut found = Vec::new();
    collect(blocks, &mut found);
    let mut levels: Vec<u8> = found.iter().map(|(level, _)| *level).collect();
    levels.sort_unstable();
    levels.dedup();
    let mut counts: Vec<u32> = Vec::new();
    found
        .into_iter()
        .map(|(level, content)| {
            let rank = levels.iter().position(|l| *l == level).unwrap_or(0);
            counts.resize(rank + 1, 0);
            counts[rank] += 1;
            let mut label = format!("{:02}", counts[0]);
            for count in &counts[1..] {
                label.push_str(&format!(".{count}"));
            }
            Numbered {
                label,
                depth: rank,
                content,
            }
        })
        .collect()
}
