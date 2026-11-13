//! Fills a database with benchmark pages, and the accounts that open them.

use argon2::Argon2;
use argon2::password_hash::PasswordHasher;
use cadocs_content::text::to_text;
use cadocs_content::yjs::read_page;
use cadocs_core::permissions::{DefaultRole, WorkspaceRole};
use cadocs_search::{Analyzer, Language};
use cadocs_store::{
    AccountStore, DocEdit, DocStore, FirstUser, InvitedUser, JobStore, NewInvitation, NewPage,
    NewSession, NewSpace, PageIndex, PageStore, Placement, SearchStore, Snapshot, SpaceStore,
    StoredUpdate, WorkspaceStore, now_ms,
};
use sha2::{Digest, Sha256};
use yrs::updates::decoder::Decode;
use yrs::{Doc, Transact, Update};

use crate::document::{encode_state, generate};

/// The server's default for `CADOCS_COMPACT_UPDATES`.
pub const COMPACT_UPDATES: usize = 500;

/// The session of the first user, as the cookie sends it.
pub const SESSION_TOKEN: &str = "abc";
/// SHA-256 of `SESSION_TOKEN`, as a session row holds it. A test compares it with the server's.
pub const SESSION_TOKEN_HASH: &str =
    "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad";
/// Every seeded account logs in with it.
pub const BENCH_PASSWORD: &str = "bench-password";
pub const WORKSPACE: &str = "bench-workspace";
pub const SPACE: &str = "bench-space";
const OWNER: &str = "bench-user";

pub fn doc_id(index: usize) -> String {
    format!("bench-{index}")
}

fn user_id(user: usize) -> String {
    if user == 0 {
        OWNER.to_owned()
    } else {
        format!("bench-user-{user}")
    }
}

pub fn user_email(user: usize) -> String {
    if user == 0 {
        "bench@example.org".to_owned()
    } else {
        format!("bench-{user}@example.org")
    }
}

/// The session the seed opens for `user`, as the cookie sends it.
pub fn session_token(user: usize) -> String {
    if user == 0 {
        SESSION_TOKEN.to_owned()
    } else {
        format!("bench-session-{user}")
    }
}

