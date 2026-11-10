//! Markdown export and import, through the AST (ADR 0008). GitHub Flavored Markdown, with these
//! conventions for what CommonMark lacks:
//!
//! | Node | Markdown |
//! | --- | --- |
//! | callout | GitHub alert: `> [!NOTE]`, `[!WARNING]`, `[!CAUTION]` for `note`, `warning`, `danger` |
//! | toggle | `<details>`, `<summary>` and `</summary>`, `</details>`, each on its own line |
//! | table of contents | `[TOC]` alone in a paragraph |
//! | link to a page | `[title](/?page=<id>)` |
//! | image | `![alt](/api/v1/attachments/<id>)` alone in a paragraph |
//! | file | `[name](/api/v1/attachments/<id>)` alone in a paragraph |
//! | underline | `<u>` and `</u>` |
//! | empty paragraph | `<br>` alone |
//! | bold, italic, strike where `*` or `~` would not be read (`foo**(bar)**baz`) | `<strong>`, `<em>`, `<s>` |
//! | spaces or tabs at the edge of a line | `&#32;`, `&#9;` |
//!
//! What is lost: a plugin block becomes its text, a table's spans and its first row's header
//! flags (the first row is always the header), several blocks in a table cell become one line
//! joined by `<br>`, an ordered list's `type`. Import reads `[!TIP]` and `[!IMPORTANT]` as notes,
//! headings below level 3 as level 3, any other image as its alternative text, and any other HTML
//! as text. A link whose address [`crate::url::allowed`] refuses keeps only its text both ways.

use std::fmt::Write;

use pulldown_cmark::{
    Alignment, BlockQuoteKind, CodeBlockKind, Event, Options, Parser, Tag, TagEnd,
};

use crate::ast::*;
use crate::url::allowed;
use crate::{ATTACHMENT_PATH, PageTitles};

const TOC: &str = "[TOC]";
const PAGE_PREFIX: &str = "/?page=";
const EMPTY: &str = "<br>";

// Export.

pub fn to_markdown(document: &Document, titles: &dyn PageTitles) -> String {
    let out = blocks(titles, &document.blocks);
    if out.is_empty() { out } else { out + "\n" }
}

/// Which lists would merge when written one after the other with the same marker.
fn list_family(block: &Block) -> u8 {
    match block {
        Block::BulletList(_) | Block::TaskList(_) => 1,
        Block::OrderedList { .. } => 2,
        _ => 0,
    }
}

/// Whether a paragraph followed by `block` on the next line still ends before it: bullet lists
/// interrupt a paragraph, ordered lists only from 1.
fn interrupts(block: &Block) -> bool {
    matches!(
        block,
        Block::BulletList(_) | Block::TaskList(_) | Block::OrderedList { start: 1, .. }
    )
}

/// Blocks separated by a blank line, without a final line break. A list right after a list of
/// its family takes the other marker (`*` or `)`), or Markdown would read one list. In a list
/// item (`tight`), a list right after a paragraph follows it on the next line.
fn blocks_in(titles: &dyn PageTitles, list: &[Block], tight: bool) -> String {
    let mut out = String::new();
    let mut alternate = false;
    for (index, current) in list.iter().enumerate() {
        let family = list_family(current);
        alternate =
            index > 0 && family != 0 && list_family(&list[index - 1]) == family && !alternate;
        if index > 0 {
            // Not after an empty paragraph: its `<br>` opens an HTML block that would take the list.
            let follows = tight
                && matches!(&list[index - 1], Block::Paragraph(content) if !content.is_empty())
                && interrupts(current);
            out.push_str(if follows { "\n" } else { "\n\n" });
        }
        out.push_str(&block(titles, current, alternate));
    }
    out
}

fn blocks(titles: &dyn PageTitles, list: &[Block]) -> String {
    blocks_in(titles, list, false)
}

/// `text` with `first` before its first line and `rest` before the others, blank lines kept
/// blank unless `rest` is a quote marker.
fn prefixed(text: &str, first: &str, rest: &str) -> String {
    let mut out = String::new();
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        let prefix = if index == 0 { first } else { rest };
        if line.is_empty() {
            out.push_str(prefix.trim_end());
        } else {
            out.push_str(prefix);
            out.push_str(line);
        }
    }
    out
}

fn quoted(text: &str) -> String {
    prefixed(text, "> ", "> ")
}

fn list_item(titles: &dyn PageTitles, marker: &str, list: &[Block]) -> String {
    let indent = " ".repeat(marker.len());
    prefixed(&blocks_in(titles, list, true), marker, &indent)
}

