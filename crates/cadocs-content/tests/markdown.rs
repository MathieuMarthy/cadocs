mod common;

use cadocs_content::ast::*;
use cadocs_content::markdown::{from_markdown, to_markdown};
use common::*;

fn titles(id: &str) -> Option<String> {
    (id == "0190a0b0-0000-7000-8000-000000000001").then(|| "Other page".to_string())
}

fn none(_: &str) -> Option<String> {
    None
}

fn md(blocks: Vec<Block>) -> String {
    to_markdown(&Document { blocks }, &none)
}

fn round_trip(blocks: Vec<Block>) {
    let document = Document { blocks };
    let written = to_markdown(&document, &titles);
    assert_eq!(from_markdown(&written), document, "through\n{written}");
}

fn bold() -> Marks {
    Marks {
        bold: true,
        ..Marks::default()
    }
}

fn italic() -> Marks {
    Marks {
        italic: true,
        ..Marks::default()
    }
}

#[test]
fn every_node_round_trips_through_markdown() {
    let mut expected = every_node();
    // Markdown has no plugin block: its text stands for it (ADR 0016).
    for block in &mut expected.blocks {
        if let Block::Plugin { text, .. } = block {
            *block = para(text);
        }
    }
    let written = to_markdown(&every_node(), &titles);
    assert_eq!(from_markdown(&written), expected, "through\n{written}");
}

#[test]
fn an_attachment_is_a_block_only_alone_in_its_paragraph() {
    let image = "/api/v1/attachments/0190a0b0-0000-7000-8000-00000000000a";
    assert_eq!(
        from_markdown(&format!("![]({image})")).blocks,
        [Block::Image {
            id: "0190a0b0-0000-7000-8000-00000000000a".into(),
            alt: None,
        }]
    );
    // Beside text, or from elsewhere, an image is still its alternative text.
    assert_eq!(
        from_markdown(&format!("see ![chart]({image}) here")).blocks,
        [para("see chart here")]
    );
    assert_eq!(
        from_markdown("![chart](https://example.com/a.png)").blocks,
        [para("chart")]
    );
    // Two images side by side are both kept, as text.
    assert_eq!(
        from_markdown(&format!("![a]({image})![b]({image})")).blocks,
        [para("ab")]
    );
    assert_eq!(
        from_markdown(&format!("![]({image})![b]({image})")).blocks,
        [para("b")]
    );
    // An image in a link keeps the link.
    assert_eq!(
        from_markdown(&format!("[![a]({image})](https://example.com/)")).blocks,
        [Block::Paragraph(vec![Inline::Text {
            text: "a".into(),
            marks: Marks {
                link: Some(Link::new("https://example.com/", None)),
                ..Marks::default()
            },
        }])]
    );
    let file = "/api/v1/attachments/0190a0b0-0000-7000-8000-00000000000f";
    let link = |text: &str| Inline::Text {
        text: text.into(),
        marks: Marks {
            link: Some(Link::new(file, None)),
            ..Marks::default()
        },
    };
    assert_eq!(
        from_markdown(&format!("see [notes]({file})")).blocks,
        [Block::Paragraph(vec![
            Inline::Text {
                text: "see ".into(),
                marks: Marks::default(),
            },
            link("notes")
        ])]
    );
    assert_eq!(
        from_markdown(&format!("[**notes**]({file})")).blocks,
        [Block::Paragraph(vec![Inline::Text {
            text: "notes".into(),
            marks: Marks {
                bold: true,
                link: Some(Link::new(file, None)),
                ..Marks::default()
            },
        }])]
    );
    round_trip(vec![
        Block::Image {
            id: "0190a0b0-0000-7000-8000-00000000000a".into(),
            alt: Some("*not* [a] link".into()),
        },
        Block::File {
            id: "0190a0b0-0000-7000-8000-00000000000f".into(),
            name: "<u>*x*</u>".into(),
        },
    ]);
}

