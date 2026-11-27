//! The language of mail (step 62): the person's setting, else the last browser language seen,
//! else their workspace's, else English.

mod common;

use common::mail::{body, messages, with_mail};
use common::{Api, Login, PASSWORD};
use serde_json::json;

const FRENCH_BROWSER: &str = "fr-FR,fr;q=0.9,en-US;q=0.8,en;q=0.7";
const FRENCH_RESET: &str = "Pour choisir un nouveau mot de passe";
const ENGLISH_RESET: &str = "To choose a new password";

async fn ada(api: &Api) -> Login {
    let reply = api.login("ada@example.org", PASSWORD).await;
    assert_eq!(reply.status, 200, "{reply:?}");
    Login::from(&reply)
}

/// Ada's session, read by a browser in `languages` when it is due for a touch.
async fn seen_by(api: &Api, ada: &Login, languages: &str) {
    api.sql("UPDATE sessions SET last_seen_at = 0").await;
    read_by(api, ada, languages).await;
}

async fn read_by(api: &Api, ada: &Login, languages: &str) {
    let headers = [
        ("cookie", ada.cookie.as_str()),
        ("accept-language", languages),
    ];
    let reply = api.raw("GET", "/session", &headers, b"").await;
    assert_eq!(reply.status, 200, "{reply:?}");
}

/// The reset message Ada gets, as the `count`th message of the relay.
async fn reset_text(api: &Api, relay: &cadocs_mail::fake::FakeRelay, count: usize) -> String {
    let body_json = json!({ "email": "ada@example.org" });
    let reply = api
        .send("POST", "/auth/forgot-password", &[], body_json)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    body(&messages(relay, count).await[count - 1])
}

#[tokio::test]
async fn a_person_with_no_setting_and_a_french_browser_gets_french_mail() {
    let (api, relay) = with_mail().await;
    let ada = ada(&api).await;
    seen_by(&api, &ada, FRENCH_BROWSER).await;
    let text = reset_text(&api, &relay, 1).await;
    assert!(text.contains(FRENCH_RESET), "{text}");
    assert!(!text.contains(ENGLISH_RESET), "{text}");

    // A browser with no language mail has keeps the last one seen.
    seen_by(&api, &ada, "de-DE").await;
    let text = reset_text(&api, &relay, 2).await;
    assert!(text.contains(FRENCH_RESET), "{text}");

    seen_by(&api, &ada, "en-GB").await;
    let text = reset_text(&api, &relay, 3).await;
    assert!(text.contains(ENGLISH_RESET), "{text}");
}

#[tokio::test]
async fn the_browser_language_is_noted_only_at_a_touch() {
    let (api, relay) = with_mail().await;
    let ada = ada(&api).await;
    // A fresh session is not due: two browsers in two languages write at most once a touch.
    read_by(&api, &ada, FRENCH_BROWSER).await;
    let noted = "SELECT count(*) FROM users WHERE browser_language IS NOT NULL";
    assert_eq!(api.number(noted).await, 0);
    let text = reset_text(&api, &relay, 1).await;
    assert!(text.contains(ENGLISH_RESET), "{text}");
}

#[tokio::test]
async fn a_language_that_cannot_be_read_still_mails_a_reset_link() {
    let (api, relay) = with_mail().await;
    // The language of mail reads this table: an English link beats none, as the old one is void.
    api.sql("DROP TABLE workspace_members").await;
    let text = reset_text(&api, &relay, 1).await;
    assert!(text.contains(ENGLISH_RESET), "{text}");
}

#[tokio::test]
async fn the_setting_wins_over_the_browser() {
    let (api, relay) = with_mail().await;
    let ada = ada(&api).await;
    let reply = api
        .send("PATCH", "/me", &ada.headers(), json!({ "language": "en" }))
        .await;
    assert_eq!(reply.status, 200, "{reply:?}");
    seen_by(&api, &ada, FRENCH_BROWSER).await;
    let text = reset_text(&api, &relay, 1).await;
    assert!(text.contains(ENGLISH_RESET), "{text}");
}

#[tokio::test]
async fn with_no_setting_nor_browser_the_workspace_language_is_used() {
    let (api, relay) = with_mail().await;
    let text = reset_text(&api, &relay, 1).await;
    assert!(text.contains(ENGLISH_RESET), "{text}");

    api.sql("UPDATE workspaces SET language = 'fr'").await;
    let text = reset_text(&api, &relay, 2).await;
    assert!(text.contains(FRENCH_RESET), "{text}");

    // A workspace language mail is not written in leaves English.
    api.sql("UPDATE workspaces SET language = 'de'").await;
    let text = reset_text(&api, &relay, 3).await;
    assert!(text.contains(ENGLISH_RESET), "{text}");
}

#[tokio::test]
async fn a_registration_code_follows_the_browser_that_asked() {
    let (api, relay) = with_mail().await;
    let headers = [("accept-language", FRENCH_BROWSER)];
    let body_json = json!({ "email": "grace@example.org" });
    let reply = api
        .send("POST", "/auth/register", &headers, body_json)
        .await;
    assert_eq!(reply.status, 204, "{reply:?}");
    let text = body(&messages(&relay, 1).await[0]);
    assert!(text.contains("Il est valable 10 minutes"), "{text}");
}
