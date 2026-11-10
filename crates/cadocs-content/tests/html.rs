mod common;

use cadocs_content::ast::*;
use cadocs_content::html::to_html;
use common::*;

fn titles(id: &str) -> Option<String> {
    (id == "0190a0b0-0000-7000-8000-000000000001").then(|| "Other <page>".to_string())
}

fn html(blocks: Vec<Block>) -> String {
    to_html(&Document { blocks }, &|_: &str| None)
}

#[test]
fn every_node_renders() {
    let out = to_html(&every_node(), &titles);
    for part in [
        r#"<h1 id="h-01">Title</h1>"#,
        "<strong>bold</strong>",
        "<em>italic</em>",
        "<s>struck</s>",
        "<u>under</u>",
        "<code>code</code>",
        r#"<a href="https://example.com/a" rel="noopener noreferrer nofollow">a link</a>"#,
        "<br>",
        r#"<a class="page-link" href="/?page=0190a0b0-0000-7000-8000-000000000001">Other &lt;page&gt;</a>"#,
        "<ul><li><p>one</p><ul><li><p>nested</p></li></ul></li><li><p>two</p></li></ul>",
        r#"<ol start="3">"#,
        r#"<ul class="tasks"><li class="task" data-checked="true"><input type="checkbox" checked disabled> <div><p>done</p></div></li>"#,
        r#"<pre><code class="language-rust">fn main() {"#,
        "println!(&quot;``&quot;);",
        "<blockquote><p>quoted</p></blockquote>",
        "<hr>",
        r#"<aside class="callout" data-kind="warning"><p>careful</p></aside>"#,
        "<details><summary>more</summary><p>hidden</p></details>",
        "<table><tbody><tr><th><p>name</p></th><th><p>value</p></th></tr><tr><td><p>a</p></td><td><p>1</p></td></tr></tbody></table>",
        r##"<nav class="toc" aria-label="table of contents"><ol><li data-depth="0"><a href="#h-01"><span class="toc-number">01</span> Title</a></li><li data-depth="1"><a href="#h-01.1"><span class="toc-number">01.1</span> Lists</a></li><li data-depth="2"><a href="#h-01.1.1"><span class="toc-number">01.1.1</span> Last</a></li></ol></nav>"##,
        r#"<figure class="image"><img src="/api/v1/attachments/0190a0b0-0000-7000-8000-00000000000a" alt="a diagram"></figure>"#,
        r#"<p class="file"><a class="file" href="/api/v1/attachments/0190a0b0-0000-7000-8000-00000000000f" download>report [final].pdf</a></p>"#,
        r#"<div class="plugin" data-kind="kanban/board">todo, done</div>"#,
        r#"<h3 id="h-01.1.1"><strong>Last</strong></h3>"#,
    ] {
        assert!(out.contains(part), "missing {part}\nin {out}");
    }
}