#[test]
fn every_node_writes_its_convention() {
    let written = to_markdown(&every_node(), &titles);
    for part in [
        "# Title\n",
        "plain **bold** *italic* ~~struck~~ <u>under</u> `code` [a link](https://example.com/a)\\\nsee [Other page](/?page=0190a0b0-0000-7000-8000-000000000001) after",
        "- one\n  - nested\n- two",
        "3. three\n4. four",
        "- [x] done\n- [ ] to do",
        "```rust\nfn main() {\n    println!(\"``\");\n}\n```",
        "> quoted",
        "---",
        "> [!WARNING]\n> careful",
        "<details>\n<summary>\n\nmore\n\n</summary>\n\nhidden\n\n</details>",
        "| name | value |\n| --- | --- |\n| a | 1 |",
        "[TOC]",
        "![a diagram](/api/v1/attachments/0190a0b0-0000-7000-8000-00000000000a)",
        "[report \\[final\\].pdf](/api/v1/attachments/0190a0b0-0000-7000-8000-00000000000f)",
        "todo, done",
        "### **Last**",
    ] {
        assert!(written.contains(part), "missing {part:?} in\n{written}");
    }
}

#[test]
fn markdown_characters_in_text_stay_text() {
    round_trip(vec![
        para(r"*not bold* _nor this_ [x](y) `c` <b>tag</b> ~~no~~ a | b \ & &amp; ![i](j)"),
        para("# not a heading"),
        para("1. not a list"),
        para("- not a list"),
        para("> not a quote"),
        para("[TOC]"),
        para("+ plus"),
        para("==="),
        para("<details>"),
    ]);
}

#[test]
fn adjacent_marks_keep_their_bounds() {
    round_trip(vec![Block::Paragraph(vec![
        marked("a", bold()),
        marked("b", italic()),
        text(" "),
        marked(
            "c",
            Marks {
                bold: true,
                italic: true,
                ..Marks::default()
            },
        ),
        marked("d", bold()),
        text(" in"),
        marked("side", bold()),
        text("word"),
    ])]);
}

#[test]
fn spaces_at_the_edge_of_a_mark_move_outside_it() {
    let written = md(vec![Block::Paragraph(vec![
        text("a"),
        marked(" b ", bold()),
        text("c"),
    ])]);
    assert_eq!(written, "a **b** c\n");
}

#[test]
fn code_spans_hold_backticks() {
    round_trip(vec![Block::Paragraph(vec![marked(
        "a `b` c",
        Marks {
            code: true,
            ..Marks::default()
        },
    )])]);
    round_trip(vec![Block::Paragraph(vec![marked(
        "`",
        Marks {
            code: true,
            ..Marks::default()
        },
    )])]);
}

