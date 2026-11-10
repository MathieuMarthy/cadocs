mod common;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use cadocs_content::ast::*;
use cadocs_content::{schema, yjs};
use common::*;
use yrs::types::text::YChange;
use yrs::{
    Any, Doc, GetString, Text, Transact, Xml, XmlElementPrelim, XmlFragment, XmlOut, XmlTextPrelim,
};

fn written(document: &Document) -> Doc {
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    yjs::write(&mut doc.transact_mut(), &fragment, document);
    doc
}

/// Every element name under `node`, at any depth.
fn tags(node: &impl XmlFragment, txn: &yrs::Transaction, found: &mut BTreeSet<String>) {
    for child in node.children(txn) {
        if let XmlOut::Element(element) = child {
            found.insert(element.tag().to_string());
            tags(&element, txn, found);
        }
    }
}

fn mark_names(node: &impl XmlFragment, txn: &yrs::Transaction, found: &mut BTreeSet<String>) {
    for child in node.children(txn) {
        match child {
            XmlOut::Element(element) => mark_names(&element, txn, found),
            XmlOut::Text(text) => {
                for diff in text.diff(txn, YChange::identity) {
                    for name in diff.attributes.iter().flat_map(|attrs| attrs.keys()) {
                        found.insert(name.to_string());
                    }
                }
            }
            XmlOut::Fragment(_) => {}
        }
    }
}

#[test]
fn every_node_round_trips_through_yjs() {
    let document = every_node();
    let doc = written(&document);
    assert_eq!(yjs::read_doc(&doc), document);
}

#[test]
fn the_sample_uses_every_node_and_mark_of_the_schema() {
    let doc = written(&every_node());
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    let txn = doc.transact();
    let mut nodes = BTreeSet::new();
    tags(&fragment, &txn, &mut nodes);
    let mut marks = BTreeSet::new();
    mark_names(&fragment, &txn, &mut marks);
    // `doc` is the fragment and `text` the text types.
    let expected: BTreeSet<String> = schema::NODES
        .iter()
        .map(|node| node.name.to_string())
        .filter(|name| name != "doc" && name != "text")
        .collect();
    assert_eq!(nodes, expected);
    let expected: BTreeSet<String> = schema::MARKS
        .iter()
        .map(|mark| mark.name.to_string())
        .collect();
    assert_eq!(marks, expected);
}

#[test]
fn write_replaces_what_the_fragment_held() {
    let doc = written(&every_node());
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    let small = Document {
        blocks: vec![para("only")],
    };
    yjs::write(&mut doc.transact_mut(), &fragment, &small);
    assert_eq!(yjs::read_doc(&doc), small);
}

#[test]
fn layout_matches_what_the_editor_writes() {
    let doc = written(&every_node());
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    let txn = doc.transact();
    let children: Vec<_> = fragment.children(&txn).collect();
    let XmlOut::Element(heading) = &children[0] else {
        panic!("a heading element first");
    };
    assert_eq!(
        heading
            .get_attribute(&txn, "level")
            .map(|v| v.to_string(&txn)),
        Some("1".into())
    );
    let XmlOut::Element(paragraph) = &children[1] else {
        panic!("a paragraph element second");
    };
    let parts: Vec<_> = paragraph.children(&txn).collect();
    // Text, then inline nodes as elements between texts.
    let kinds: Vec<String> = parts
        .iter()
        .map(|part| match part {
            XmlOut::Element(e) => e.tag().to_string(),
            _ => "text".into(),
        })
        .collect();
    assert_eq!(kinds, ["text", "hardBreak", "text", "pageLink", "text"]);
    let XmlOut::Text(first) = &parts[0] else {
        unreachable!()
    };
    let diff = first.diff(&txn, YChange::identity);
    let bold = diff
        .iter()
        .find_map(|d| d.attributes.as_ref().and_then(|a| a.get("bold").cloned()))
        .expect("bold is an attribute");
    assert_eq!(bold, Any::Map(Arc::new(HashMap::new())));
    // A null attribute is left out, as y-prosemirror does.
    let XmlOut::Element(list) = &children[4] else {
        unreachable!()
    };
    assert_eq!(list.tag().as_ref(), "orderedList");
    assert!(list.get_attribute(&txn, "type").is_none());
}

#[test]
fn what_the_editor_writes_reads_back() {
    // As yjs writes it: a float level, a hashed mark name and a null attribute.
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    {
        let mut txn = doc.transact_mut();
        let heading = fragment.push_back(&mut txn, XmlElementPrelim::empty("heading"));
        heading.insert_attribute(&mut txn, "level", Any::from(2.0));
        let text = heading.push_back(&mut txn, XmlTextPrelim::new(""));
        let attrs = HashMap::from([(
            Arc::from("bold--abcdEF12"),
            Any::Map(Arc::new(HashMap::new())),
        )]);
        text.insert_with_attributes(&mut txn, 0, "two", attrs);
        let block = fragment.push_back(&mut txn, XmlElementPrelim::empty("codeBlock"));
        block.insert_attribute(&mut txn, "language", Any::Null);
        block.push_back(&mut txn, XmlTextPrelim::new("x"));
    }
    let read = yjs::read_doc(&doc);
    assert_eq!(
        read.blocks,
        vec![
            Block::Heading {
                level: 2,
                content: vec![marked(
                    "two",
                    Marks {
                        bold: true,
                        ..Marks::default()
                    }
                )],
            },
            Block::CodeBlock {
                language: None,
                code: "x".into(),
            },
        ]
    );
}

