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
        "todo, done",
        "Last",
    ] {
        assert!(text.contains(part), "missing {part:?} in {text:?}");
    }
    // A page link's title belongs to another page, which a reader of this one may not see.
    assert!(!text.contains("0190a0b0"));
}