/// What the server stores of a token: SHA-256, in lowercase hexadecimal.
pub fn token_hash(token: &str) -> String {
    Sha256::digest(token)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// The parent of page `index` in a tree where each page has `fanout` children: the first
/// `fanout` pages are at the root. `None` keeps every page at the root.
pub fn parent_of(index: usize, fanout: Option<usize>) -> Option<usize> {
    let fanout = fanout?;
    (index >= fanout).then(|| index / fanout - 1)
}

/// What to seed. `Seed::docs(n)` is the scenario of stage 0: `n` documents at the root, one user.
#[derive(Debug, Clone)]
pub struct Seed {
    pub pages: usize,
    /// The first `docs` pages hold a benchmark document, the others are empty.
    pub docs: usize,
    /// The first is the instance admin and workspace owner, the others are members.
    pub users: usize,
    pub fanout: Option<usize>,
    /// Indexes each document for search, as the reindex job would.
    pub index: bool,
    /// Document `i` uses `seed + i`.
    pub seed: u64,
    /// `None` keeps the whole log.
    pub compact_updates: Option<usize>,
}

impl Seed {
    pub fn docs(count: usize) -> Self {
        Self {
            pages: count,
            docs: count,
            users: 1,
            fanout: None,
            index: false,
            seed: 1,
            compact_updates: Some(COMPACT_UPDATES),
        }
    }
}

fn session(user: usize, now: i64) -> NewSession {
    NewSession {
        token_hash: token_hash(&session_token(user)),
        user_id: user_id(user),
        now,
        expires_at: i64::MAX,
    }
}

/// The users of an empty database, with their sessions, a workspace and a space they can edit.
async fn seed_accounts<S: AccountStore + SpaceStore + WorkspaceStore>(
    store: &S,
    users: usize,
) -> anyhow::Result<()> {
    let now = now_ms();
    // One hash for all: they share the password, and each hash takes a while.
    let password_hash = Argon2::default()
        .hash_password(BENCH_PASSWORD.as_bytes())
        .map_err(|error| anyhow::anyhow!("hashing failed: {error}"))?
        .to_string();
    let owner = FirstUser {
        id: user_id(0),
        email: user_email(0),
        name: "Bench".to_owned(),
        password_hash: password_hash.clone(),
        workspace_id: WORKSPACE.to_owned(),
        workspace_slug: WORKSPACE.to_owned(),
        workspace_name: "Bench".to_owned(),
    };
    store.create_first_user(&owner, &session(0, now)).await?;
    let space = NewSpace {
        id: SPACE.to_owned(),
        workspace_id: WORKSPACE.to_owned(),
        slug: SPACE.to_owned(),
        name: "Bench".to_owned(),
        description: String::new(),
        default_role: Some(DefaultRole::Editor),
        actor: user_id(0),
        now,
    };
    store.create_space(&space).await?;

    for user in 1..users {
        let link = token_hash(&format!("bench-invitation-{user}"));
        let invitation = NewInvitation {
            id: format!("bench-invitation-{user}"),
            workspace_id: WORKSPACE.to_owned(),
            email: user_email(user),
            role: WorkspaceRole::Member,
            token_hash: link.clone(),
            invited_by: user_id(0),
            now,
            expires_at: i64::MAX,
        };
        store.create_invitation(&invitation).await?;
        let account = InvitedUser {
            id: user_id(user),
            name: format!("Bench {user}"),
            password_hash: password_hash.clone(),
        };
        store
            .accept_as_new_user(&link, &account, &session(user, now))
            .await?;
    }
    Ok(())
}

/// What the reindex job stores for this page (`cadocs-jobs`).
async fn index_page<S: SearchStore>(store: &S, id: &str, doc: &Doc) -> anyhow::Result<()> {
    let target = store
        .index_target(id)
        .await?
        .ok_or_else(|| anyhow::anyhow!("{id} has no index target"))?;
    let language = Language::parse(&target.language)
        .ok_or_else(|| anyhow::anyhow!("unknown language {}", target.language))?;
    let (title, document) = read_page(doc);
    let plain_text = to_text(&document);
    let analyzer = Analyzer::new(language);
    let indexed = store
        .index_page(&PageIndex {
            page_id: id,
            title: &analyzer.index_text(&title),
            body: &analyzer.index_text(&plain_text),
            plain_text: &plain_text,
            attachments: &[],
        })
        .await?;
    anyhow::ensure!(indexed, "{id} was not indexed");
    Ok(())
}

/// Stores the document of `seed` the way a room would: one row per edit, and a compaction each
/// time the log passes `compact_updates`. Returns the document.
async fn store_document<S: DocStore>(
    store: &S,
    id: &str,
    seed: u64,
    compact_updates: Option<usize>,
) -> anyhow::Result<Doc> {
    let generated = generate(seed);
    let updates = generated.updates;
    let (title, _) = read_page(&generated.doc);
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
        // The title, as a flush copies it to the page.
        let edit = DocEdit {
            title: Some(title.clone()),
            author: None,
        };
        store.append_updates(id, &rows, &edit).await?;

        if compact_updates.is_some() && chunk.len() == batch {
            let snapshot = Snapshot {
                state: encode_state(&replayed),
                upto_seq: seq,
            };
            store.compact(id, &snapshot).await?;
        }
    }
    Ok(replayed)
}

/// Ends the jobs that storing the documents queued: a reindex and a revision for each. The
/// seeded pages count as indexed (or not, without `index`) and have no revision. Left queued,
/// the jobs would run on every server started on the database, during what it measures as idle.
async fn drop_jobs<S: JobStore>(store: &S) -> anyhow::Result<()> {
    while let Some(job) = store.claim_job(i64::MAX).await? {
        store.finish_job(&job.id).await?;
    }
    Ok(())
}

/// Seeds an empty database. It leaves no job queued.
pub async fn seed<S>(store: &S, seed: &Seed) -> anyhow::Result<()>
where
    S: AccountStore + DocStore + JobStore + PageStore + SearchStore + SpaceStore + WorkspaceStore,
{
    anyhow::ensure!(seed.users >= 1, "the seed needs a user");
    anyhow::ensure!(seed.docs <= seed.pages, "more documents than pages");
    seed_accounts(store, seed.users).await?;
    for index in 0..seed.pages {
        let id = doc_id(index);
        let page = NewPage {
            id: id.clone(),
            space_id: SPACE.to_owned(),
            parent_id: parent_of(index, seed.fanout).map(doc_id),
            placement: Placement::Last,
            slug: id.clone(),
            actor: user_id(0),
            now: now_ms(),
        };
        store.create_page(&page).await?;
        if index < seed.docs {
            let doc =
                store_document(store, &id, seed.seed + index as u64, seed.compact_updates).await?;
            if seed.index {
                index_page(store, &id, &doc).await?;
            }
        }
    }
    drop_jobs(store).await
}
