//! Step 73: the people an admin can invite, the mail of an invitation, and the invitations an
//! account accepts or declines by id, without the link.

mod common;

use common::mail::{body, messages, settle, with_mail};
use common::{Api, Login, ORIGIN, Reply, logged_in};
use serde_json::{Value, json};

const BOB: &str = "33333333-0000-7000-8000-000000000001";
const CARA: &str = "33333333-0000-7000-8000-000000000002";
const DAN: &str = "33333333-0000-7000-8000-000000000003";

impl Api {
    async fn people(&self, by: &Login, workspace: &str, q: &str) -> Reply {
        self.get(by, &format!("/workspaces/{workspace}/invitees?q={q}"))
            .await
    }

    async fn invite(&self, by: &Login, workspace: &str, email: &str) -> Reply {
        let path = format!("/workspaces/{workspace}/invitations");
        let body = json!({ "email": email, "role": "admin" });
        let reply = self.send("POST", &path, &by.headers(), body).await;
        assert_eq!(reply.status, 201, "{reply:?}");
        reply
    }

    async fn named(&self, id: &str, email: &str, name: &str) -> Login {
        let login = self.add_and_login(id, email).await;
        self.sql(&format!(
            "UPDATE users SET name = '{name}' WHERE id = '{id}'"
        ))
        .await;
        login
    }

    async fn pending(&self, by: &Login) -> Vec<Value> {
        let reply = self.get(by, "/me/invitations").await;
        assert_eq!(reply.status, 200, "{reply:?}");
        reply.body["items"].as_array().unwrap().clone()
    }

    async fn role_of(&self, workspace: &str, user: &str) -> i64 {
        let query = format!(
            "SELECT count(*) FROM workspace_members
             WHERE workspace_id = '{workspace}' AND user_id = '{user}' AND role = 'admin'"
        );
        self.number(&query).await
    }
}

