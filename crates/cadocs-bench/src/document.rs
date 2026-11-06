//! The benchmark document: about 20 KB of text, about 3,000 words, from 2,000 small edits.

use yrs::types::xml::XmlOut;
use yrs::{
    ClientID, Doc, GetString, Options, ReadTxn, Text, Transact, XmlElementPrelim, XmlFragment,
    XmlTextPrelim, XmlTextRef,
};

use crate::rng::Rng;

pub const EDITS: usize = 2_000;
/// Root types, as in `docs/DATA_MODEL.md`.
pub const TITLE: &str = "title";
pub const BODY: &str = "default";

const PARAGRAPH: &str = "paragraph";
const PARAGRAPH_WORDS: usize = 60;

/// Averages about 5.7 letters, for about 6.7 bytes per word with its space.
const WORDS: &[&str] = &[
    "server",
    "memory",
    "backup",
    "editor",
    "client",
    "update",
    "shared",
    "folder",
    "system",
    "review",
    "stored",
    "writer",
    "reader",
    "socket",
    "branch",
    "manual",
    "report",
    "policy",
    "and",
    "the",
    "with",
    "from",
    "page",
    "team",
    "note",
    "plan",
    "wiki",
    "each",
    "room",
    "data",
    "document",
    "workspace",
    "permission",
    "collaborate",
    "attachment",
    "revision",
    "database",
];

/// One document, as the edits that build it.
pub struct BenchDoc {
    pub doc: Doc,
    /// One Yjs update v1 per edit.
    pub updates: Vec<Vec<u8>>,
}

fn word(rng: &mut Rng) -> &'static str {
    WORDS[rng.below(WORDS.len())]
}

/// An id Yjs accepts, which stops at 53 bits.
pub fn client_id(rng: &mut Rng) -> ClientID {
    ClientID::new((rng.next_u64() >> 12) | 1)
}

fn new_paragraph(doc: &Doc, rng: &mut Rng) -> (XmlTextRef, Vec<u8>) {
    let body = doc.get_or_insert_xml_fragment(BODY);
    let mut txn = doc.transact_mut();
    let index = body.len(&txn);
    let paragraph = body.insert(&mut txn, index, XmlElementPrelim::empty(PARAGRAPH));
    let text = paragraph.insert(&mut txn, 0, XmlTextPrelim::new(word(rng)));
    let update = txn.encode_update_v1();
    (text, update)
}

/// Types one to three words at the end of a paragraph.
fn append(doc: &Doc, text: &XmlTextRef, rng: &mut Rng) -> (Vec<u8>, usize) {
    let count = 1 + rng.below(3);
    let mut typed = String::new();
    for _ in 0..count {
        typed.push(' ');
        typed.push_str(word(rng));
    }
    let mut txn = doc.transact_mut();
    let end = text.len(&txn);
    text.insert(&mut txn, end, &typed);
    (txn.encode_update_v1(), count)
}

/// Adds a word between two words of an earlier paragraph.
fn insert_word(doc: &Doc, text: &XmlTextRef, rng: &mut Rng) -> Vec<u8> {
    let mut txn = doc.transact_mut();
    let current = text.get_string(&txn);
    let from = rng.below(current.len());
    let at = current[from..]
        .find(' ')
        .map_or(current.len(), |i| from + i);
    text.insert(&mut txn, at as u32, &format!(" {}", word(rng)));
    txn.encode_update_v1()
}

/// Removes a few characters, as a backspace would. This leaves tombstones in the document.
fn backspace(doc: &Doc, text: &XmlTextRef, rng: &mut Rng) -> Vec<u8> {
    let mut txn = doc.transact_mut();
    let length = text.len(&txn);
    let count = (1 + rng.below(3) as u32).min(length);
    let at = rng.below((length - count + 1) as usize) as u32;
    text.remove_range(&mut txn, at, count);
    txn.encode_update_v1()
}

/// Generates the document of `seed`. The same seed always gives the same bytes.
pub fn generate(seed: u64) -> BenchDoc {
    let mut rng = Rng::new(seed);
    let doc = Doc::with_options(Options {
        client_id: client_id(&mut rng),
        ..Options::default()
    });
    let mut updates = Vec::with_capacity(EDITS);

    let title = doc.get_or_insert_text(TITLE);
    let first = {
        let mut txn = doc.transact_mut();
        title.insert(&mut txn, 0, &format!("Bench page {seed}"));
        txn.encode_update_v1()
    };
    updates.push(first);

    let (mut current, update) = new_paragraph(&doc, &mut rng);
    updates.push(update);
    let mut paragraphs = vec![current.clone()];
    let mut words = 1;

    while updates.len() < EDITS {
        let earlier = paragraphs[rng.below(paragraphs.len())].clone();
        let update = match rng.below(100) {
            _ if words >= PARAGRAPH_WORDS => {
                let (text, update) = new_paragraph(&doc, &mut rng);
                current = text;
                paragraphs.push(current.clone());
                words = 1;
                update
            }
            0..8 => backspace(&doc, &earlier, &mut rng),
            8..40 => insert_word(&doc, &earlier, &mut rng),
            _ => {
                let (update, count) = append(&doc, &current, &mut rng);
                words += count;
                update
            }
        };
        updates.push(update);
    }
    BenchDoc { doc, updates }
}

/// The text of every paragraph, one per line.
pub fn body_text(doc: &Doc) -> String {
    let body = doc.get_or_insert_xml_fragment(BODY);
    let txn = doc.transact();
    let mut lines = Vec::new();
    for index in 0..body.len(&txn) {
        if let Some(XmlOut::Element(paragraph)) = body.get(&txn, index)
            && let Some(XmlOut::Text(text)) = paragraph.get(&txn, 0)
        {
            lines.push(text.get_string(&txn));
        }
    }
    lines.join("\n")
}

/// The whole document as one update, as a room encodes it for a snapshot.
pub fn encode_state(doc: &Doc) -> Vec<u8> {
    doc.transact()
        .encode_state_as_update_v1(&yrs::StateVector::default())
}