fn block(titles: &dyn PageTitles, block: &Block, alternate: bool) -> String {
    let bullet = if alternate { "*" } else { "-" };
    let number = if alternate { ")" } else { "." };
    match block {
        // `<br>` alone stands for an empty paragraph, which Markdown has no other way to keep.
        Block::Paragraph(content) if content.is_empty() => EMPTY.into(),
        // `[TOC]` written by a user comes out escaped, so it stays text.
        Block::Paragraph(content) => inlines(titles, content, Flow::Block),
        Block::Heading { level, content } => {
            format!(
                "{} {}",
                "#".repeat((*level).clamp(1, 6).into()),
                inlines(titles, content, Flow::Line)
            )
        }
        Block::BulletList(items) => items
            .iter()
            .map(|item| list_item(titles, &format!("{bullet} "), &item.blocks))
            .collect::<Vec<_>>()
            .join("\n"),
        Block::OrderedList { start, items, .. } => items
            .iter()
            .enumerate()
            .map(|(index, item)| {
                list_item(
                    titles,
                    // `-1.` would be a bullet: Markdown lists start at 0 or more.
                    &format!("{}{number} ", (*start).max(0) + index as i64),
                    &item.blocks,
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Block::TaskList(items) => items
            .iter()
            .map(|item| {
                let marker = format!("{bullet} [{}] ", if item.checked { "x" } else { " " });
                // The continuation lines align on the list marker, not on the box.
                prefixed(&blocks_in(titles, &item.blocks, true), &marker, "  ")
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Block::CodeBlock { language, code } => {
            let longest = longest_run(code, '`');
            let fence = "`".repeat(longest.max(2) + 1);
            let info = language.as_deref().unwrap_or("").replace(['`', '\n'], "");
            format!("{fence}{info}\n{code}\n{fence}")
        }
        Block::Blockquote(inner) => quoted(&blocks(titles, inner)),
        Block::HorizontalRule => "---".into(),
        Block::Callout {
            kind,
            blocks: inner,
        } => {
            let alert = match kind.as_str() {
                "warning" => "WARNING",
                "danger" => "CAUTION",
                _ => "NOTE",
            };
            quoted(&format!("[!{alert}]\n{}", blocks(titles, inner)))
        }
        Block::Details { summary, content } => format!(
            "<details>\n<summary>\n\n{}\n\n</summary>\n\n{}\n\n</details>",
            inlines(titles, summary, Flow::Block),
            blocks(titles, content)
        ),
        Block::Table(rows) => table(titles, rows),
        Block::TableOfContents => TOC.into(),
        Block::Image { id, alt } => {
            let mut out = "![".to_string();
            escape(alt.as_deref().unwrap_or_default(), false, &mut out);
            let _ = write!(out, "]({})", destination(&format!("{ATTACHMENT_PATH}{id}")));
            out
        }
        Block::File { id, name } => {
            let mut out = "[".to_string();
            escape(name, false, &mut out);
            let _ = write!(out, "]({})", destination(&format!("{ATTACHMENT_PATH}{id}")));
            out
        }
        Block::Plugin { text, .. } => {
            let content: Vec<Inline> = text
                .split('\n')
                .enumerate()
                .flat_map(|(index, line)| {
                    let text = Inline::Text {
                        text: line.into(),
                        marks: Marks::default(),
                    };
                    if index == 0 {
                        vec![text]
                    } else {
                        vec![Inline::HardBreak, text]
                    }
                })
                .collect();
            inlines(titles, &content, Flow::Block)
        }
    }
}

fn longest_run(text: &str, wanted: char) -> usize {
    let (mut longest, mut current) = (0, 0);
    for c in text.chars() {
        current = if c == wanted { current + 1 } else { 0 };
        longest = longest.max(current);
    }
    longest
}

fn table(titles: &dyn PageTitles, rows: &[TableRow]) -> String {
    let width = rows.iter().map(|row| row.cells.len()).max().unwrap_or(0);
    if width == 0 {
        return String::new();
    }
    let line = |cells: Vec<String>| format!("| {} |", cells.join(" | "));
    let row_cells = |row: &TableRow| {
        let mut cells: Vec<String> = row
            .cells
            .iter()
            .map(|cell| {
                cell.blocks
                    .iter()
                    .map(|block| match block {
                        Block::Paragraph(content) | Block::Heading { content, .. } => {
                            inlines(titles, content, Flow::Cell)
                        }
                        other => {
                            let text = crate::text::to_text(&Document {
                                blocks: vec![other.clone()],
                            });
                            let content: Vec<Inline> = text
                                .split('\n')
                                .enumerate()
                                .flat_map(|(index, line)| {
                                    let text = Inline::Text {
                                        text: line.into(),
                                        marks: Marks::default(),
                                    };
                                    if index == 0 {
                                        vec![text]
                                    } else {
                                        vec![Inline::HardBreak, text]
                                    }
                                })
                                .collect();
                            inlines(titles, &content, Flow::Cell)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join("<br>")
            })
            .collect();
        cells.resize(width, String::new());
        cells
    };
    let mut out = vec![line(row_cells(&rows[0]))];
    let rule: Vec<String> = (0..width)
        .map(|index| {
            let align = rows[0]
                .cells
                .get(index)
                .and_then(|cell| cell.align.as_deref());
            match align {
                Some("left") => ":---",
                Some("center") => ":---:",
                Some("right") => "---:",
                _ => "---",
            }
            .to_string()
        })
        .collect();
    out.push(line(rule));
    out.extend(rows[1..].iter().map(|row| line(row_cells(row))));
    out.join("\n")
}

/// Where inline content goes, which decides how a line break is written.
#[derive(Clone, Copy, PartialEq)]
enum Flow {
    /// A paragraph: `\` and a new line.
    Block,
    /// A heading: a space.
    Line,
    /// A table cell: `<br>`.
    Cell,
}

#[derive(Clone, PartialEq)]
enum Delimiter {
    Link(Link),
    Bold,
    Italic,
    Strike,
    Underline,
}

impl Delimiter {
    /// `html` writes the tag instead, where Markdown's flanking rules would not read the
    /// delimiter (`foo**(bar)**baz`). Links and underline do not depend on them.
    fn open(&self, html: bool) -> String {
        match (self, html) {
            (Delimiter::Link(_), _) => "[".into(),
            (Delimiter::Bold, false) => "**".into(),
            (Delimiter::Bold, true) => "<strong>".into(),
            (Delimiter::Italic, false) => "*".into(),
            (Delimiter::Italic, true) => "<em>".into(),
            (Delimiter::Strike, false) => "~~".into(),
            (Delimiter::Strike, true) => "<s>".into(),
            (Delimiter::Underline, _) => "<u>".into(),
        }
    }

    fn close(&self, html: bool) -> String {
        match (self, html) {
            (Delimiter::Link(link), _) => {
                let mut out = String::from("](");
                out.push_str(&destination(&link.href));
                if let Some(title) = &link.title {
                    out.push_str(" \"");
                    for c in title.chars() {
                        if matches!(c, '"' | '\\' | '&') {
                            out.push('\\');
                        }
                        out.push(c);
                    }
                    out.push('"');
                }
                out.push(')');
                out
            }
            (Delimiter::Bold, false) => "**".into(),
            (Delimiter::Bold, true) => "</strong>".into(),
            (Delimiter::Italic, false) => "*".into(),
            (Delimiter::Italic, true) => "</em>".into(),
            (Delimiter::Strike, false) => "~~".into(),
            (Delimiter::Strike, true) => "</s>".into(),
            (Delimiter::Underline, _) => "</u>".into(),
        }
    }

    fn flanking(&self) -> bool {
        matches!(
            self,
            Delimiter::Bold | Delimiter::Italic | Delimiter::Strike
        )
    }
}

/// A link destination that parses back to `href`: between `<` and `>` when it holds spaces or
/// brackets, with backslashes before what would end it, and before `&`, which a parser would
/// otherwise decode as an entity (`javascript&#58;` is `javascript:` to any of them).
fn destination(href: &str) -> String {
    let angled = href.is_empty() || href.contains([' ', '(', ')', '<', '>']);
    let mut out = String::new();
    if angled {
        out.push('<');
    }
    for c in href.chars() {
        if matches!(c, '\\' | '<' | '>' | '&') {
            out.push('\\');
        }
        out.push(c);
    }
    if angled {
        out.push('>');
    }
    out
}

/// The delimiters a run of text needs, outermost first. Code is written apart, innermost.
fn delimiters(marks: &Marks) -> Vec<Delimiter> {
    let mut list = Vec::new();
    if let Some(link) = &marks.link
        && allowed(&link.href).is_some()
    {
        list.push(Delimiter::Link(link.clone()));
    }
    for (on, delimiter) in [
        (marks.bold, Delimiter::Bold),
        (marks.italic, Delimiter::Italic),
        (marks.strike, Delimiter::Strike),
        (marks.underline, Delimiter::Underline),
    ] {
        if on {
            list.push(delimiter);
        }
    }
    list
}

/// Escapes what Markdown would read as syntax. `line_start` is true at the start of a line,
/// where `-`, `+`, `=` and `1.` start blocks. `#` is always escaped, since a heading also drops
/// a final run of them.
fn escape(text: &str, line_start: bool, out: &mut String) {
    let mut at_start = line_start;
    let mut digits = 0;
    for c in text.chars() {
        let escaped = match c {
            '\\' | '`' | '*' | '_' | '[' | ']' | '<' | '>' | '~' | '|' | '&' | '!' | '#' => true,
            '-' | '+' | '=' => at_start,
            '.' | ')' => digits > 0 && digits == out_digits_prefix(out, digits),
            _ => false,
        };
        if escaped {
            out.push('\\');
        }
        out.push(c);
        if c.is_ascii_digit() && (at_start || digits > 0) {
            digits += 1;
        } else {
            digits = 0;
        }
        at_start = false;
    }
}

/// `digits` when the last `digits` characters written are digits that started the line.
fn out_digits_prefix(out: &str, digits: usize) -> usize {
    let line = out.rsplit('\n').next().unwrap_or(out);
    if line.len() == digits && line.bytes().all(|b| b.is_ascii_digit()) {
        digits
    } else {
        0
    }
}

/// Inline code. In a table cell, `|` is escaped: GFM splits cells before it reads code spans,
/// and removes the backslash.
fn code_span(text: &str, flow: Flow) -> String {
    let text = if flow == Flow::Cell {
        text.replace('|', "\\|")
    } else {
        text.to_string()
    };
    let fence = "`".repeat(longest_run(&text, '`') + 1);
    let pad = text.starts_with('`')
        || text.ends_with('`')
        || (text.starts_with(' ') && text.ends_with(' ') && !text.trim().is_empty());
    let space = if pad { " " } else { "" };
    format!("{fence}{space}{text}{space}{fence}")
}

const SPACE: [char; 2] = [' ', '\t'];

fn at_line_start(out: &str) -> bool {
    out.is_empty() || out.ends_with('\n')
}

/// Spaces and tabs. At the start or end of a line Markdown would drop them, or read an indented
/// code block, so they are written as character references there.
fn write_space(out: &mut String, space: &str, edge: bool) {
    for c in space.chars() {
        match (edge || at_line_start(out), c) {
            (true, '\t') => out.push_str("&#9;"),
            (true, _) => out.push_str("&#32;"),
            (false, c) => out.push(c),
        }
    }
}

fn is_space(c: Option<char>) -> bool {
    c.is_none_or(char::is_whitespace)
}

fn is_punctuation(c: Option<char>) -> bool {
    c.is_some_and(|c| c.is_ascii_punctuation() || (!c.is_alphanumeric() && !c.is_whitespace()))
}

/// What a delimiter around this run sees inside it: its first and last characters.
fn edges(inline: &Inline) -> (Option<char>, Option<char>) {
    match inline {
        Inline::Text { text, marks } if marks.code => (Some('`'), Some('`')),
        Inline::Text { text, .. } => {
            let core = text.trim_matches(SPACE);
            (core.chars().next(), core.chars().last())
        }
        _ => (None, None),
    }
}

/// Whether CommonMark reads `*` or `~` delimiters around the runs `first..=last` of `content`,
/// written after `before`, for a delimiter at `depth` in `runs`. Deeper delimiters open or close
/// against it, and all of them (`*`, `~`, `[`, `]`, tags) are punctuation. An approximation
/// that errs towards HTML tags.
fn flanks(
    content: &[Inline],
    runs: &[Vec<Delimiter>],
    depth: usize,
    first: usize,
    last: usize,
    before: Option<char>,
) -> bool {
    let inner = |run: usize| runs[run].len() > depth + 1;
    let start = if inner(first) {
        Some('*')
    } else {
        edges(&content[first]).0
    };
    let end = if inner(last) {
        Some('*')
    } else {
        edges(&content[last]).1
    };
    let after = match &content[last] {
        Inline::Text { text, .. } if text.ends_with(SPACE) => Some(' '),
        _ => match content.get(last + 1) {
            Some(Inline::Text { text, .. }) => text.chars().next(),
            Some(Inline::HardBreak) => Some('\n'),
            Some(Inline::PageLink { .. }) => Some('['),
            None => None,
        },
    };
    let opens =
        !is_space(start) && (!is_punctuation(start) || is_space(before) || is_punctuation(before));
    let closes =
        !is_space(end) && (!is_punctuation(end) || is_space(after) || is_punctuation(after));
    opens && closes
}

fn inlines(titles: &dyn PageTitles, content: &[Inline], flow: Flow) -> String {
    let mut out = String::new();
    // Each open delimiter, and whether it was written as an HTML tag.
    let mut open: Vec<(Delimiter, bool)> = Vec::new();
    // Spaces that ended the last run, written once its delimiters are closed.
    let mut pending = String::new();
    let close_to = |out: &mut String, open: &mut Vec<(Delimiter, bool)>, keep: usize| {
        while open.len() > keep {
            let (delimiter, html) = open.pop().expect("longer than keep");
            out.push_str(&delimiter.close(html));
        }
    };
    let runs: Vec<Vec<Delimiter>> = content
        .iter()
        .map(|inline| match inline {
            Inline::Text { marks, .. } => delimiters(marks),
            _ => Vec::new(),
        })
        .collect();
    for (index, inline) in content.iter().enumerate() {
        let wanted = &runs[index];
        let keep = match inline {
            Inline::Text { .. } => open
                .iter()
                .zip(wanted)
                .take_while(|((a, _), b)| a == *b)
                .count(),
            _ => 0,
        };
        close_to(&mut out, &mut open, keep);
        let last = index + 1 == content.len();
        let edge = matches!(inline, Inline::HardBreak) && flow == Flow::Block;
        write_space(&mut out, &pending, edge);
        pending.clear();
        match inline {
            Inline::Text { text, marks } => {
                let text = text.replace('\n', " ");
                let core = text.trim_matches(SPACE);
                if core.is_empty() {
                    pending.push_str(&text);
                    continue;
                }
                let lead = &text[..text.len() - text.trim_start_matches(SPACE).len()];
                let trail = &text[text.trim_end_matches(SPACE).len()..];
                write_space(&mut out, lead, false);
                for (depth, delimiter) in wanted.iter().enumerate().skip(keep) {
                    // The runs this delimiter will stay open over.
                    let mut end = index;
                    while end + 1 < content.len()
                        && runs[end + 1].len() > depth
                        && runs[end + 1][..=depth] == wanted[..=depth]
                    {
                        end += 1;
                    }
                    let html = delimiter.flanking()
                        && !flanks(content, &runs, depth, index, end, out.chars().last());
                    out.push_str(&delimiter.open(html));
                    open.push((delimiter.clone(), html));
                }
                if marks.code {
                    out.push_str(&code_span(core, flow));
                } else {
                    let line_start = at_line_start(&out);
                    escape(core, line_start, &mut out);
                }
                pending.push_str(trail);
            }
            Inline::HardBreak => out.push_str(match flow {
                // A backslash that ends a paragraph is only a backslash.
                Flow::Block if last => "<br>",
                Flow::Block => "\\\n",
                Flow::Line => " ",
                Flow::Cell => "<br>",
            }),
            Inline::PageLink { id } => {
                let title = titles.title(id).unwrap_or_else(|| "page not found".into());
                out.push('[');
                escape(&title, false, &mut out);
                let _ = write!(out, "]({PAGE_PREFIX}{})", destination(id));
            }
        }
    }
    close_to(&mut out, &mut open, 0);
    write_space(&mut out, &pending, true);
    out
}

// Import.

/// The id in an attachment's address, as the renderers write it.
fn attachment_id(href: &str) -> Option<String> {
    let id = href.strip_prefix(ATTACHMENT_PATH)?;
    (!id.is_empty() && !id.contains(['/', '?', '#'])).then(|| id.to_string())
}

/// A paragraph holding only a plain link to an attachment, as a file block is written.
fn file_block(content: &[Inline]) -> Option<Block> {
    let [Inline::Text { text, marks }] = content else {
        return None;
    };
    let link = marks.link.as_ref()?;
    let plain = Marks {
        link: None,
        ..marks.clone()
    };
    if !plain.is_empty() || link.title.is_some() {
        return None;
    }
    Some(Block::File {
        id: attachment_id(&link.href)?,
        name: text.clone(),
    })
}

/// The document a Markdown text describes. Never fails: what it cannot map stays as text.
pub fn from_markdown(markdown: &str) -> Document {
    let options = Options::ENABLE_TABLES
        | Options::ENABLE_STRIKETHROUGH
        | Options::ENABLE_TASKLISTS
        | Options::ENABLE_GFM;
    let mut reader = Reader::default();
    for (event, range) in Parser::new_ext(markdown, options).into_offset_iter() {
        // The table of contents is known by its source: escaped brackets read as the same text.
        if matches!(event, Event::Start(Tag::Paragraph)) {
            reader.toc = markdown[range].trim() == TOC;
        }
        reader.event(event);
    }
    reader.finish()
}

enum Container {
    Root(Vec<Block>),
    Quote {
        kind: Option<BlockQuoteKind>,
        blocks: Vec<Block>,
    },
    List {
        start: Option<u64>,
        items: Vec<(Option<bool>, Vec<Block>)>,
    },
    Item {
        task: Option<bool>,
        blocks: Vec<Block>,
    },
    Details {
        summary: Option<Vec<Inline>>,
        in_summary: bool,
        blocks: Vec<Block>,
    },
    Table {
        alignments: Vec<Alignment>,
        rows: Vec<TableRow>,
        header: bool,
    },
    Row(Vec<TableCell>),
}

impl Container {
    fn blocks(&mut self) -> Option<&mut Vec<Block>> {
        match self {
            Container::Root(blocks)
            | Container::Quote { blocks, .. }
            | Container::Item { blocks, .. }
            | Container::Details { blocks, .. } => Some(blocks),
            _ => None,
        }
    }
}

/// Inline content being read, for a paragraph, heading, cell or code block.
#[derive(Default)]
struct Inlines {
    content: Vec<Inline>,
    /// Text of a code block, or of an HTML block.
    raw: String,
}

#[derive(Default)]
struct Reader {
    stack: Vec<Container>,
    inline: Option<Inlines>,
    /// Paragraph opened for text a tight list item holds directly.
    implicit: bool,
    /// Reading a code or HTML block, whose text is raw.
    raw: bool,
    code_language: Option<String>,
    /// The paragraph being read is `[TOC]`, unescaped.
    toc: bool,
    bold: usize,
    italic: usize,
    strike: usize,
    underline: usize,
    links: Vec<Option<Link>>,
    /// Inside a link to a page: its text is replaced by the link.
    page: Option<String>,
    image: usize,
    /// The inline content being read is a paragraph's, not a heading's or a cell's.
    in_paragraph: bool,
    /// Inside an attachment image that opened its paragraph.
    attachment_image: Option<String>,
    /// The attachment image that opened the paragraph, with its alternative text: the paragraph
    /// becomes an image block if nothing follows.
    lone_image: Option<(String, String)>,
}

impl Reader {
    fn push_block(&mut self, block: Block) {
        if self.stack.is_empty() {
            self.stack.push(Container::Root(Vec::new()));
        }
        // A summary takes the text of paragraphs and headings read between its tags, and the
        // other blocks go to the toggle's content.
        if let Some(Container::Details {
            summary,
            in_summary: true,
            blocks,
        }) = self.stack.last_mut()
        {
            match block {
                Block::Paragraph(content) | Block::Heading { content, .. } => {
                    let summary = summary.get_or_insert_with(Vec::new);
                    if !summary.is_empty() {
                        push_inline(
                            summary,
                            Inline::Text {
                                text: " ".into(),
                                marks: Marks::default(),
                            },
                        );
                    }
                    for inline in content {
                        push_inline(summary, inline);
                    }
                }
                other => blocks.push(other),
            }
            return;
        }
        if let Some(blocks) = self.stack.last_mut().and_then(Container::blocks) {
            blocks.push(block);
        }
    }

    fn marks(&self) -> Marks {
        Marks {
            bold: self.bold > 0,
            italic: self.italic > 0,
            strike: self.strike > 0,
            underline: self.underline > 0,
            code: false,
            link: self.links.iter().rev().flatten().next().cloned(),
        }
    }

    fn inline(&mut self, inline: Inline) {
        if self.page.is_some() {
            return;
        }
        if self.inline.is_none() {
            // Text right in a tight list item, or a stray one: it gets a paragraph.
            self.inline = Some(Inlines::default());
            self.implicit = true;
            self.in_paragraph = false;
        }
        if let Some(current) = &mut self.inline {
            push_inline(&mut current.content, inline);
        }
    }

    fn text(&mut self, text: &str, code: bool) {
        let mut marks = self.marks();
        marks.code = code;
        self.inline(Inline::Text {
            text: text.into(),
            marks,
        });
    }

    /// Ends the paragraph opened for text a tight list item held.
    fn close_implicit(&mut self) {
        if self.implicit {
            self.implicit = false;
            let content = self.inline.take().map(|i| i.content).unwrap_or_default();
            self.paragraph(content);
        }
    }

    fn paragraph(&mut self, content: Vec<Inline>) {
        let toc = std::mem::take(&mut self.toc);
        let block = if toc {
            Block::TableOfContents
        } else if let Some((id, alt)) = self.lone_image.take() {
            if content.is_empty() {
                let alt = (!alt.is_empty()).then_some(alt);
                Block::Image { id, alt }
            } else {
                // Text follows: the image stays its alternative text, as any other.
                let mut joined = Vec::new();
                let text = Inline::Text {
                    text: alt,
                    marks: Marks::default(),
                };
                push_inline(&mut joined, text);
                content
                    .into_iter()
                    .for_each(|inline| push_inline(&mut joined, inline));
                Block::Paragraph(joined)
            }
        } else {
            file_block(&content).unwrap_or(Block::Paragraph(content))
        };
        self.push_block(block);
    }

    fn event(&mut self, event: Event) {
        match event {
            Event::Start(tag) => self.start(tag),
            Event::End(tag) => self.end(tag),
            Event::Text(text) => {
                if self.raw {
                    if let Some(current) = &mut self.inline {
                        current.raw.push_str(&text);
                    }
                } else {
                    self.text(&text, false)
                }
            }
            Event::Code(code) => self.text(&code, true),
            Event::SoftBreak => self.text(" ", false),
            Event::HardBreak => self.inline(Inline::HardBreak),
            Event::Html(html) => {
                if let Some(current) = &mut self.inline {
                    current.raw.push_str(&html);
                }
            }
            Event::InlineHtml(html) => self.inline_html(&html),
            Event::TaskListMarker(checked) => {
                if let Some(Container::Item { task, .. }) = self.stack.last_mut() {
                    *task = Some(checked);
                }
            }
            Event::Rule => {
                self.close_implicit();
                self.push_block(Block::HorizontalRule);
            }
            _ => {}
        }
    }

    fn inline_html(&mut self, html: &str) {
        let tag: String = html
            .chars()
            .filter(|c| !c.is_whitespace())
            .collect::<String>()
            .to_ascii_lowercase();
        match tag.as_str() {
            "<u>" => self.underline += 1,
            "</u>" => self.underline = self.underline.saturating_sub(1),
            "<strong>" | "<b>" => self.bold += 1,
            "</strong>" | "</b>" => self.bold = self.bold.saturating_sub(1),
            "<em>" | "<i>" => self.italic += 1,
            "</em>" | "</i>" => self.italic = self.italic.saturating_sub(1),
            "<s>" | "<del>" => self.strike += 1,
            "</s>" | "</del>" => self.strike = self.strike.saturating_sub(1),
            "<br>" | "<br/>" => self.inline(Inline::HardBreak),
            _ => self.text(html, false),
        }
    }

    fn start(&mut self, tag: Tag) {
        let block_level = !matches!(
            tag,
            Tag::Emphasis | Tag::Strong | Tag::Strikethrough | Tag::Link { .. } | Tag::Image { .. }
        );
        if block_level {
            self.close_implicit();
        }
        match tag {
            Tag::Paragraph | Tag::Heading { .. } | Tag::TableCell => {
                self.in_paragraph = matches!(tag, Tag::Paragraph);
                self.lone_image = None;
                self.inline = Some(Inlines::default());
            }
            Tag::CodeBlock(kind) => {
                self.inline = Some(Inlines::default());
                self.raw = true;
                self.code_language = match kind {
                    CodeBlockKind::Fenced(info) => {
                        info.split_whitespace().next().map(str::to_string)
                    }
                    CodeBlockKind::Indented => None,
                };
            }
            Tag::HtmlBlock => {
                self.inline = Some(Inlines::default());
                self.raw = true;
            }
            Tag::BlockQuote(kind) => self.stack.push(Container::Quote {
                kind,
                blocks: Vec::new(),
            }),
            Tag::List(start) => self.stack.push(Container::List {
                start,
                items: Vec::new(),
            }),
            Tag::Item => self.stack.push(Container::Item {
                task: None,
                blocks: Vec::new(),
            }),
            Tag::Table(alignments) => self.stack.push(Container::Table {
                alignments,
                rows: Vec::new(),
                header: false,
            }),
            Tag::TableHead => {
                if let Some(Container::Table { header, .. }) = self.stack.last_mut() {
                    *header = true;
                }
                self.stack.push(Container::Row(Vec::new()));
            }
            Tag::TableRow => self.stack.push(Container::Row(Vec::new())),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link {
                dest_url, title, ..
            } => {
                if let Some(id) = dest_url
                    .strip_prefix(PAGE_PREFIX)
                    .filter(|id| !id.is_empty())
                {
                    self.page = Some(id.to_string());
                    self.links.push(None);
                } else {
                    let title = (!title.is_empty()).then(|| title.to_string());
                    let link = allowed(&dest_url).map(|href| Link::new(href, title));
                    self.links.push(link);
                }
            }
            Tag::Image { dest_url, .. } => {
                self.image += 1;
                // A second image after a lone one: both stay text, even an empty one.
                let after_lone = self.lone_image.take().map(|(_, alt)| alt);
                if let Some(alt) = &after_lone {
                    self.text(alt, false);
                }
                let opens = after_lone.is_none()
                    && self.in_paragraph
                    && self.image == 1
                    && self.links.is_empty()
                    && self.inline.as_ref().is_some_and(|i| i.content.is_empty());
                self.attachment_image = attachment_id(&dest_url).filter(|_| opens);
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => {
                let content = self.inline.take().map(|i| i.content).unwrap_or_default();
                self.paragraph(content);
            }
            TagEnd::Heading(level) => {
                let content = self.inline.take().map(|i| i.content).unwrap_or_default();
                let level = (level as u8).min(3);
                // A heading holds no line break.
                let content = content
                    .into_iter()
                    .map(|inline| match inline {
                        Inline::HardBreak => Inline::Text {
                            text: " ".into(),
                            marks: Marks::default(),
                        },
                        other => other,
                    })
                    .fold(Vec::new(), |mut list, inline| {
                        push_inline(&mut list, inline);
                        list
                    });
                self.push_block(Block::Heading { level, content });
            }
            TagEnd::CodeBlock => {
                self.raw = false;
                let raw = self.inline.take().map(|i| i.raw).unwrap_or_default();
                let code = raw.strip_suffix('\n').unwrap_or(&raw).to_string();
                let language = self.code_language.take();
                self.push_block(Block::CodeBlock { language, code });
            }
            TagEnd::HtmlBlock => {
                self.raw = false;
                let raw = self.inline.take().map(|i| i.raw).unwrap_or_default();
                self.html_block(&raw);
            }
            TagEnd::BlockQuote(_) => {
                self.close_implicit();
                if let Some(Container::Quote { kind, blocks }) = self.stack.pop() {
                    let block = match kind {
                        None => Block::Blockquote(blocks),
                        Some(kind) => Block::Callout {
                            kind: match kind {
                                BlockQuoteKind::Warning => "warning",
                                BlockQuoteKind::Caution => "danger",
                                _ => "note",
                            }
                            .into(),
                            blocks,
                        },
                    };
                    self.push_block(block);
                }
            }
            TagEnd::Item => {
                self.close_implicit();
                if let Some(Container::Item { task, mut blocks }) = self.stack.pop() {
                    if !matches!(blocks.first(), Some(Block::Paragraph(_))) {
                        blocks.insert(0, Block::Paragraph(Vec::new()));
                    }
                    if let Some(Container::List { items, .. }) = self.stack.last_mut() {
                        items.push((task, blocks));
                    }
                }
            }
            TagEnd::List(_) => {
                if let Some(Container::List { start, items }) = self.stack.pop() {
                    let block = if items.iter().any(|(task, _)| task.is_some()) {
                        Block::TaskList(
                            items
                                .into_iter()
                                .map(|(task, blocks)| TaskItem {
                                    checked: task == Some(true),
                                    blocks,
                                })
                                .collect(),
                        )
                    } else {
                        let items = items
                            .into_iter()
                            .map(|(_, blocks)| ListItem { blocks })
                            .collect();
                        match start {
                            Some(start) => Block::OrderedList {
                                start: i64::try_from(start).unwrap_or(1),
                                kind: None,
                                items,
                            },
                            None => Block::BulletList(items),
                        }
                    };
                    self.push_block(block);
                }
            }
            TagEnd::TableCell => {
                let content = self.inline.take().map(|i| i.content).unwrap_or_default();
                let header = self.stack.iter().rev().find_map(|c| match c {
                    Container::Table { header, .. } => Some(*header),
                    _ => None,
                });
                let alignments = self.stack.iter().rev().find_map(|c| match c {
                    Container::Table { alignments, .. } => Some(alignments.clone()),
                    _ => None,
                });
                if let Some(Container::Row(cells)) = self.stack.last_mut() {
                    let align = alignments
                        .and_then(|a| a.get(cells.len()).copied())
                        .and_then(|a| match a {
                            Alignment::Left => Some("left".to_string()),
                            Alignment::Center => Some("center".to_string()),
                            Alignment::Right => Some("right".to_string()),
                            Alignment::None => None,
                        });
                    cells.push(TableCell {
                        header: header.unwrap_or(false),
                        colspan: 1,
                        rowspan: 1,
                        colwidth: None,
                        align,
                        blocks: vec![Block::Paragraph(content)],
                    });
                }
            }
            TagEnd::TableHead | TagEnd::TableRow => {
                if let Some(Container::Row(cells)) = self.stack.pop()
                    && let Some(Container::Table { rows, header, .. }) = self.stack.last_mut()
                {
                    rows.push(TableRow { cells });
                    *header = false;
                }
            }
            TagEnd::Table => {
                if let Some(Container::Table { rows, .. }) = self.stack.pop() {
                    self.push_block(Block::Table(rows));
                }
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                self.links.pop();
                if let Some(id) = self.page.take() {
                    self.inline(Inline::PageLink { id });
                }
            }
            TagEnd::Image => {
                self.image = self.image.saturating_sub(1);
                if let Some(id) = self.attachment_image.take()
                    && let Some(current) = &mut self.inline
                {
                    let alt = std::mem::take(&mut current.content)
                        .iter()
                        .filter_map(|inline| match inline {
                            Inline::Text { text, .. } => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    self.lone_image = Some((id, alt));
                }
            }
            _ => {}
        }
    }

    /// An HTML block: the tags of a toggle, or text.
    fn html_block(&mut self, raw: &str) {
        let tokens: Vec<&str> = raw.split_whitespace().collect();
        let toggle = !tokens.is_empty()
            && tokens
                .iter()
                .all(|t| matches!(*t, "<details>" | "</details>" | "<summary>" | "</summary>"));
        if raw.trim() == EMPTY {
            self.push_block(Block::Paragraph(Vec::new()));
            return;
        }
        if !toggle {
            let text = raw.trim_end_matches('\n');
            let content = text
                .split('\n')
                .enumerate()
                .flat_map(|(index, line)| {
                    let text = Inline::Text {
                        text: line.into(),
                        marks: Marks::default(),
                    };
                    if index == 0 {
                        vec![text]
                    } else {
                        vec![Inline::HardBreak, text]
                    }
                })
                .fold(Vec::new(), |mut list, inline| {
                    push_inline(&mut list, inline);
                    list
                });
            self.push_block(Block::Paragraph(content));
            return;
        }
        for token in tokens {
            match token {
                "<details>" => self.stack.push(Container::Details {
                    summary: None,
                    in_summary: false,
                    blocks: Vec::new(),
                }),
                "<summary>" => {
                    if let Some(Container::Details { in_summary, .. }) = self.stack.last_mut() {
                        *in_summary = true;
                    }
                }
                "</summary>" => {
                    if let Some(Container::Details { in_summary, .. }) = self.stack.last_mut() {
                        *in_summary = false;
                    }
                }
                _ => {
                    if let Some(Container::Details { .. }) = self.stack.last() {
                        let Some(Container::Details {
                            summary, blocks, ..
                        }) = self.stack.pop()
                        else {
                            unreachable!()
                        };
                        self.push_block(Block::Details {
                            summary: summary.unwrap_or_default(),
                            content: blocks,
                        });
                    }
                }
            }
        }
    }

    fn finish(mut self) -> Document {
        self.close_implicit();
        // Unclosed toggles keep what they read.
        while self.stack.len() > 1 {
            match self.stack.pop() {
                Some(Container::Details {
                    summary, blocks, ..
                }) => self.push_block(Block::Details {
                    summary: summary.unwrap_or_default(),
                    content: blocks,
                }),
                _ => break,
            }
        }
        match self.stack.into_iter().next() {
            Some(Container::Root(blocks)) => Document { blocks },
            _ => Document::default(),
        }
    }
}
