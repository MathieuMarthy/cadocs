mod common;

use cadocs_content::text::to_text;
use common::*;

#[test]
fn every_node_gives_its_text() {
    let text = to_text(&every_node());
    for part in [
        "Title",
        "plain bold italic struck under code a link\nsee  after",
        "one\nnested\ntwo",
        "three\nfour",
        "done\nto do",
        "println!",
        "quoted",
        "careful",
        "more\nhidden",
        "name\nvalue\na\n1",
        "a diagram",
        "report [final].pdf",
        "todo, done",
        "Last",
    ] {
        assert!(text.contains(part), "missing {part:?} in {text:?}");
    }
    // A page link's title belongs to another page, which a reader of this one may not see.
    assert!(!text.contains("0190a0b0"));
}

#[test]
fn every_node_lists_its_attachments() {
    assert_eq!(
        every_node().attachments(),
        [
            "0190a0b0-0000-7000-8000-00000000000a",
            "0190a0b0-0000-7000-8000-00000000000f",
        ]
    );
}

#[test]
fn attachments_are_found_deep_and_listed_once() {
    use cadocs_content::ast::{Block, Document, ListItem};
    let image = || Block::Image {
        id: "a".into(),
        alt: None,
    };
    let document = Document {
        blocks: vec![
            image(),
            Block::Callout {
                kind: "note".into(),
                blocks: vec![Block::BulletList(vec![ListItem {
                    blocks: vec![
                        image(),
                        Block::File {
                            id: "b".into(),
                            name: "b.pdf".into(),
                        },
                    ],
                }])],
            },
        ],
    };
    assert_eq!(document.attachments(), ["a", "b"]);
}