fn names(reply: &Reply) -> Vec<&str> {
    let items = reply.body["items"].as_array().expect("items");
    items.iter().map(|p| p["name"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn an_admin_finds_accounts_to_invite_by_name_or_address() {
    let (api, ada) = logged_in().await;
    let ws = api.first_workspace().await;
    api.named(BOB, "bob@example.org", "Bob Martin").await;
    api.named(CARA, "cara@example.org", "Cara Marty").await;
    let dan = api.named(DAN, "mar@example.org", "Dan").await;

    // A word of the name or the start of the address, whatever the case.
    let reply = api.people(&ada, &ws, "MAR").await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(names(&reply), ["Bob Martin", "Cara Marty", "Dan"]);
    assert_eq!(
        reply.body["items"][0],
        json!({ "id": BOB, "name": "Bob Martin", "email": "bob@example.org" })
    );
    let reply = api.people(&ada, &ws, "cara%40").await;
    assert_eq!(names(&reply), ["Cara Marty"]);
    // Not the middle of a word, and `%` or `_` match only themselves.
    assert!(names(&api.people(&ada, &ws, "art").await).is_empty());
    assert!(names(&api.people(&ada, &ws, "%25%25%25").await).is_empty());
    assert!(names(&api.people(&ada, &ws, "___").await).is_empty());

    // Members and disabled accounts are left out: Ada herself, Dan once he joins, Cara disabled.
    assert!(names(&api.people(&ada, &ws, "ada").await).is_empty());
    api.sql(&format!(
        "INSERT INTO workspace_members (workspace_id, user_id, role, created_at)
         VALUES ('{ws}', '{DAN}', 'member', 1)"
    ))
    .await;
    api.sql(&format!(
        "UPDATE users SET disabled_at = 1 WHERE id = '{CARA}'"
    ))
    .await;
    assert_eq!(names(&api.people(&ada, &ws, "mar").await), ["Bob Martin"]);

    // Fewer than 3 characters, spaces aside, is refused.
    let reply = api.people(&ada, &ws, "bo+").await;
    assert_eq!((reply.status, reply.code()), (400, "validation_failed"));
    assert_eq!(reply.body["error"]["fields"]["q"], "too_short");

    // A member may not look, a stranger finds no workspace.
    let reply = api.people(&dan, &ws, "bob").await;
    assert_eq!((reply.status, reply.code()), (403, "forbidden"));
    let bob = api.login("bob@example.org", common::PASSWORD).await;
    let bob = Login::from(&bob);
    let reply = api.people(&bob, &ws, "cara").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn the_search_gives_ten_people_at_most_and_is_rate_limited() {
    let (api, ada) = logged_in().await;
    let ws = api.first_workspace().await;
    for n in 0..12 {
        api.sql(&format!(
            "INSERT INTO users (id, email, name, password_hash, created_at, updated_at)
             VALUES ('user-{n:02}', 'zed{n:02}@example.org', 'Zed {n:02}', NULL, 1, 1)"
        ))
        .await;
    }
    let reply = api.people(&ada, &ws, "zed").await;
    assert_eq!(names(&reply).len(), 10);
    assert_eq!(names(&reply)[0], "Zed 00");

    let mut last = reply;
    for _ in 0..30 {
        last = api.people(&ada, &ws, "zed").await;
        if last.status != 200 {
            break;
        }
    }
    assert_eq!((last.status, last.code()), (429, "rate_limited"));
    assert!(last.header("retry-after").is_some());
}

#[tokio::test]
async fn an_invited_account_lists_its_invitations_and_accepts_one_by_id() {
    let (api, ada) = logged_in().await;
    let ws = api.first_workspace().await;
    let bob = api.named(BOB, "bob@example.org", "Bob").await;
    let cara = api.named(CARA, "cara@example.org", "Cara").await;
    let invitation = api.invite(&ada, &ws, "bob@example.org").await;
    let id = invitation.body["id"].as_str().unwrap();

    let pending = api.pending(&bob).await;
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0]["id"], id);
    assert_eq!(pending[0]["workspace_id"], ws.as_str());
    assert_eq!(pending[0]["workspace_name"], "Acme Docs");
    assert_eq!(pending[0]["role"], "admin");
    assert_eq!(pending[0]["invited_by_name"], "Ada");
    assert!(api.pending(&cara).await.is_empty());

    // Another account, a missing CSRF token, or no session, cannot use it.
    let path = format!("/me/invitations/{id}/accept");
    let reply = api.send("POST", &path, &cara.headers(), json!({})).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let cookie = [("cookie", bob.cookie.as_str())];
    let reply = api.send("POST", &path, &cookie, json!({})).await;
    assert_eq!((reply.status, reply.code()), (403, "csrf_failed"));
    let reply = api.send("POST", &path, &[], json!({})).await;
    assert_eq!(reply.status, 401, "{reply:?}");
    assert_eq!(api.role_of(&ws, BOB).await, 0);

    let reply = api.send("POST", &path, &bob.headers(), json!({})).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    assert_eq!(reply.body["user"]["id"], BOB);
    assert_eq!(reply.body["workspaces"][0]["id"], ws.as_str());
    assert_eq!(api.role_of(&ws, BOB).await, 1);
    assert!(api.pending(&bob).await.is_empty());
    let reply = api.send("POST", &path, &bob.headers(), json!({})).await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
}

#[tokio::test]
async fn an_invited_account_declines_and_the_link_goes_with_it() {
    let (api, ada) = logged_in().await;
    let ws = api.first_workspace().await;
    let bob = api.named(BOB, "bob@example.org", "Bob").await;
    let cara = api.named(CARA, "cara@example.org", "Cara").await;
    let invitation = api.invite(&ada, &ws, "bob@example.org").await;
    let id = invitation.body["id"].as_str().unwrap();
    let link = invitation.body["link"].as_str().unwrap();
    let token = link.rsplit('/').next().unwrap();

    let path = format!("/me/invitations/{id}");
    let reply = api.raw("DELETE", &path, &cara.headers(), b"").await;
    assert_eq!((reply.status, reply.code()), (404, "not_found"));
    let reply = api.raw("DELETE", &path, &bob.headers(), b"").await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let reply = api.raw("DELETE", &path, &bob.headers(), b"").await;
    assert_eq!(reply.status, 404, "{reply:?}");
    assert!(api.pending(&bob).await.is_empty());
    let reply = api
        .raw("GET", &format!("/invitations/{token}"), &[], b"")
        .await;
    assert_eq!(reply.status, 404, "{reply:?}");
    assert_eq!(api.role_of(&ws, BOB).await, 0);
}

#[tokio::test]
async fn only_the_account_that_held_the_address_when_invited_sees_the_invitation() {
    let (api, ada) = logged_in().await;
    let ws = api.first_workspace().await;
    let invitation = api.invite(&ada, &ws, "eve@example.org").await;
    let id = invitation.body["id"].as_str().unwrap();
    // Bob takes the address afterwards, as an address change without a relay allows.
    let bob = api.named(BOB, "eve@example.org", "Bob").await;
    assert!(api.pending(&bob).await.is_empty());
    let path = format!("/me/invitations/{id}/accept");
    let reply = api.send("POST", &path, &bob.headers(), json!({})).await;
    assert_eq!(reply.status, 404, "{reply:?}");

    // An account that moved away from the address loses it too, and an expired one shows nowhere.
    let cara = api.named(CARA, "cara@example.org", "Cara").await;
    api.invite(&ada, &ws, "cara@example.org").await;
    api.sql(&format!(
        "UPDATE users SET email = 'c@example.org' WHERE id = '{CARA}'"
    ))
    .await;
    assert!(api.pending(&cara).await.is_empty());
    api.sql(&format!(
        "UPDATE users SET email = 'cara@example.org' WHERE id = '{CARA}'"
    ))
    .await;
    assert_eq!(api.pending(&cara).await.len(), 1);
    api.sql("UPDATE workspace_invitations SET expires_at = 1")
        .await;
    assert!(api.pending(&cara).await.is_empty());
}

#[tokio::test]
async fn with_a_relay_an_invitation_mails_its_link_in_the_workspace_language() {
    let (api, relay) = with_mail().await;
    let reply = api.login("ada@example.org", common::PASSWORD).await;
    let ada = Login::from(&reply);
    let ws = api.first_workspace().await;

    let invitation = api.invite(&ada, &ws, "eve@example.org").await;
    let link = invitation.body["link"].as_str().unwrap();
    let message = &messages(&relay, 1).await[0];
    assert_eq!(message.to, "<eve@example.org>");
    let text = body(message);
    assert!(text.contains(link), "{text}");
    assert!(
        text.contains("Ada invited you to the workspace Acme Docs"),
        "{text}"
    );
    assert!(text.contains(ORIGIN), "{text}");

    let path = format!("/workspaces/{ws}");
    let reply = api
        .send("PATCH", &path, &ada.headers(), json!({ "language": "fr" }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    let invitation = api.invite(&ada, &ws, "eve@example.org").await;
    let link = invitation.body["link"].as_str().unwrap();
    let text = body(&messages(&relay, 2).await[1]);
    assert!(text.contains(link), "{text}");
    assert!(
        text.contains("Ada vous invite dans l'espace de travail Acme Docs"),
        "{text}"
    );

    // An address gets 3 messages at once, then the link still comes back, with no mail.
    api.invite(&ada, &ws, "eve@example.org").await;
    api.invite(&ada, &ws, "eve@example.org").await;
    settle().await;
    assert_eq!(relay.messages().len(), 3);
}