#[test]
fn an_unknown_node_keeps_its_text() {
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    {
        let mut txn = doc.transact_mut();
        let block = fragment.push_back(&mut txn, XmlElementPrelim::empty("futureBlock"));
        block.push_back(&mut txn, XmlTextPrelim::new("from the future"));
        let paragraph = fragment.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        paragraph.push_back(&mut txn, XmlTextPrelim::new("a "));
        let inline = paragraph.push_back(&mut txn, XmlElementPrelim::empty("futureInline"));
        inline.push_back(&mut txn, XmlTextPrelim::new("mention"));
        let text = paragraph.push_back(&mut txn, XmlTextPrelim::new(""));
        let attrs = HashMap::from([(Arc::from("futureMark"), Any::Bool(true))]);
        text.insert_with_attributes(&mut txn, 0, " here", attrs);
        // An unknown block holding known blocks keeps them.
        let columns = fragment.push_back(&mut txn, XmlElementPrelim::empty("columns"));
        let inner = columns.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        inner.push_back(&mut txn, XmlTextPrelim::new("in a column"));
    }
    assert_eq!(
        yjs::read_doc(&doc).blocks,
        vec![
            para("from the future"),
            Block::Paragraph(vec![text("a mention here")]),
            para("in a column"),
        ]
    );
}

#[test]
fn written_content_fits_the_schema() {
    // Containers the schema requires content in get an empty paragraph, a list item starts with
    // one, and a summary holds text only.
    let document = Document {
        blocks: vec![
            Block::Blockquote(vec![]),
            Block::BulletList(vec![ListItem {
                blocks: vec![Block::HorizontalRule],
            }]),
            Block::Details {
                summary: vec![text("a"), Inline::HardBreak, text("b")],
                content: vec![],
            },
        ],
    };
    let doc = written(&document);
    assert_eq!(
        yjs::read_doc(&doc).blocks,
        vec![
            Block::Blockquote(vec![Block::Paragraph(vec![])]),
            Block::BulletList(vec![ListItem {
                blocks: vec![Block::Paragraph(vec![]), Block::HorizontalRule],
            }]),
            Block::Details {
                summary: vec![text("a b")],
                content: vec![Block::Paragraph(vec![])],
            },
        ]
    );
}

#[test]
fn the_title_reads_from_its_own_root() {
    let doc = Doc::new();
    let title = doc.get_or_insert_text(yjs::TITLE);
    title.insert(&mut doc.transact_mut(), 0, "Hello");
    assert_eq!(yjs::read_title(&doc.transact()), "Hello");
    assert_eq!(title.get_string(&doc.transact()), "Hello");
}

#[test]
fn odd_children_of_lists_and_tables_keep_their_text() {
    let doc = Doc::new();
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    {
        let mut txn = doc.transact_mut();
        let list = fragment.push_back(&mut txn, XmlElementPrelim::empty("bulletList"));
        let stray = list.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        stray.push_back(&mut txn, XmlTextPrelim::new("in a list"));
        let table = fragment.push_back(&mut txn, XmlElementPrelim::empty("table"));
        let row = table.push_back(&mut txn, XmlElementPrelim::empty("tableRow"));
        let odd = row.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        odd.push_back(&mut txn, XmlTextPrelim::new("in a row"));
        let loose = table.push_back(&mut txn, XmlElementPrelim::empty("paragraph"));
        loose.push_back(&mut txn, XmlTextPrelim::new("in a table"));
    }
    let blocks = yjs::read_doc(&doc).blocks;
    let text = cadocs_content::text::to_text(&Document { blocks });
    for part in ["in a list", "in a row", "in a table"] {
        assert!(text.contains(part), "{part} lost from {text:?}");
    }
}

#[test]
fn replace_page_writes_the_title_and_the_content() {
    let doc = written(&every_node());
    doc.get_or_insert_text(yjs::TITLE)
        .insert(&mut doc.transact_mut(), 0, "Old title");
    let small = Document {
        blocks: vec![para("restored")],
    };
    assert!(yjs::replace_page(&mut doc.transact_mut(), "New", &small));
    assert_eq!(yjs::read_page(&doc), ("New".to_owned(), small));
}

#[test]
fn replace_page_keeps_a_title_holding_more_than_text() {
    let doc = Doc::new();
    let title = doc.get_or_insert_text(yjs::TITLE);
    title.insert(&mut doc.transact_mut(), 0, "Odd");
    title.insert_embed(&mut doc.transact_mut(), 3, Any::from("embed"));
    let small = Document {
        blocks: vec![para("restored")],
    };
    assert!(!yjs::replace_page(&mut doc.transact_mut(), "New", &small));
    assert_eq!(yjs::read_doc(&doc), small);
}

#[test]
fn page_links_lists_each_linked_page_once() {
    let link = |id: &str| Inline::PageLink { id: id.to_owned() };
    let document = Document {
        blocks: vec![
            Block::Paragraph(vec![link("a"), text(" and "), link("b")]),
            Block::Blockquote(vec![Block::Heading {
                level: 2,
                content: vec![link("a"), link("c")],
            }]),
        ],
    };
    assert_eq!(document.page_links(), ["a", "b", "c"]);
}
