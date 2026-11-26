//! Comments of a page over REST (`docs/API.md#comments`, ADR 0012). The cells of the permission
//! matrix are in `matrix.rs`, the `comment` event in `events.rs`.

mod common;

use common::{Api, Login, Reply, logged_in};
use serde_json::{Value, json};

const BOB: &str = "33333333-0000-7000-8000-000000000001";
const CAROL: &str = "33333333-0000-7000-8000-000000000002";
/// Has an account, but is no member of the workspace.
const DAVE: &str = "33333333-0000-7000-8000-000000000003";

/// Ada owns the workspace and the space, Bob comments on it, Carol only reads it.
struct Wiki {
    api: Api,
    ada: Login,
    bob: Login,
    carol: Login,
    ada_id: String,
    page: String,
}

fn id(reply: &Reply) -> String {
    reply.body["id"].as_str().unwrap().to_owned()
}

async fn wiki() -> Wiki {
    let (api, ada) = logged_in().await;
    let workspace = api.first_workspace().await;
    let bob = api
        .add_member(&workspace, BOB, "bob@example.org", "member")
        .await;
    let carol = api
        .add_member(&workspace, CAROL, "carol@example.org", "member")
        .await;
    let path = format!("/workspaces/{workspace}/spaces");
    let body = json!({ "name": "Handbook", "default_role": null });
    let reply = api.send("POST", &path, &ada.headers(), body).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let space = id(&reply);
    let grants = json!({ "grants": [
        { "user_id": BOB, "role": "commenter" },
        { "user_id": CAROL, "role": "viewer" },
    ] });
    let path = format!("/spaces/{space}/grants");
    let reply = api.send("PUT", &path, &ada.headers(), grants).await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let path = format!("/spaces/{space}/pages");
    let reply = api.send("POST", &path, &ada.headers(), json!({})).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    let page = id(&reply);
    let ada_id = api.session(&ada).await.body["user"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    Wiki {
        api,
        ada,
        bob,
        carol,
        ada_id,
        page,
    }
}

/// Base64 of a few bytes, as a client encodes a Yjs relative position.
const START: &str = "AQIDBA==";
const END: &str = "BQYHCA==";

impl Wiki {
    async fn send(&self, by: &Login, method: &str, path: &str, body: Value) -> Reply {
        self.api.send(method, path, &by.headers(), body).await
    }

    async fn comment(&self, by: &Login, body: Value) -> Reply {
        let path = format!("/pages/{}/comments", self.page);
        self.send(by, "POST", &path, body).await
    }

    async fn thread(&self, by: &Login, text: &str) -> String {
        let reply = self.comment(by, json!({ "body": text })).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn reply_to(&self, by: &Login, parent: &str, text: &str) -> String {
        let body = json!({ "body": text, "parent_id": parent });
        let reply = self.comment(by, body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        id(&reply)
    }

    async fn list(&self, by: &Login, query: &str) -> Reply {
        let path = format!("/pages/{}/comments{query}", self.page);
        self.api.get(by, &path).await
    }

    async fn threads(&self, by: &Login) -> Vec<Value> {
        let reply = self.list(by, "").await;
        assert_eq!(reply.status, 200, "{reply:?}");
        reply.body["items"].as_array().unwrap().clone()
    }

    async fn resolve(&self, by: &Login, comment: &str, resolved: bool) -> Reply {
        let path = format!("/comments/{comment}/resolve");
        let body = json!({ "resolved": resolved });
        self.send(by, "POST", &path, body).await
    }

    async fn edit(&self, by: &Login, comment: &str, text: &str) -> Reply {
        let path = format!("/comments/{comment}");
        self.send(by, "PATCH", &path, json!({ "body": text })).await
    }

    async fn delete(&self, by: &Login, comment: &str) -> Reply {
        let path = format!("/comments/{comment}");
        self.api.raw("DELETE", &path, &by.headers(), b"").await
    }
}

// Creating and listing

#[tokio::test]
async fn a_comment_answers_with_its_author_and_anchor() {
    let wiki = wiki().await;
    let anchor = json!({ "start": START, "end": END, "text": "the quoted words" });
    let reply = wiki
        .comment(
            &wiki.ada,
            json!({ "body": "  Is this **right**?\n", "anchor": anchor }),
        )
        .await;

    assert_eq!(reply.status, 201, "{reply:?}");
    let comment = &reply.body;
    assert_eq!(comment["page_id"], wiki.page.as_str());
    assert_eq!(comment["parent_id"], Value::Null);
    assert_eq!(comment["body"], "Is this **right**?");
    assert_eq!(
        comment["author"],
        json!({ "id": wiki.ada_id, "name": "Ada" })
    );
    assert_eq!(comment["anchor"], anchor);
    assert_eq!(comment["resolved_at"], Value::Null);
    assert_eq!(comment["resolved_by"], Value::Null);
    assert_eq!(comment["created_at"], comment["updated_at"]);
    let stored = wiki
        .api
        .number("SELECT count(*) FROM comments WHERE anchor_start = x'01020304' AND anchor_end = x'05060708'")
        .await;
    assert_eq!(stored, 1);
}

#[tokio::test]
async fn threads_list_oldest_first_with_their_replies() {
    let wiki = wiki().await;
    let first = wiki.thread(&wiki.ada, "First").await;
    let second = wiki.thread(&wiki.bob, "Second").await;
    let answer = wiki.reply_to(&wiki.bob, &first, "An answer").await;
    let another = wiki.reply_to(&wiki.ada, &first, "Another").await;

    let threads = wiki.threads(&wiki.carol).await;

    let ids: Vec<&str> = threads.iter().map(|t| t["id"].as_str().unwrap()).collect();
    assert_eq!(ids, [first.as_str(), second.as_str()]);
    let replies: Vec<&str> = threads[0]["replies"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r["id"].as_str().unwrap())
        .collect();
    assert_eq!(replies, [answer.as_str(), another.as_str()]);
    assert_eq!(threads[0]["replies"][0]["parent_id"], first.as_str());
    assert_eq!(threads[0]["replies"][0]["anchor"], Value::Null);
    assert_eq!(threads[1]["replies"], json!([]));
}

#[tokio::test]
async fn threads_come_in_pages() {
    let wiki = wiki().await;
    let mut made = Vec::new();
    for n in 0..3 {
        made.push(wiki.thread(&wiki.ada, &format!("Thread {n}")).await);
    }
    wiki.reply_to(&wiki.ada, &made[1], "Reply").await;

    let reply = wiki.list(&wiki.ada, "?limit=2").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let cursor = reply.body["next_cursor"].as_str().unwrap().to_owned();
    let reply2 = wiki
        .list(&wiki.ada, &format!("?limit=2&cursor={cursor}"))
        .await;

    let ids = |reply: &Reply| -> Vec<String> {
        let items = reply.body["items"].as_array().unwrap();
        items.iter().map(id_of).collect()
    };
    assert_eq!(ids(&reply), made[..2]);
    assert_eq!(ids(&reply2), made[2..]);
    assert_eq!(reply2.body["next_cursor"], Value::Null);
    assert_eq!(
        reply.body["items"][1]["replies"].as_array().unwrap().len(),
        1
    );
}

fn id_of(value: &Value) -> String {
    value["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn a_comment_on_the_whole_page_has_no_anchor() {
    let wiki = wiki().await;
    let reply = wiki
        .comment(&wiki.bob, json!({ "body": "Whole page" }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(reply.body["anchor"], Value::Null);
}

// Refusals

#[tokio::test]
async fn a_body_must_hold_something_and_not_too_much() {
    let wiki = wiki().await;
    for (body, problem) in [
        (" \n ".to_owned(), "required"),
        ("x".repeat(10_001), "too_long"),
    ] {
        let reply = wiki.comment(&wiki.ada, json!({ "body": body })).await;
        assert_eq!(reply.status, 400, "{reply:?}");
        assert_eq!(reply.body["error"]["fields"]["body"], problem);
    }
    let reply = wiki
        .comment(&wiki.ada, json!({ "body": "x".repeat(10_000) }))
        .await;
    assert_eq!(reply.status, 201, "{reply:?}");
}

#[tokio::test]
async fn an_anchor_must_be_whole_and_small() {
    let wiki = wiki().await;
    let long = "A".repeat(1400);
    for anchor in [
        json!({ "start": START, "end": END }),
        json!({ "start": START, "text": "quote" }),
        json!({ "start": "not base64!", "end": END, "text": "quote" }),
        json!({ "start": "", "end": END, "text": "quote" }),
        json!({ "start": long, "end": END, "text": "quote" }),
        json!({ "start": START, "end": END, "text": "q".repeat(1001) }),
    ] {
        let reply = wiki
            .comment(&wiki.ada, json!({ "body": "Hm", "anchor": anchor }))
            .await;
        assert_eq!(reply.status, 400, "{anchor} gave {reply:?}");
    }
}

#[tokio::test]
async fn a_reply_answers_a_thread_of_the_same_page_and_has_no_anchor() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;
    let reply = wiki.reply_to(&wiki.ada, &thread, "Reply").await;
    let path = format!("/spaces/{}/pages", space_of(&wiki).await);
    let other = id(&wiki.send(&wiki.ada, "POST", &path, json!({})).await);
    let elsewhere = {
        let path = format!("/pages/{other}/comments");
        let created = wiki
            .send(&wiki.ada, "POST", &path, json!({ "body": "There" }))
            .await;
        id(&created)
    };

    let cases = [
        (
            json!({ "body": "x", "parent_id": reply }),
            "parent_id",
            "nested",
        ),
        (
            json!({ "body": "x", "parent_id": elsewhere }),
            "parent_id",
            "unknown",
        ),
        (
            json!({ "body": "x", "parent_id": "nope" }),
            "parent_id",
            "unknown",
        ),
        (
            json!({ "body": "x", "parent_id": thread,
                    "anchor": { "start": START, "end": END, "text": "q" } }),
            "anchor",
            "reply",
        ),
    ];
    for (body, field, problem) in cases {
        let got = wiki.comment(&wiki.ada, body.clone()).await;
        assert_eq!(got.status, 400, "{body} gave {got:?}");
        assert_eq!(got.body["error"]["fields"][field], problem, "{body}");
    }
}

async fn space_of(wiki: &Wiki) -> String {
    let reply = wiki
        .api
        .get(&wiki.ada, &format!("/pages/{}", wiki.page))
        .await;
    reply.body["space_id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn unknown_fields_are_refused() {
    let wiki = wiki().await;
    let reply = wiki
        .comment(&wiki.ada, json!({ "body": "x", "author_id": BOB }))
        .await;
    assert_eq!(reply.status, 400, "{reply:?}");
}

#[tokio::test]
async fn a_trashed_page_hides_its_comments() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;
    let reply = wiki
        .api
        .raw(
            "DELETE",
            &format!("/pages/{}", wiki.page),
            &wiki.ada.headers(),
            b"",
        )
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");

    assert_eq!(wiki.list(&wiki.ada, "").await.status, 404);
    assert_eq!(
        wiki.comment(&wiki.ada, json!({ "body": "x" })).await.status,
        404
    );
    assert_eq!(wiki.edit(&wiki.ada, &thread, "y").await.status, 404);
    assert_eq!(wiki.resolve(&wiki.ada, &thread, true).await.status, 404);
    assert_eq!(wiki.delete(&wiki.ada, &thread).await.status, 404);
}

#[tokio::test]
async fn an_unknown_comment_is_not_found() {
    let wiki = wiki().await;
    let missing = "00000000-0000-7000-8000-000000000000";
    assert_eq!(wiki.edit(&wiki.ada, missing, "y").await.status, 404);
    assert_eq!(wiki.resolve(&wiki.ada, missing, true).await.status, 404);
    assert_eq!(wiki.delete(&wiki.ada, missing).await.status, 404);
}

// Editing and deleting

#[tokio::test]
async fn the_author_edits_their_comment() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.bob, "Frist").await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;

    let reply = wiki.edit(&wiki.bob, &thread, " First ").await;

    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["body"], "First");
    let (created, updated) = (&reply.body["created_at"], &reply.body["updated_at"]);
    assert!(updated.as_i64() > created.as_i64(), "{reply:?}");
    let reply = wiki.edit(&wiki.bob, &thread, "").await;
    assert_eq!(reply.body["error"]["fields"]["body"], "required");
}

#[tokio::test]
async fn only_the_author_or_a_space_admin_edits_or_deletes() {
    let wiki = wiki().await;
    let by_ada = wiki.thread(&wiki.ada, "Ada's").await;
    let by_bob = wiki.thread(&wiki.bob, "Bob's").await;

    assert_eq!(wiki.edit(&wiki.bob, &by_ada, "Mine now").await.status, 403);
    assert_eq!(wiki.delete(&wiki.bob, &by_ada).await.status, 403);
    assert_eq!(
        wiki.edit(&wiki.carol, &by_bob, "Mine now").await.status,
        403
    );
    assert_eq!(wiki.delete(&wiki.carol, &by_bob).await.status, 403);

    assert_eq!(wiki.edit(&wiki.ada, &by_bob, "Moderated").await.status, 200);
    assert_eq!(wiki.delete(&wiki.ada, &by_bob).await.status, 204);
}

#[tokio::test]
async fn an_author_who_lost_the_commenter_role_no_longer_edits() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.bob, "Bob's").await;
    wiki.api
        .sql(&format!(
            "UPDATE space_grants SET role = 'viewer' WHERE user_id = '{BOB}'"
        ))
        .await;

    assert_eq!(
        wiki.edit(&wiki.bob, &thread, "Still mine").await.status,
        403
    );
    assert_eq!(wiki.delete(&wiki.bob, &thread).await.status, 403);
}