#[test]
fn text_and_attributes_are_escaped() {
    let out = html(vec![
        para(r#"<script>alert("x")</script> & 'y'"#),
        Block::CodeBlock {
            language: Some(r#"x" onload="alert(1)"#.into()),
            code: "</code><script>".into(),
        },
        Block::Callout {
            kind: r#""><script>"#.into(),
            blocks: vec![para("k")],
        },
        Block::Plugin {
            kind: r#""><img src=x onerror=alert(1)>"#.into(),
            data: None,
            text: "<b>fallback</b>".into(),
        },
        Block::Paragraph(vec![marked(
            "t",
            Marks {
                link: Some(Link {
                    title: Some(r#"" onmouseover="alert(1)"#.into()),
                    ..link(r#"https://example.com/"><script>"#)
                }),
                ..Marks::default()
            },
        )]),
    ]);
    assert!(!out.contains("<script"), "{out}");
    assert!(!out.contains("<img"), "{out}");
    assert!(!out.contains("<b>"), "{out}");
    assert!(!out.contains(r#"" on"#), "{out}");
    assert!(
        out.contains("&lt;script&gt;alert(&quot;x&quot;)&lt;/script&gt; &amp; &#39;y&#39;"),
        "{out}"
    );
    // A language is a class: anything else than a plain name is left out.
    assert!(
        out.contains("<pre><code>&lt;/code&gt;&lt;script&gt;</code></pre>"),
        "{out}"
    );
}

#[test]
fn a_link_outside_the_allow_list_keeps_only_its_text() {
    let out = html(vec![Block::Paragraph(vec![marked(
        "click",
        Marks {
            link: Some(link("javascript:alert(1)")),
            ..Marks::default()
        },
    )])]);
    assert_eq!(out, "<p>click</p>");
    let out = html(vec![Block::Paragraph(vec![marked(
        "x",
        Marks {
            link: Some(link("jav&#x09;ascript:alert(1)")),
            ..Marks::default()
        },
    )])]);
    assert_eq!(
        out,
        r#"<p><a href="jav&amp;#x09;ascript:alert(1)" rel="noopener noreferrer nofollow">x</a></p>"#
    );
}

#[test]
fn a_stored_class_or_target_is_not_rendered() {
    let out = html(vec![Block::Paragraph(vec![marked(
        "x",
        Marks {
            link: Some(Link {
                class: Some("admin-only".into()),
                target: Some("_self".into()),
                ..link("https://example.com")
            }),
            ..Marks::default()
        },
    )])]);
    assert!(
        !out.contains("admin-only") && !out.contains("_self"),
        "{out}"
    );
}

#[test]
fn a_page_the_reader_cannot_open_shows_no_title() {
    let out = html(vec![Block::Paragraph(vec![Inline::PageLink {
        id: "secret".into(),
    }])]);
    assert_eq!(
        out,
        r#"<p><span class="page-link page-link-missing">page not found</span></p>"#
    );
}

#[test]
fn attachment_ids_and_names_are_escaped() {
    let out = html(vec![
        Block::Image {
            id: r#"a"&b/c"#.into(),
            alt: Some(r#""><script>"#.into()),
        },
        Block::File {
            id: "../x".into(),
            name: "<script>".into(),
        },
    ]);
    assert!(
        out.contains(r#"src="/api/v1/attachments/a%22%26b%2Fc" alt="&quot;&gt;&lt;script&gt;""#),
        "{out}"
    );
    assert!(
        out.contains(r#"href="/api/v1/attachments/..%2Fx" download>&lt;script&gt;</a>"#),
        "{out}"
    );
}

#[test]
fn page_link_ids_are_encoded() {
    let out = to_html(
        &Document {
            blocks: vec![Block::Paragraph(vec![Inline::PageLink {
                id: r#"a"&b/c"#.into(),
            }])],
        },
        &|_: &str| Some("t".to_string()),
    );
    assert!(out.contains(r#"href="/?page=a%22%26b%2Fc""#), "{out}");
}

#[test]
fn cells_keep_their_spans_and_alignment() {
    let out = html(vec![Block::Table(vec![TableRow {
        cells: vec![TableCell {
            colspan: 2,
            rowspan: 3,
            align: Some("center".into()),
            ..cell(false, "x")
        }],
    }])]);
    assert!(
        out.contains(r#"<td colspan="2" rowspan="3" data-align="center">"#),
        "{out}"
    );
    let out = html(vec![Block::Table(vec![TableRow {
        cells: vec![TableCell {
            align: Some(r#"" onclick="x"#.into()),
            ..cell(false, "x")
        }],
    }])]);
    assert!(out.contains("<td><p>x</p></td>"), "{out}");
}

#[test]
fn an_ordered_list_keeps_only_known_types() {
    let out = html(vec![Block::OrderedList {
        start: 1,
        kind: Some("a".into()),
        items: vec![ListItem {
            blocks: vec![para("x")],
        }],
    }]);
    assert!(out.starts_with(r#"<ol type="a">"#), "{out}");
    let out = html(vec![Block::OrderedList {
        start: 1,
        kind: Some(r#"" x="#.into()),
        items: vec![ListItem {
            blocks: vec![para("x")],
        }],
    }]);
    assert!(out.starts_with("<ol>"), "{out}");
}

#[test]
fn plugin_text_keeps_its_lines() {
    let out = html(vec![Block::Plugin {
        kind: "k".into(),
        data: None,
        text: "a\nb".into(),
    }]);
    assert_eq!(out, r#"<div class="plugin" data-kind="k">a<br>b</div>"#);
}
