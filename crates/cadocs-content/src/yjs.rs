//! The AST from and to the Yjs layout of `@tiptap/y-tiptap`, the binding the editor uses.
//!
//! A node is an `XmlElement` named after its type, with an attribute per non-null node
//! attribute, defaults included. Runs of text are one `XmlText` between inline elements, and a
//! mark is a formatting attribute named after it whose value is the mark's attributes (`{}` for
//! bold). A mark that may overlap itself gets a `--hash` suffix, which the reader drops.

use std::collections::HashMap;
use std::sync::Arc;

use yrs::any::Number;
use yrs::types::Attrs;
use yrs::types::text::YChange;
use yrs::{
    Any, Doc, GetString, Out, ReadTxn, Text, Transact, TransactionMut, WriteTxn, Xml,
    XmlElementPrelim, XmlElementRef, XmlFragment, XmlFragmentRef, XmlOut, XmlTextPrelim,
    XmlTextRef,
};

use crate::ast::*;

/// The root `XmlFragment` the editor edits.
pub const FRAGMENT: &str = "default";
/// The root `Y.Text` that holds the title.
pub const TITLE: &str = "title";

// Reading.

/// The content of `doc`, empty if it has none.
pub fn read_doc(doc: &Doc) -> Document {
    read(&doc.transact())
}

/// The title and the content of `doc`, as of one moment.
pub fn read_page(doc: &Doc) -> (String, Document) {
    let txn = doc.transact();
    (read_title(&txn), read(&txn))
}

/// The content of the document `txn` reads.
pub fn read<T: ReadTxn>(txn: &T) -> Document {
    match txn.get_xml_fragment(FRAGMENT) {
        Some(fragment) => read_fragment(txn, &fragment),
        None => Document::default(),
    }
}

pub fn read_fragment<T: ReadTxn>(txn: &T, fragment: &XmlFragmentRef) -> Document {
    Document {
        blocks: blocks(txn, fragment),
    }
}

/// The title of the document `txn` reads.
pub fn read_title<T: ReadTxn>(txn: &T) -> String {
    txn.get_text(TITLE)
        .map(|title| title.get_string(txn))
        .unwrap_or_default()
}

fn attr<T: ReadTxn>(txn: &T, element: &XmlElementRef, name: &str) -> Option<Any> {
    match element.get_attribute(txn, name)? {
        Out::Any(Any::Null | Any::Undefined) => None,
        Out::Any(any) => Some(any),
        _ => None,
    }
}

fn int(any: &Any) -> Option<i64> {
    match any {
        Any::Number(Number::Int(n)) => Some(*n),
        Any::Number(Number::Float(f)) if f.fract() == 0.0 && f.is_finite() => Some(*f as i64),
        _ => None,
    }
}

fn int_attr<T: ReadTxn>(txn: &T, element: &XmlElementRef, name: &str) -> Option<i64> {
    attr(txn, element, name).as_ref().and_then(int)
}

fn string(any: Any) -> Option<String> {
    match any {
        Any::String(s) => Some(s.to_string()),
        _ => None,
    }
}

fn string_attr<T: ReadTxn>(txn: &T, element: &XmlElementRef, name: &str) -> Option<String> {
    attr(txn, element, name).and_then(string)
}

fn warn_unknown(kind: &str, name: &str) {
    tracing::warn!(kind, name, "content the schema does not know, kept as text");
}

/// The children of `parent`, read by `read` when they are elements named one of `tags`. Any
/// other child keeps its content as blocks, which `wrap` turns into an item of the container:
/// content is never dropped (ADR 0008).
fn children<T: ReadTxn, R>(
    txn: &T,
    parent: &impl XmlFragment,
    tags: &[&str],
    read: impl Fn(&XmlElementRef) -> R,
    wrap: impl Fn(Vec<Block>) -> R,
) -> Vec<R> {
    let mut found = Vec::new();
    for child in parent.children(txn) {
        let mut stray = Vec::new();
        match child {
            XmlOut::Element(element) if tags.contains(&element.tag().as_ref()) => {
                found.push(read(&element));
                continue;
            }
            XmlOut::Element(element) => {
                warn_unknown("child", element.tag());
                block(txn, &element, &mut stray);
            }
            XmlOut::Text(text) => {
                let mut content = Vec::new();
                text_runs(txn, &text, &mut content);
                if !content.is_empty() {
                    stray.push(Block::Paragraph(content));
                }
            }
            XmlOut::Fragment(_) => {}
        }
        if !stray.is_empty() {
            found.push(wrap(stray));
        }
    }
    found
}