#[tokio::test]
async fn deleting_a_thread_takes_its_replies() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;
    let reply = wiki.reply_to(&wiki.bob, &thread, "Reply").await;
    let kept = wiki.thread(&wiki.bob, "Kept").await;

    assert_eq!(wiki.delete(&wiki.bob, &reply).await.status, 204);
    assert_eq!(wiki.threads(&wiki.ada).await[0]["replies"], json!([]));
    wiki.reply_to(&wiki.bob, &thread, "Again").await;
    assert_eq!(wiki.delete(&wiki.ada, &thread).await.status, 204);

    let threads = wiki.threads(&wiki.ada).await;
    assert_eq!(threads.len(), 1);
    assert_eq!(threads[0]["id"], kept.as_str());
    let left = wiki.api.number("SELECT count(*) FROM comments").await;
    assert_eq!(left, 1);
}

#[tokio::test]
async fn a_deleted_account_leaves_its_comments_without_an_author() {
    let wiki = wiki().await;
    wiki.thread(&wiki.bob, "Bob's").await;
    wiki.api
        .sql(&format!("DELETE FROM users WHERE id = '{BOB}'"))
        .await;

    let threads = wiki.threads(&wiki.ada).await;
    assert_eq!(threads[0]["author"], Value::Null);
    assert_eq!(threads[0]["body"], "Bob's");
}

