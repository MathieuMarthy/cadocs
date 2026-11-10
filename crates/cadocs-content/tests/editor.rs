//! Against what the editor's binding really writes: `every-node.bin` is the Yjs update of
//! `every-node.json`, written by `@tiptap/y-tiptap` through `npm --prefix web run schema`.

mod common;

use cadocs_content::yjs;
use yrs::types::text::YChange;
use yrs::updates::decoder::Decode;
use yrs::{Doc, Text, Transact, Update, Xml, XmlFragment, XmlOut};

fn editor_doc() -> Doc {
    let doc = Doc::new();
    let update = Update::decode_v1(include_bytes!("every-node.bin")).expect("a Yjs update");
    doc.transact_mut().apply_update(update).expect("applies");
    doc
}

/// The tree under `node`, with attributes and formatting in a stable order.
fn dump(node: &impl XmlFragment, txn: &yrs::Transaction, out: &mut Vec<String>, depth: usize) {
    for child in node.children(txn) {
        let indent = "  ".repeat(depth);
        match child {
            XmlOut::Element(element) => {
                let mut attrs: Vec<String> = element
                    .attributes(txn)
                    .map(|(k, v)| format!("{k}={}", v.to_string(txn)))
                    .collect();
                attrs.sort();
                out.push(format!("{indent}<{} {}>", element.tag(), attrs.join(" ")));
                dump(&element, txn, out, depth + 1);
            }
            XmlOut::Text(text) => {
                for diff in text.diff(txn, YChange::identity) {
                    let mut attrs: Vec<String> = diff
                        .attributes
                        .iter()
                        .flat_map(|a| a.iter())
                        .map(|(k, v)| {
                            // Maps print in hash order: sort their entries too.
                            let mut entries: Vec<String> = match v {
                                yrs::Any::Map(map) => {
                                    map.iter().map(|(a, b)| format!("{a}:{b}")).collect()
                                }
                                other => vec![other.to_string()],
                            };
                            entries.sort();
                            format!("{k}={}", entries.join(","))
                        })
                        .collect();
                    attrs.sort();
                    out.push(format!(
                        "{indent}{:?} [{}]",
                        diff.insert.to_string(txn),
                        attrs.join(" ")
                    ));
                }
            }
            XmlOut::Fragment(_) => {}
        }
    }
}

fn tree(doc: &Doc) -> Vec<String> {
    let fragment = doc.get_or_insert_xml_fragment(yjs::FRAGMENT);
    let txn = doc.transact();
    let mut out = Vec::new();
    dump(&fragment, &txn, &mut out, 0);
    out
}

#[test]
fn what_the_editor_writes_reads_as_every_node() {
    assert_eq!(yjs::read_doc(&editor_doc()), common::every_node());
}

#[test]
fn the_server_writes_what_the_editor_writes() {
    let editor = editor_doc();
    let server = Doc::new();
    let fragment = server.get_or_insert_xml_fragment(yjs::FRAGMENT);
    yjs::write(
        &mut server.transact_mut(),
        &fragment,
        &yjs::read_doc(&editor),
    );
    assert_eq!(tree(&server), tree(&editor));
}
