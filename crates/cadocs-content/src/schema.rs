//! The editor's schema, defined once for the server (ADR 0008). `tests/schema.rs` compares it
//! with the one `web/` exports, so a block added to one side only fails a test.

/// The default value of an attribute, as the editor declares it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Default {
    Null,
    Bool(bool),
    Int(i64),
    Str(&'static str),
}

#[derive(Debug)]
pub struct NodeSpec {
    pub name: &'static str,
    /// ProseMirror's content expression.
    pub content: &'static str,
    pub group: &'static str,
    pub inline: bool,
    pub atom: bool,
    pub attrs: &'static [(&'static str, Default)],
}

#[derive(Debug)]
pub struct MarkSpec {
    pub name: &'static str,
    pub attrs: &'static [(&'static str, Default)],
}

const fn block(
    name: &'static str,
    content: &'static str,
    group: &'static str,
    attrs: &'static [(&'static str, Default)],
) -> NodeSpec {
    NodeSpec {
        name,
        content,
        group,
        inline: false,
        atom: content.is_empty(),
        attrs,
    }
}

const fn inline(name: &'static str) -> NodeSpec {
    NodeSpec {
        name,
        content: "",
        group: "inline",
        inline: true,
        atom: true,
        attrs: &[],
    }
}

const CELL: &[(&str, Default)] = &[
    ("align", Default::Null),
    ("colspan", Default::Int(1)),
    ("colwidth", Default::Null),
    ("rowspan", Default::Int(1)),
];

pub const NODES: &[NodeSpec] = &[
    block("doc", "block+", "", &[]),
    block("paragraph", "inline*", "block", &[]),
    inline("text"),
    inline("hardBreak"),
    NodeSpec {
        attrs: &[("id", Default::Null)],
        ..inline("pageLink")
    },
    block("heading", "inline*", "block", &[("level", Default::Int(1))]),
    block("bulletList", "listItem+", "block list", &[]),
    block(
        "orderedList",
        "listItem+",
        "block list",
        &[("start", Default::Int(1)), ("type", Default::Null)],
    ),
    block("listItem", "paragraph block*", "", &[]),
    block("taskList", "taskItem+", "block list", &[]),
    block(
        "taskItem",
        "paragraph block*",
        "",
        &[("checked", Default::Bool(false))],
    ),
    block(
        "codeBlock",
        "text*",
        "block",
        &[("language", Default::Null)],
    ),
    block("blockquote", "block+", "block", &[]),
    block("horizontalRule", "", "block", &[]),
    block(
        "callout",
        "block+",
        "block",
        &[("kind", Default::Str("note"))],
    ),
    block("details", "detailsSummary detailsContent", "block", &[]),
    block("detailsSummary", "text*", "", &[]),
    block("detailsContent", "block+", "", &[]),
    block("table", "tableRow+", "block", &[]),
    block("tableRow", "(tableCell | tableHeader)*", "", &[]),
    block("tableCell", "block+", "", CELL),
    block("tableHeader", "block+", "", CELL),
    block("tableOfContents", "", "block", &[]),
    block(
        "plugin",
        "",
        "block",
        &[
            ("data", Default::Null),
            ("kind", Default::Str("")),
            ("text", Default::Str("")),
        ],
    ),
];

pub const MARKS: &[MarkSpec] = &[
    MarkSpec {
        name: "bold",
        attrs: &[],
    },
    MarkSpec {
        name: "code",
        attrs: &[],
    },
    MarkSpec {
        name: "italic",
        attrs: &[],
    },
    MarkSpec {
        name: "link",
        attrs: &[
            ("class", Default::Null),
            ("href", Default::Null),
            ("rel", Default::Str("noopener noreferrer nofollow")),
            ("target", Default::Str("_blank")),
            ("title", Default::Null),
        ],
    },
    MarkSpec {
        name: "strike",
        attrs: &[],
    },
    MarkSpec {
        name: "underline",
        attrs: &[],
    },
];