fn blocks<T: ReadTxn>(txn: &T, parent: &impl XmlFragment) -> Vec<Block> {
    let mut found = Vec::new();
    for child in parent.children(txn) {
        match child {
            XmlOut::Element(element) => block(txn, &element, &mut found),
            // Text where blocks belong: kept as a paragraph.
            XmlOut::Text(text) => {
                let mut content = Vec::new();
                text_runs(txn, &text, &mut content);
                if !content.is_empty() {
                    found.push(Block::Paragraph(content));
                }
            }
            XmlOut::Fragment(_) => {}
        }
    }
    found
}

fn list_items<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> Vec<ListItem> {
    children(
        txn,
        element,
        &["listItem"],
        |item| ListItem {
            blocks: blocks(txn, item),
        },
        |blocks| ListItem { blocks },
    )
}

fn cell<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> TableCell {
    let colwidth = match attr(txn, element, "colwidth") {
        Some(Any::Array(widths)) => Some(widths.iter().filter_map(int).collect()),
        _ => None,
    };
    TableCell {
        header: element.tag().as_ref() == "tableHeader",
        colspan: int_attr(txn, element, "colspan")
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(1),
        rowspan: int_attr(txn, element, "rowspan")
            .and_then(|n| u32::try_from(n).ok())
            .unwrap_or(1),
        colwidth,
        align: string_attr(txn, element, "align"),
        blocks: blocks(txn, element),
    }
}

fn block<T: ReadTxn>(txn: &T, element: &XmlElementRef, found: &mut Vec<Block>) {
    let read = match element.tag().as_ref() {
        "paragraph" => Block::Paragraph(inlines(txn, element)),
        "heading" => Block::Heading {
            level: int_attr(txn, element, "level").unwrap_or(1).clamp(1, 6) as u8,
            content: inlines(txn, element),
        },
        "bulletList" => Block::BulletList(list_items(txn, element)),
        "orderedList" => Block::OrderedList {
            start: int_attr(txn, element, "start").unwrap_or(1),
            kind: string_attr(txn, element, "type"),
            items: list_items(txn, element),
        },
        "taskList" => Block::TaskList(children(
            txn,
            element,
            &["taskItem"],
            |item| TaskItem {
                checked: matches!(attr(txn, item, "checked"), Some(Any::Bool(true))),
                blocks: blocks(txn, item),
            },
            |blocks| TaskItem {
                checked: false,
                blocks,
            },
        )),
        "codeBlock" => Block::CodeBlock {
            language: string_attr(txn, element, "language"),
            code: plain_text(txn, element),
        },
        "blockquote" => Block::Blockquote(blocks(txn, element)),
        "horizontalRule" => Block::HorizontalRule,
        "callout" => Block::Callout {
            kind: string_attr(txn, element, "kind").unwrap_or_else(|| "note".into()),
            blocks: blocks(txn, element),
        },
        "details" => {
            let mut summary = Vec::new();
            let mut content = Vec::new();
            for part in element.children(txn) {
                match part {
                    XmlOut::Element(part) if part.tag().as_ref() == "detailsSummary" => {
                        summary = inlines(txn, &part)
                    }
                    XmlOut::Element(part) if part.tag().as_ref() == "detailsContent" => {
                        content = blocks(txn, &part)
                    }
                    XmlOut::Element(part) => block(txn, &part, &mut content),
                    _ => {}
                }
            }
            Block::Details { summary, content }
        }
        "table" => {
            let stray_cell = |blocks| TableCell {
                header: false,
                colspan: 1,
                rowspan: 1,
                colwidth: None,
                align: None,
                blocks,
            };
            Block::Table(children(
                txn,
                element,
                &["tableRow"],
                |row| TableRow {
                    cells: children(
                        txn,
                        row,
                        &["tableCell", "tableHeader"],
                        |element| cell(txn, element),
                        stray_cell,
                    ),
                },
                |blocks| TableRow {
                    cells: vec![stray_cell(blocks)],
                },
            ))
        }
        "tableOfContents" => Block::TableOfContents,
        "image" => Block::Image {
            id: string_attr(txn, element, "id").unwrap_or_default(),
            alt: string_attr(txn, element, "alt"),
        },
        "file" => Block::File {
            id: string_attr(txn, element, "id").unwrap_or_default(),
            name: string_attr(txn, element, "name").unwrap_or_default(),
        },
        "plugin" => Block::Plugin {
            kind: string_attr(txn, element, "kind").unwrap_or_default(),
            data: attr(txn, element, "data").map(|data| {
                let mut json = String::new();
                data.to_json(&mut json);
                json
            }),
            text: string_attr(txn, element, "text").unwrap_or_default(),
        },
        // Not a block of the schema: its blocks, or its text as a paragraph, are kept.
        other => {
            warn_unknown("node", other);
            let inner = blocks(txn, element);
            let has_elements = element
                .children(txn)
                .any(|c| matches!(c, XmlOut::Element(_)));
            if has_elements {
                found.extend(inner);
            } else {
                let text = plain_text(txn, element);
                if !text.is_empty() {
                    found.push(Block::Paragraph(vec![Inline::Text {
                        text,
                        marks: Marks::default(),
                    }]));
                }
            }
            return;
        }
    };
    found.push(read);
}

