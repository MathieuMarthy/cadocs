//! Fills a database with benchmark documents.

use cadocs_store::{DocStore, Snapshot, StoredUpdate};
use yrs::updates::decoder::Decode;
use yrs::{Doc, Transact, Update};

use crate::document::{encode_state, generate};

/// The server's default for `CADOCS_COMPACT_UPDATES`.
pub const COMPACT_UPDATES: usize = 500;

pub fn doc_id(index: usize) -> String {
    format!("bench-{index}")
}

/// Stores `count` documents the way a room would: one row per edit, and a compaction each time
/// the log passes `compact_updates`. `None` keeps the whole log. Document `i` uses `seed + i`.
pub async fn seed<S: DocStore>(
    store: &S,
    count: usize,
    seed: u64,
    compact_updates: Option<usize>,
) -> anyhow::Result<()> {
    for index in 0..count {
        let id = doc_id(index);
        let updates = generate(seed + index as u64).updates;
        let batch = compact_updates.map_or(updates.len(), |limit| limit + 1);
        let replayed = Doc::new();
        let mut seq = 0;

        for chunk in updates.chunks(batch) {
            let mut rows = Vec::with_capacity(chunk.len());
            let mut txn = replayed.transact_mut();
            for data in chunk {
                seq += 1;
                txn.apply_update(Update::decode_v1(data)?)?;
                rows.push(StoredUpdate {
                    seq,
                    data: data.clone(),
                });
            }
            drop(txn);
            store.append_updates(&id, &rows).await?;

            if compact_updates.is_some() && chunk.len() == batch {
                let snapshot = Snapshot {
                    state: encode_state(&replayed),
                    upto_seq: seq,
                };
                store.compact(&id, &snapshot).await?;
            }
        }
    }
    Ok(())
}
