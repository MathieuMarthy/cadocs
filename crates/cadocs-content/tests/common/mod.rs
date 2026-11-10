#![allow(dead_code)]

use cadocs_content::ast::*;

pub fn text(value: &str) -> Inline {
    Inline::Text {
        text: value.into(),
        marks: Marks::default(),
    }
}

pub fn marked(value: &str, marks: Marks) -> Inline {
    Inline::Text {
        text: value.into(),
        marks,
    }
}

pub fn para(value: &str) -> Block {
    Block::Paragraph(vec![text(value)])
}

pub fn link(href: &str) -> Link {
    Link {
        href: href.into(),
        target: Some("_blank".into()),
        rel: Some("noopener noreferrer nofollow".into()),
        class: None,
        title: None,
    }
}

pub fn cell(header: bool, value: &str) -> TableCell {
    TableCell {
        header,
        colspan: 1,
        rowspan: 1,
        colwidth: None,
        align: None,
        blocks: vec![para(value)],
    }
}

/// A document with every node and mark of the schema, in the shape the editor writes them.
pub fn every_node() -> Document {
    let bold = Marks {
        bold: true,
        ..Marks::default()
    };
    Document {
        blocks: vec![
            Block::Heading {
                level: 1,
                content: vec![text("Title")],
            },
            Block::Paragraph(vec![
                text("plain "),
                marked("bold", bold.clone()),
                text(" "),
                marked(
                    "italic",
                    Marks {
                        italic: true,
                        ..Marks::default()
                    },
                ),
                text(" "),
                marked(
                    "struck",
                    Marks {
                        strike: true,
                        ..Marks::default()
                    },
                ),
                text(" "),
                marked(
                    "under",
                    Marks {
                        underline: true,
                        ..Marks::default()
                    },
                ),
                text(" "),
                marked(
                    "code",
                    Marks {
                        code: true,
                        ..Marks::default()
                    },
                ),
                text(" "),
                marked(
                    "a link",
                    Marks {
                        link: Some(link("https://example.com/a")),
                        ..Marks::default()
                    },
                ),
                Inline::HardBreak,
                text("see "),
                Inline::PageLink {
                    id: "0190a0b0-0000-7000-8000-000000000001".into(),
                },
                text(" after"),
            ]),
            Block::Heading {
                level: 2,
                content: vec![text("Lists")],
            },
            Block::BulletList(vec![
                ListItem {
                    blocks: vec![
                        para("one"),
                        Block::BulletList(vec![ListItem {
                            blocks: vec![para("nested")],
                        }]),
                    ],
                },
                ListItem {
                    blocks: vec![para("two")],
                },
            ]),
            Block::OrderedList {
                start: 3,
                kind: None,
                items: vec![
                    ListItem {
                        blocks: vec![para("three")],
                    },
                    ListItem {
                        blocks: vec![para("four")],
                    },
                ],
            },
            Block::TaskList(vec![
                TaskItem {
                    checked: true,
                    blocks: vec![para("done")],
                },
                TaskItem {
                    checked: false,
                    blocks: vec![para("to do")],
                },
            ]),
            Block::CodeBlock {
                language: Some("rust".into()),
                code: "fn main() {\n    println!(\"``\");\n}".into(),
            },
            Block::Blockquote(vec![para("quoted")]),
            Block::HorizontalRule,
            Block::Callout {
                kind: "warning".into(),
                blocks: vec![para("careful")],
            },
            Block::Details {
                summary: vec![text("more")],
                content: vec![para("hidden")],
            },
            Block::Table(vec![
                TableRow {
                    cells: vec![cell(true, "name"), cell(true, "value")],
                },
                TableRow {
                    cells: vec![cell(false, "a"), cell(false, "1")],
                },
            ]),
            Block::TableOfContents,
            Block::Image {
                id: "0190a0b0-0000-7000-8000-00000000000a".into(),
                alt: Some("a diagram".into()),
            },
            Block::File {
                id: "0190a0b0-0000-7000-8000-00000000000f".into(),
                name: "report [final].pdf".into(),
            },
            Block::Plugin {
                kind: "kanban/board".into(),
                data: Some(r#"{"columns":["todo","done"]}"#.into()),
                text: "todo, done".into(),
            },
            Block::Heading {
                level: 3,
                content: vec![marked("Last", bold)],
            },
        ],
    }
}