// Resolving

#[tokio::test]
async fn a_commenter_resolves_and_reopens_a_thread() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;

    let reply = wiki.resolve(&wiki.bob, &thread, true).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert!(reply.body["resolved_at"].is_i64(), "{reply:?}");
    assert_eq!(
        reply.body["resolved_by"],
        json!({ "id": BOB, "name": "Other" })
    );
    assert_eq!(reply.body["updated_at"], reply.body["created_at"]);
    let first = reply.body["resolved_at"].clone();
    let again = wiki.resolve(&wiki.ada, &thread, true).await;
    assert_eq!(
        again.body["resolved_at"], first,
        "resolving twice keeps the first"
    );
    assert_eq!(again.body["resolved_by"]["id"], BOB);

    let reply = wiki.resolve(&wiki.bob, &thread, false).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["resolved_at"], Value::Null);
    assert_eq!(reply.body["resolved_by"], Value::Null);

    assert_eq!(wiki.resolve(&wiki.carol, &thread, true).await.status, 403);
}

#[tokio::test]
async fn a_reply_cannot_be_resolved() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;
    let reply = wiki.reply_to(&wiki.ada, &thread, "Reply").await;

    let got = wiki.resolve(&wiki.ada, &reply, true).await;
    assert_eq!(got.status, 409, "{got:?}");
}