#[test]
fn links_keep_titles_and_odd_addresses() {
    round_trip(vec![Block::Paragraph(vec![marked(
        "x",
        Marks {
            link: Some(Link {
                title: Some(r#"a "title""#.into()),
                ..link("https://example.com/a b(c)")
            }),
            ..Marks::default()
        },
    )])]);
}

#[test]
fn an_unreadable_page_link_writes_no_title() {
    let written = md(vec![Block::Paragraph(vec![Inline::PageLink {
        id: "x".into(),
    }])]);
    assert_eq!(written, "[page not found](/?page=x)\n");
}

#[test]
fn a_link_outside_the_allow_list_is_written_and_read_as_text() {
    let written = md(vec![Block::Paragraph(vec![marked(
        "click",
        Marks {
            link: Some(link("javascript:alert(1)")),
            ..Marks::default()
        },
    )])]);
    assert_eq!(written, "click\n");
    assert_eq!(
        from_markdown("[click](javascript:alert(1))").blocks,
        vec![para("click")]
    );
}

#[test]
fn raw_html_imports_as_text() {
    assert_eq!(
        from_markdown("<script>alert(1)</script>\n\nhi <img src=x onerror=alert(1)>").blocks,
        vec![
            para("<script>alert(1)</script>"),
            para("hi <img src=x onerror=alert(1)>"),
        ]
    );
}

#[test]
fn import_maps_what_the_schema_lacks() {
    assert_eq!(
        from_markdown("#### deep\n\n![alt text](https://example.com/i.png)").blocks,
        vec![
            Block::Heading {
                level: 3,
                content: vec![text("deep")],
            },
            para("alt text"),
        ]
    );
}

#[test]
fn alerts_become_callouts() {
    let read = from_markdown(
        "> [!NOTE]\n> a\n\n> [!TIP]\n> b\n\n> [!IMPORTANT]\n> c\n\n> [!WARNING]\n> d\n\n> [!CAUTION]\n> e",
    );
    let kinds: Vec<_> = read
        .blocks
        .iter()
        .map(|block| match block {
            Block::Callout { kind, .. } => kind.as_str(),
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["note", "note", "note", "warning", "danger"]);
    round_trip(vec![Block::Callout {
        kind: "danger".into(),
        blocks: vec![
            para("x"),
            Block::BulletList(vec![ListItem {
                blocks: vec![para("y")],
            }]),
        ],
    }]);
}

#[test]
fn tight_and_loose_lists_read_the_same() {
    let item = |value| ListItem {
        blocks: vec![para(value)],
    };
    let expected = vec![Block::BulletList(vec![item("a"), item("b")])];
    assert_eq!(from_markdown("- a\n- b").blocks, expected);
    assert_eq!(from_markdown("- a\n\n- b").blocks, expected);
    assert_eq!(
        from_markdown("1. a\n2. b").blocks,
        vec![Block::OrderedList {
            start: 1,
            kind: None,
            items: vec![item("a"), item("b")],
        }]
    );
}

#[test]
fn a_list_item_starting_with_another_block_gets_a_paragraph() {
    assert_eq!(
        from_markdown("- ```\n  x\n  ```").blocks,
        vec![Block::BulletList(vec![ListItem {
            blocks: vec![
                Block::Paragraph(vec![]),
                Block::CodeBlock {
                    language: None,
                    code: "x".into(),
                },
            ],
        }])]
    );
}

#[test]
fn nested_blocks_round_trip() {
    round_trip(vec![
        Block::BulletList(vec![ListItem {
            blocks: vec![
                para("a"),
                para("second paragraph"),
                Block::Blockquote(vec![para("q")]),
                Block::CodeBlock {
                    language: None,
                    code: "code\n\nwith a blank line".into(),
                },
            ],
        }]),
        Block::TaskList(vec![TaskItem {
            checked: false,
            blocks: vec![
                para("parent"),
                Block::TaskList(vec![TaskItem {
                    checked: true,
                    blocks: vec![para("child")],
                }]),
            ],
        }]),
        Block::Details {
            summary: vec![text("outer "), marked("bold", bold())],
            content: vec![Block::Details {
                summary: vec![text("inner")],
                content: vec![para("deep"), Block::HorizontalRule],
            }],
        },
        Block::Blockquote(vec![Block::Blockquote(vec![para("twice")])]),
    ]);
}

#[test]
fn tables_keep_alignment_and_escape_pipes() {
    let aligned = |header, value: &str, align: &str| TableCell {
        align: Some(align.into()),
        ..cell(header, value)
    };
    round_trip(vec![Block::Table(vec![
        TableRow {
            cells: vec![
                aligned(true, "a|b", "left"),
                aligned(true, "c", "center"),
                aligned(true, "d", "right"),
            ],
        },
        TableRow {
            cells: vec![
                aligned(false, "1", "left"),
                TableCell {
                    blocks: vec![Block::Paragraph(vec![])],
                    ..aligned(false, "", "center")
                },
                aligned(false, "3", "right"),
            ],
        },
    ])]);
}

#[test]
fn table_cells_with_several_lines_use_breaks() {
    let written = md(vec![Block::Table(vec![
        TableRow {
            cells: vec![cell(true, "h")],
        },
        TableRow {
            cells: vec![TableCell {
                blocks: vec![para("one"), para("two")],
                ..cell(false, "")
            }],
        },
    ])]);
    assert!(written.contains("| one<br>two |"), "{written}");
    assert_eq!(
        from_markdown(&written).blocks,
        vec![Block::Table(vec![
            TableRow {
                cells: vec![cell(true, "h")],
            },
            TableRow {
                cells: vec![TableCell {
                    blocks: vec![Block::Paragraph(vec![
                        text("one"),
                        Inline::HardBreak,
                        text("two")
                    ])],
                    ..cell(false, "")
                }],
            },
        ])]
    );
}

#[test]
fn rows_shorter_than_the_header_are_padded() {
    let written = md(vec![Block::Table(vec![
        TableRow {
            cells: vec![cell(true, "a"), cell(true, "b")],
        },
        TableRow {
            cells: vec![cell(false, "1")],
        },
    ])]);
    assert!(written.contains("| 1 |  |"), "{written}");
}

#[test]
fn hard_breaks_and_soft_breaks() {
    assert_eq!(
        from_markdown("a\nb  \nc").blocks,
        vec![Block::Paragraph(vec![
            text("a b"),
            Inline::HardBreak,
            text("c")
        ])]
    );
}

#[test]
fn empty_input_gives_an_empty_document() {
    assert_eq!(from_markdown(""), Document::default());
    assert_eq!(md(vec![]), "");
}

// Found in review.

#[test]
fn entities_in_addresses_stay_literal() {
    let written = md(vec![Block::Paragraph(vec![marked(
        "x",
        Marks {
            link: Some(Link {
                title: Some("a &amp; b".into()),
                ..link("javascript&#58;alert(1)")
            }),
            ..Marks::default()
        },
    )])]);
    assert!(!written.contains("](javascript&#58;"), "{written}");
    round_trip(vec![Block::Paragraph(vec![marked(
        "x",
        Marks {
            link: Some(Link {
                title: Some("a &amp; b".into()),
                ..link("?a=1&amp;b=2")
            }),
            ..Marks::default()
        },
    )])]);
}

#[test]
fn a_heading_keeps_its_final_hashes() {
    round_trip(vec![
        Block::Heading {
            level: 2,
            content: vec![text("Item #")],
        },
        Block::Heading {
            level: 1,
            content: vec![text("a ##")],
        },
    ]);
}

#[test]
fn leading_spaces_do_not_start_a_block() {
    round_trip(vec![
        para("    four spaces"),
        para("\ttab"),
        para(" # not a heading"),
        para(" - not a list"),
        Block::Paragraph(vec![
            text("a"),
            Inline::HardBreak,
            text(" # b"),
            Inline::HardBreak,
            text("  - c"),
        ]),
    ]);
}

#[test]
fn a_final_hard_break_stays_a_break() {
    round_trip(vec![
        Block::Paragraph(vec![text("foo"), Inline::HardBreak]),
        para("next"),
    ]);
}

#[test]
fn a_pipe_in_code_stays_in_its_cell() {
    let code = Marks {
        code: true,
        ..Marks::default()
    };
    round_trip(vec![Block::Table(vec![
        TableRow {
            cells: vec![cell(true, "a"), cell(true, "b")],
        },
        TableRow {
            cells: vec![
                TableCell {
                    blocks: vec![Block::Paragraph(vec![marked("x|y", code)])],
                    ..cell(false, "")
                },
                cell(false, "z"),
            ],
        },
    ])]);
}

#[test]
fn marks_next_to_punctuation_keep_their_bounds() {
    let strike = Marks {
        strike: true,
        ..Marks::default()
    };
    round_trip(vec![
        Block::Paragraph(vec![text("foo"), marked("(bar)", bold()), text("baz")]),
        Block::Paragraph(vec![marked("world,", bold()), text("x")]),
        Block::Paragraph(vec![text("a"), marked("\"b\"", italic()), text("c")]),
        Block::Paragraph(vec![text("a"), marked("-b-", strike), text("c")]),
        Block::Paragraph(vec![marked("(x)", bold())]),
    ]);
}

#[test]
fn empty_paragraphs_are_kept_and_keep_lists_apart() {
    let item = |value| ListItem {
        blocks: vec![para(value)],
    };
    round_trip(vec![
        Block::BulletList(vec![item("a")]),
        Block::Paragraph(vec![]),
        Block::BulletList(vec![item("b")]),
        Block::Paragraph(vec![]),
    ]);
}

#[test]
fn blocks_in_a_summary_are_kept() {
    let read = from_markdown(
        "<details>\n<summary>\n\n## title\n\n- item\n\n</summary>\n\nbody\n\n</details>",
    );
    assert_eq!(
        read.blocks,
        vec![Block::Details {
            summary: vec![text("title")],
            content: vec![
                Block::BulletList(vec![ListItem {
                    blocks: vec![para("item")],
                }]),
                para("body"),
            ],
        }]
    );
}

#[test]
fn an_ordered_list_never_starts_below_zero() {
    let written = md(vec![Block::OrderedList {
        start: -1,
        kind: None,
        items: vec![ListItem {
            blocks: vec![para("x")],
        }],
    }]);
    assert!(written.starts_with("0. x"), "{written}");
}

#[test]
fn an_item_starting_with_a_list_keeps_it() {
    round_trip(vec![Block::BulletList(vec![ListItem {
        blocks: vec![
            Block::Paragraph(vec![]),
            Block::BulletList(vec![ListItem {
                blocks: vec![para("a")],
            }]),
        ],
    }])]);
}

#[test]
fn marks_written_as_tags_inside_others_keep_their_bounds() {
    let under = Marks {
        bold: true,
        underline: true,
        ..Marks::default()
    };
    let both = Marks {
        bold: true,
        italic: true,
        ..Marks::default()
    };
    round_trip(vec![
        Block::Paragraph(vec![text("foo"), marked("bar", under), text("baz")]),
        Block::Paragraph(vec![marked("a", bold()), marked("(b)c", both), text("d")]),
    ]);
}
