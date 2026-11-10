//! The Rust schema against the editor's, which `npm --prefix web run schema` writes to
//! `tiptap-schema.json` and `npm --prefix web run build` checks is current (ADR 0008).

use cadocs_content::schema::{self, Default};
use serde_json::{Map, Value, json};

fn default(value: &Default) -> Value {
    match value {
        Default::Null => Value::Null,
        Default::Bool(b) => json!(b),
        Default::Int(n) => json!(n),
        Default::Str(s) => json!(s),
    }
}

fn attrs(list: &[(&str, Default)]) -> Value {
    Value::Object(
        list.iter()
            .map(|(name, value)| (name.to_string(), default(value)))
            .collect(),
    )
}

fn rust_schema() -> Value {
    let nodes: Map<String, Value> = schema::NODES
        .iter()
        .map(|node| {
            let spec = json!({
                "content": node.content,
                "group": node.group,
                "inline": node.inline,
                "atom": node.atom,
                "attrs": attrs(node.attrs),
            });
            (node.name.to_string(), spec)
        })
        .collect();
    let marks: Map<String, Value> = schema::MARKS
        .iter()
        .map(|mark| (mark.name.to_string(), json!({ "attrs": attrs(mark.attrs) })))
        .collect();
    json!({ "nodes": nodes, "marks": marks })
}

#[test]
fn the_rust_schema_matches_the_editor() {
    let editor: Value =
        serde_json::from_str(include_str!("tiptap-schema.json")).expect("the file is JSON");
    assert_eq!(rust_schema(), editor);
}

#[test]
fn the_schema_has_no_duplicate_name() {
    assert_eq!(
        rust_schema()["nodes"].as_object().unwrap().len(),
        schema::NODES.len()
    );
    assert_eq!(
        rust_schema()["marks"].as_object().unwrap().len(),
        schema::MARKS.len()
    );
}