#[tokio::test]
async fn a_resolve_body_is_required() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;
    let path = format!("/comments/{thread}/resolve");
    let reply = wiki.send(&wiki.ada, "POST", &path, json!({})).await;
    assert_eq!(reply.status, 400, "{reply:?}");
}

// Hidden from strangers

#[tokio::test]
async fn a_stranger_to_the_space_learns_nothing() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;
    let workspace = wiki.api.first_workspace().await;
    let dave = wiki
        .api
        .add_member(
            &workspace,
            "33333333-0000-7000-8000-000000000003",
            "dave@example.org",
            "member",
        )
        .await;

    assert_eq!(wiki.list(&dave, "").await.status, 404);
    assert_eq!(
        wiki.comment(&dave, json!({ "body": "x" })).await.status,
        404
    );
    assert_eq!(wiki.edit(&dave, &thread, "y").await.status, 404);
    assert_eq!(wiki.resolve(&dave, &thread, true).await.status, 404);
    assert_eq!(wiki.delete(&dave, &thread).await.status, 404);
}

/// The users comment `comment` mentions, each checked to be by Bob on the wiki's page.
async fn comment_mentions(w: &Wiki, comment: &str) -> Vec<&'static str> {
    let all = w
        .api
        .number(&format!(
            "SELECT count(*) FROM mentions WHERE comment_id = '{comment}'"
        ))
        .await;
    let mut found = Vec::new();
    for user in [BOB, CAROL, DAVE] {
        let rows = w
            .api
            .number(&format!(
                "SELECT count(*) FROM mentions WHERE comment_id = '{comment}'
                 AND user_id = '{user}' AND author_id = '{BOB}' AND page_id = '{}'",
                w.page
            ))
            .await;
        if rows == 1 {
            found.push(user);
        }
    }
    assert_eq!(
        all as usize,
        found.len(),
        "every mention is by Bob, on the page"
    );
    found
}

