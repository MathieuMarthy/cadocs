//! Fills a database with benchmark documents, and the account that opens them.

use cadocs_store::{
    AccountStore, DocEdit, DocStore, FirstUser, NewPage, NewSession, NewSpace, PageStore,
    Placement, Snapshot, SpaceStore, StoredUpdate, now_ms,
};
use yrs::updates::decoder::Decode;
use yrs::{Doc, Transact, Update};

use crate::document::{encode_state, generate};

/// The server's default for `CADOCS_COMPACT_UPDATES`.
pub const COMPACT_UPDATES: usize = 500;

/// The session of every load client, as the cookie sends it.
pub const SESSION_TOKEN: &str = "abc";
/// SHA-256 of `SESSION_TOKEN`, as a session row holds it. A test compares it with the server's.
pub const SESSION_TOKEN_HASH: &str =
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
const USER: &str = "bench-user";
const WORKSPACE: &str = "bench-workspace";
const SPACE: &str = "bench-space";

pub fn doc_id(index: usize) -> String {
    format!("bench-{index}")
}

/// The first user of an empty database, with a session, a workspace and a space.
async fn seed_account<S: AccountStore + SpaceStore>(store: &S) -> anyhow::Result<()> {
    let now = now_ms();
    let user = FirstUser {
        id: USER.to_owned(),
        email: "bench@example.org".to_owned(),
        name: "Bench".to_owned(),
        password_hash: "unused".to_owned(),
        workspace_id: WORKSPACE.to_owned(),
        workspace_slug: WORKSPACE.to_owned(),
        workspace_name: "Bench".to_owned(),
    };
    let session = NewSession {
        token_hash: SESSION_TOKEN_HASH.to_owned(),
        user_id: USER.to_owned(),
        now,
        expires_at: i64::MAX,
    };
    store.create_first_user(&user, &session).await?;
    let space = NewSpace {
        id: SPACE.to_owned(),
        workspace_id: WORKSPACE.to_owned(),
        slug: SPACE.to_owned(),
        name: "Bench".to_owned(),
        description: String::new(),
        default_role: None,
        actor: USER.to_owned(),
        now,
    };
    store.create_space(&space).await?;
    Ok(())
}

/// Stores `count` documents the way a room would: one row per edit, and a compaction each time
/// the log passes `compact_updates`. `None` keeps the whole log. Document `i` uses `seed + i`.
pub async fn seed<S: AccountStore + DocStore + PageStore + SpaceStore>(
    store: &S,
    count: usize,
    seed: u64,
    compact_updates: Option<usize>,
) -> anyhow::Result<()> {
    seed_account(store).await?;
    for index in 0..count {
        let id = doc_id(index);
        let page = NewPage {
            id: id.clone(),
            space_id: SPACE.to_owned(),
            parent_id: None,
            placement: Placement::Last,
            slug: id.clone(),
            actor: USER.to_owned(),
            now: now_ms(),
        };
        store.create_page(&page).await?;
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
            store
                .append_updates(&id, &rows, &DocEdit::default())
                .await?;

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