/// Every character under `element`, without marks.
fn plain_text<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> String {
    let mut text = String::new();
    for child in element.children(txn) {
        match child {
            XmlOut::Text(part) => text.push_str(&part.get_string(txn)),
            XmlOut::Element(part) => text.push_str(&plain_text(txn, &part)),
            XmlOut::Fragment(_) => {}
        }
    }
    text
}

fn inlines<T: ReadTxn>(txn: &T, element: &XmlElementRef) -> Vec<Inline> {
    let mut found = Vec::new();
    for child in element.children(txn) {
        match child {
            XmlOut::Text(text) => text_runs(txn, &text, &mut found),
            XmlOut::Element(inline) => match inline.tag().as_ref() {
                "hardBreak" => found.push(Inline::HardBreak),
                "pageLink" => found.push(Inline::PageLink {
                    id: string_attr(txn, &inline, "id").unwrap_or_default(),
                }),
                other => {
                    warn_unknown("node", other);
                    push_inline(
                        &mut found,
                        Inline::Text {
                            text: plain_text(txn, &inline),
                            marks: Marks::default(),
                        },
                    );
                }
            },
            XmlOut::Fragment(_) => {}
        }
    }
    found
}

/// `bold--abcdEF12` is `bold`: y-tiptap suffixes marks that may overlap themselves.
fn mark_name(attribute: &str) -> &str {
    match attribute.rsplit_once("--") {
        Some((name, hash))
            if hash.len() == 8
                && hash
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(&b)) =>
        {
            name
        }
        _ => attribute,
    }
}

fn link(value: &Any) -> Option<Link> {
    let Any::Map(attrs) = value else { return None };
    let get = |name: &str| attrs.get(name).cloned().and_then(string);
    Some(Link {
        href: get("href")?,
        target: get("target"),
        rel: get("rel"),
        class: get("class"),
        title: get("title"),
    })
}

fn marks(attributes: Option<&Attrs>) -> Marks {
    let mut marks = Marks::default();
    for (attribute, value) in attributes.into_iter().flatten() {
        if matches!(value, Any::Null | Any::Undefined) {
            continue;
        }
        match mark_name(attribute) {
            "bold" => marks.bold = true,
            "italic" => marks.italic = true,
            "strike" => marks.strike = true,
            "underline" => marks.underline = true,
            "code" => marks.code = true,
            "link" => marks.link = link(value),
            other => warn_unknown("mark", other),
        }
    }
    marks
}

fn text_runs<T: ReadTxn>(txn: &T, text: &XmlTextRef, found: &mut Vec<Inline>) {
    for diff in text.diff(txn, YChange::identity) {
        let Out::Any(Any::String(chunk)) = &diff.insert else {
            continue;
        };
        push_inline(
            found,
            Inline::Text {
                text: chunk.to_string(),
                marks: marks(diff.attributes.as_deref()),
            },
        );
    }
}

// Writing.

/// Replaces what `fragment` holds with `document`, in `txn`. Content the schema would refuse is
/// fixed up: an empty container gets an empty paragraph, a list item starts with a paragraph,
/// and a summary or code block holds plain text.
pub fn write(txn: &mut TransactionMut, fragment: &XmlFragmentRef, document: &Document) {
    let len = fragment.len(txn);
    if len > 0 {
        fragment.remove_range(txn, 0, len);
    }
    write_blocks(txn, fragment, &document.blocks, false);
}