// Mentions

#[tokio::test]
async fn a_comment_records_the_members_it_mentions() {
    let w = wiki().await;
    w.api.add_user(DAVE, "dave@example.org").await;
    let body = format!(
        "<user:{CAROL}>, <user:{CAROL}>, <user:{DAVE}> and <page:{}>",
        w.page
    );
    let reply = w.comment(&w.bob, json!({ "body": body })).await;
    assert_eq!(reply.status, 201, "{reply:?}");
    assert_eq!(
        reply.body["body"],
        body.as_str(),
        "the body is kept as given"
    );
    let comment = id(&reply);
    // Dave is no member, and a page mention is no one's.
    assert_eq!(comment_mentions(&w, &comment).await, [CAROL]);

    // An edit adds and removes, and keeps what stays as it was.
    w.api
        .sql(&format!(
            "UPDATE mentions SET read_at = 7 WHERE comment_id = '{comment}'"
        ))
        .await;
    let reply = w
        .edit(&w.bob, &comment, &format!("<user:{BOB}> <user:{CAROL}>"))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(comment_mentions(&w, &comment).await, [BOB, CAROL]);
    let read = w
        .api
        .number(&format!(
            "SELECT count(*) FROM mentions WHERE user_id = '{CAROL}' AND read_at = 7"
        ))
        .await;
    assert_eq!(read, 1, "Carol's mention stays read");
    let reply = w.edit(&w.bob, &comment, "nobody").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert!(comment_mentions(&w, &comment).await.is_empty());

    // A reply mentions too, and deleting the thread takes its mentions.
    w.reply_to(&w.bob, &comment, &format!("<user:{CAROL}>"))
        .await;
    assert_eq!(w.api.number("SELECT count(*) FROM mentions").await, 1);
    assert_eq!(w.delete(&w.bob, &comment).await.status, 204);
    assert_eq!(w.api.number("SELECT count(*) FROM mentions").await, 0);
}

// Limits

/// A thread takes `MAX_REPLIES` replies, so a list of threads stays bounded in memory.
#[tokio::test]
async fn a_thread_takes_a_bounded_number_of_replies() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.ada, "Thread").await;
    let page = &wiki.page;
    wiki.api
        .sql(&format!(
            "WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < {})
             INSERT INTO comments (id, page_id, parent_id, author_id, body, created_at, updated_at)
             SELECT 'reply-' || i, '{page}', '{thread}', '{}', 'Me too', 1, 1 FROM n",
            cadocs_store::MAX_REPLIES - 1,
            wiki.ada_id
        ))
        .await;
    wiki.reply_to(&wiki.bob, &thread, "The last one").await;

    let body = json!({ "body": "One more", "parent_id": thread });
    let got = wiki.comment(&wiki.bob, body).await;
    assert_eq!(got.status, 409, "{got:?}");
    // A new thread is still welcome.
    wiki.thread(&wiki.bob, "Another").await;
}

/// Each account writes comments at a bounded pace, edits included.
#[tokio::test]
async fn comments_are_written_at_a_bounded_pace() {
    let wiki = wiki().await;
    let thread = wiki.thread(&wiki.bob, "Thread").await;
    let mut got = wiki.edit(&wiki.bob, &thread, "Edited").await;
    for _ in 0..cadocs_server::api::comments::WRITE_BURST {
        if got.status == 429 {
            break;
        }
        got = wiki.edit(&wiki.bob, &thread, "Edited").await;
    }
    assert_eq!(got.status, 429, "{got:?}");
    assert!(got.header("retry-after").is_some(), "{got:?}");
    let got = wiki.comment(&wiki.bob, json!({ "body": "More" })).await;
    assert_eq!(got.status, 429, "{got:?}");
    // Another account keeps its own pace.
    wiki.thread(&wiki.ada, "Mine").await;
}
