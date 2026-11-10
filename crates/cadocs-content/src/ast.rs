//! The content of a page, as the editor's schema shapes it ([`crate::schema`]).
//!
//! The types are looser than the schema: a list item may start with any block, a container may
//! be empty. The Yjs writer fixes that up, so anything built here writes valid content.

/// The `default` fragment of a page.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Block {
    Paragraph(Vec<Inline>),
    /// `level` is 1 to 6. The editor offers 1 to 3.
    Heading {
        level: u8,
        content: Vec<Inline>,
    },
    BulletList(Vec<ListItem>),
    /// `kind` is the HTML `type` of the list (`a`, `I`...), unset by the editor so far.
    OrderedList {
        start: i64,
        kind: Option<String>,
        items: Vec<ListItem>,
    },
    TaskList(Vec<TaskItem>),
    CodeBlock {
        language: Option<String>,
        code: String,
    },
    Blockquote(Vec<Block>),
    HorizontalRule,
    /// `kind` is `note`, `warning` or `danger`, but is kept as read.
    Callout {
        kind: String,
        blocks: Vec<Block>,
    },
    /// A toggle. The summary holds text only.
    Details {
        summary: Vec<Inline>,
        content: Vec<Block>,
    },
    Table(Vec<TableRow>),
    /// The headings of the page, listed where it stands.
    TableOfContents,
    /// A content plugin's block (ADR 0016). `data` is the plugin's JSON, as text, and `text` is
    /// what every renderer shows instead of running the plugin.
    Plugin {
        kind: String,
        data: Option<String>,
        text: String,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ListItem {
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TaskItem {
    pub checked: bool,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableRow {
    pub cells: Vec<TableCell>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableCell {
    /// `tableHeader` rather than `tableCell`.
    pub header: bool,
    pub colspan: u32,
    pub rowspan: u32,
    /// Stays `None` while the editor's tables are not resizable.
    pub colwidth: Option<Vec<i64>>,
    /// `left`, `center` or `right`.
    pub align: Option<String>,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Inline {
    /// Never empty once read. Line breaks are [`Inline::HardBreak`].
    Text {
        text: String,
        marks: Marks,
    },
    HardBreak,
    /// A link to another page, by id only: its title is looked up when rendered.
    PageLink {
        id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Marks {
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
    pub underline: bool,
    pub code: bool,
    pub link: Option<Link>,
}

impl Marks {
    pub fn is_empty(&self) -> bool {
        *self == Marks::default()
    }
}

/// The attributes of a `link` mark, as stored. Renderers only use `href` and `title`.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    pub href: String,
    pub target: Option<String>,
    pub rel: Option<String>,
    pub class: Option<String>,
    pub title: Option<String>,
}

impl Link {
    /// A link as the editor makes one, with its default attributes.
    pub fn new(href: impl Into<String>, title: Option<String>) -> Self {
        Link {
            href: href.into(),
            target: Some("_blank".into()),
            rel: Some("noopener noreferrer nofollow".into()),
            class: None,
            title,
        }
    }
}

/// Appends `inline`, merging it into the text before it when their marks are equal, and
/// dropping empty text.
pub(crate) fn push_inline(list: &mut Vec<Inline>, inline: Inline) {
    if let Inline::Text { text, marks } = &inline {
        if text.is_empty() {
            return;
        }
        if let Some(Inline::Text {
            text: last,
            marks: last_marks,
        }) = list.last_mut()
            && last_marks == marks
        {
            last.push_str(text);
            return;
        }
    }
    list.push(inline);
}