/// Replaces the title and the content of a page, in `txn`, as restoring a revision does. A title
/// that holds more than text is left as it is, since yrs cannot remove what is not text, and the
/// answer is then `false`.
pub fn replace_page(txn: &mut TransactionMut, title: &str, document: &Document) -> bool {
    let text = txn.get_or_insert_text(TITLE);
    let current = text.get_string(txn);
    let length = text.len(txn);
    let plain = current.len() == length as usize;
    if plain && current != title {
        text.remove_range(txn, 0, length);
        text.insert(txn, 0, title);
    }
    let fragment = txn.get_or_insert_xml_fragment(FRAGMENT);
    write(txn, &fragment, document);
    plain
}

fn element<P: XmlFragment>(txn: &mut TransactionMut, parent: &P, tag: &str) -> XmlElementRef {
    parent.push_back(txn, XmlElementPrelim::empty(tag))
}

/// Writes `blocks` under `parent`. `required` adds an empty paragraph when there are none,
/// for content expressions with `+`.
fn write_blocks<P: XmlFragment>(
    txn: &mut TransactionMut,
    parent: &P,
    blocks: &[Block],
    required: bool,
) {
    if blocks.is_empty() && required {
        element(txn, parent, "paragraph");
    }
    for block in blocks {
        write_block(txn, parent, block);
    }
}

/// The blocks of a list item, which must start with a paragraph.
fn write_item<P: XmlFragment>(txn: &mut TransactionMut, item: &P, blocks: &[Block]) {
    if !matches!(blocks.first(), Some(Block::Paragraph(_))) {
        element(txn, item, "paragraph");
    }
    write_blocks(txn, item, blocks, false);
}

fn write_block<P: XmlFragment>(txn: &mut TransactionMut, parent: &P, block: &Block) {
    match block {
        Block::Paragraph(content) => {
            let paragraph = element(txn, parent, "paragraph");
            write_inlines(txn, &paragraph, content);
        }
        Block::Heading { level, content } => {
            let heading = element(txn, parent, "heading");
            heading.insert_attribute(txn, "level", Any::from(i64::from(*level)));
            write_inlines(txn, &heading, content);
        }
        Block::BulletList(items) => {
            let list = element(txn, parent, "bulletList");
            write_list_items(txn, &list, items);
        }
        Block::OrderedList { start, kind, items } => {
            let list = element(txn, parent, "orderedList");
            list.insert_attribute(txn, "start", Any::from(*start));
            if let Some(kind) = kind {
                list.insert_attribute(txn, "type", Any::from(kind.as_str()));
            }
            write_list_items(txn, &list, items);
        }
        Block::TaskList(items) => {
            let list = element(txn, parent, "taskList");
            if items.is_empty() {
                let item = element(txn, &list, "taskItem");
                item.insert_attribute(txn, "checked", Any::Bool(false));
                element(txn, &item, "paragraph");
            }
            for task in items {
                let item = element(txn, &list, "taskItem");
                item.insert_attribute(txn, "checked", Any::Bool(task.checked));
                write_item(txn, &item, &task.blocks);
            }
        }
        Block::CodeBlock { language, code } => {
            let block = element(txn, parent, "codeBlock");
            if let Some(language) = language {
                block.insert_attribute(txn, "language", Any::from(language.as_str()));
            }
            if !code.is_empty() {
                block.push_back(txn, XmlTextPrelim::new(code.as_str()));
            }
        }
        Block::Blockquote(blocks) => {
            let quote = element(txn, parent, "blockquote");
            write_blocks(txn, &quote, blocks, true);
        }
        Block::HorizontalRule => {
            element(txn, parent, "horizontalRule");
        }
        Block::Callout { kind, blocks } => {
            let callout = element(txn, parent, "callout");
            callout.insert_attribute(txn, "kind", Any::from(kind.as_str()));
            write_blocks(txn, &callout, blocks, true);
        }
        Block::Details { summary, content } => {
            let details = element(txn, parent, "details");
            let head = element(txn, &details, "detailsSummary");
            // `text*`: breaks become spaces, and page links are left out.
            let text: Vec<Inline> = summary.iter().fold(Vec::new(), |mut list, inline| {
                let inline = match inline {
                    Inline::Text { .. } => inline.clone(),
                    Inline::HardBreak => Inline::Text {
                        text: " ".into(),
                        marks: Marks::default(),
                    },
                    Inline::PageLink { .. } => return list,
                };
                push_inline(&mut list, inline);
                list
            });
            write_inlines(txn, &head, &text);
            let body = element(txn, &details, "detailsContent");
            write_blocks(txn, &body, content, true);
        }
        Block::Table(rows) => {
            let table = element(txn, parent, "table");
            if rows.is_empty() {
                element(txn, &table, "tableRow");
            }
            for row in rows {
                let line = element(txn, &table, "tableRow");
                for cell in &row.cells {
                    let tag = if cell.header {
                        "tableHeader"
                    } else {
                        "tableCell"
                    };
                    let node = element(txn, &line, tag);
                    if let Some(align) = &cell.align {
                        node.insert_attribute(txn, "align", Any::from(align.as_str()));
                    }
                    node.insert_attribute(txn, "colspan", Any::from(i64::from(cell.colspan)));
                    if let Some(widths) = &cell.colwidth {
                        let widths: Vec<Any> = widths.iter().map(|w| Any::from(*w)).collect();
                        node.insert_attribute(txn, "colwidth", Any::Array(widths.into()));
                    }
                    node.insert_attribute(txn, "rowspan", Any::from(i64::from(cell.rowspan)));
                    write_blocks(txn, &node, &cell.blocks, true);
                }
            }
        }
        Block::TableOfContents => {
            element(txn, parent, "tableOfContents");
        }
        Block::Image { id, alt } => {
            let node = element(txn, parent, "image");
            if let Some(alt) = alt {
                node.insert_attribute(txn, "alt", Any::from(alt.as_str()));
            }
            node.insert_attribute(txn, "id", Any::from(id.as_str()));
        }
        Block::File { id, name } => {
            let node = element(txn, parent, "file");
            node.insert_attribute(txn, "id", Any::from(id.as_str()));
            node.insert_attribute(txn, "name", Any::from(name.as_str()));
        }
        Block::Plugin { kind, data, text } => {
            let node = element(txn, parent, "plugin");
            if let Some(data) = data.as_deref().and_then(|data| Any::from_json(data).ok()) {
                node.insert_attribute(txn, "data", data);
            }
            node.insert_attribute(txn, "kind", Any::from(kind.as_str()));
            node.insert_attribute(txn, "text", Any::from(text.as_str()));
        }
    }
}

fn write_list_items<P: XmlFragment>(txn: &mut TransactionMut, list: &P, items: &[ListItem]) {
    if items.is_empty() {
        let item = element(txn, list, "listItem");
        element(txn, &item, "paragraph");
    }
    for item in items {
        let node = element(txn, list, "listItem");
        write_item(txn, &node, &item.blocks);
    }
}

fn mark_attrs(marks: &Marks) -> Attrs {
    let empty = || Any::Map(Arc::new(HashMap::new()));
    let mut attrs = Attrs::new();
    for (on, name) in [
        (marks.bold, "bold"),
        (marks.italic, "italic"),
        (marks.strike, "strike"),
        (marks.underline, "underline"),
        (marks.code, "code"),
    ] {
        if on {
            attrs.insert(name.into(), empty());
        }
    }
    if let Some(link) = &marks.link {
        let value = |v: &Option<String>| v.as_deref().map(Any::from).unwrap_or(Any::Null);
        let map = HashMap::from([
            ("href".to_string(), Any::from(link.href.as_str())),
            ("target".to_string(), value(&link.target)),
            ("rel".to_string(), value(&link.rel)),
            ("class".to_string(), value(&link.class)),
            ("title".to_string(), value(&link.title)),
        ]);
        attrs.insert("link".into(), Any::Map(Arc::new(map)));
    }
    attrs
}

fn write_inlines(txn: &mut TransactionMut, parent: &XmlElementRef, content: &[Inline]) {
    let mut text: Option<(XmlTextRef, u32)> = None;
    for inline in content {
        match inline {
            Inline::Text { text: chunk, marks } => {
                if chunk.is_empty() {
                    continue;
                }
                let (node, len) =
                    text.get_or_insert_with(|| (parent.push_back(txn, XmlTextPrelim::new("")), 0));
                // Explicit attributes, even empty: an insert without them takes the marks of
                // the text before it.
                node.insert_with_attributes(txn, *len, chunk, mark_attrs(marks));
                *len = node.len(txn);
            }
            Inline::HardBreak => {
                text = None;
                element(txn, parent, "hardBreak");
            }
            Inline::PageLink { id } => {
                text = None;
                let link = element(txn, parent, "pageLink");
                if !id.is_empty() {
                    link.insert_attribute(txn, "id", Any::from(id.as_str()));
                }
            }
        }
    }
}
