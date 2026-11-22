//! OpenID Connect login against a fake provider in the same process: the code flow with PKCE,
//! the binding of a callback to the browser that started it, and how an identity meets accounts.

mod common;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use axum::Router;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use cadocs_store::now_ms;
use common::{Api, Login, ORIGIN, Reply, start_tuned};
use ring::rand::SystemRandom;
use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair, RsaPublicKeyComponents};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::net::TcpListener;
use url::Url;

const CLIENT_ID: &str = "cadocs";
const SECRET: &str = "provider secret";
const CALLBACK: &str = "https://wiki.example.org/api/v1/auth/oidc/fake/callback";

/// What the provider handed out a code for, as it checks it when the code comes back.
struct Grant {
    challenge: String,
    redirect_uri: String,
    claims: Value,
}

#[derive(Clone)]
struct Fake {
    issuer: String,
    key: Arc<RsaKeyPair>,
    grants: Arc<Mutex<HashMap<String, Grant>>>,
    /// Requests that reached the token endpoint.
    tokens: Arc<AtomicUsize>,
}

async fn fake_provider() -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let issuer = format!("http://{}", listener.local_addr().unwrap());
    let key = RsaKeyPair::from_pkcs8(include_bytes!("oidc-key.der")).unwrap();
    let fake = Fake {
        issuer,
        key: Arc::new(key),
        grants: Arc::default(),
        tokens: Arc::default(),
    };
    let app = Router::new()
        .route("/.well-known/openid-configuration", get(discovery))
        .route("/jwks", get(jwks))
        .route("/token", post(token))
        .with_state(fake.clone());
    tokio::spawn(async move { axum::serve(listener, app).await });
    fake
}

async fn discovery(State(fake): State<Fake>) -> Response {
    let issuer = &fake.issuer;
    axum::Json(json!({
        "issuer": issuer,
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "jwks_uri": format!("{issuer}/jwks"),
    }))
    .into_response()
}

async fn jwks(State(fake): State<Fake>) -> Response {
    let public = RsaPublicKeyComponents::<Vec<u8>>::from(fake.key.public());
    axum::Json(json!({ "keys": [
        // Another key first: the token's `kid` picks the right one.
        { "kty": "RSA", "kid": "old", "use": "sig", "n": "AQAB", "e": "AQAB" },
        {
            "kty": "RSA", "kid": "k1", "use": "sig", "alg": "RS256",
            "n": URL_SAFE_NO_PAD.encode(&public.n),
            "e": URL_SAFE_NO_PAD.encode(&public.e),
        },
    ] }))
    .into_response()
}

/// RFC 6749 section 4.1.3, with the client's secret in Basic and the PKCE verifier of RFC 7636.
async fn token(State(fake): State<Fake>, headers: HeaderMap, body: String) -> Response {
    fake.tokens.fetch_add(1, Ordering::SeqCst);
    let form: HashMap<String, String> = url::form_urlencoded::parse(body.as_bytes())
        .into_owned()
        .collect();
    let basic = format!(
        "Basic {}",
        STANDARD.encode(format!("{CLIENT_ID}:provider+secret"))
    );
    let refused = (StatusCode::BAD_REQUEST, "invalid_grant").into_response();
    if headers
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        != Some(&basic)
        || form.get("grant_type").map(String::as_str) != Some("authorization_code")
    {
        return refused;
    }
    let Some(grant) = form
        .get("code")
        .and_then(|code| fake.grants.lock().unwrap().remove(code))
    else {
        return refused;
    };
    let verifier = form.get("code_verifier").cloned().unwrap_or_default();
    if URL_SAFE_NO_PAD.encode(Sha256::digest(verifier)) != grant.challenge
        || form.get("redirect_uri") != Some(&grant.redirect_uri)
    {
        return refused;
    }
    let id_token = sign(&fake.key, &grant.claims);
    axum::Json(json!({ "access_token": "unused", "token_type": "Bearer", "id_token": id_token }))
        .into_response()
}

fn sign(key: &RsaKeyPair, claims: &Value) -> String {
    let header = json!({ "alg": "RS256", "kid": "k1", "typ": "JWT" });
    let header = URL_SAFE_NO_PAD.encode(header.to_string());
    let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
    let signed = format!("{header}.{payload}");
    let mut signature = vec![0; key.public().modulus_len()];
    let random = SystemRandom::new();
    key.sign(
        &RSA_PKCS1_SHA256,
        &random,
        signed.as_bytes(),
        &mut signature,
    )
    .unwrap();
    format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
}

async fn server(fake: &Fake, settings: &[(&str, &str)]) -> Api {
    let mut all = vec![
        ("CADOCS_OIDC_PROVIDERS", "fake"),
        ("CADOCS_OIDC_FAKE_ISSUER", fake.issuer.as_str()),
        ("CADOCS_OIDC_FAKE_CLIENT_ID", CLIENT_ID),
        ("CADOCS_OIDC_FAKE_CLIENT_SECRET", SECRET),
    ];
    all.extend_from_slice(settings);
    start_tuned(ORIGIN, "", &all).await
}

/// What the start of a login left in the browser: its cookie, and where it sent it.
struct Started {
    cookie: String,
    query: HashMap<String, String>,
}

impl Started {
    fn state(&self) -> &str {
        &self.query["state"]
    }
}

/// The value of the cookie `name` that a reply sets, as `name=value`.
fn set_cookie(reply: &Reply, name: &str) -> Option<String> {
    reply
        .headers
        .iter()
        .filter(|(header, _)| header == "set-cookie")
        .filter_map(|(_, value)| value.split(';').next())
        .find(|pair| pair.starts_with(&format!("{name}=")))
        .map(str::to_owned)
}

impl Api {
    async fn start_oidc(&self, fake: &Fake) -> Started {
        let reply = self.raw("GET", "/auth/oidc/fake", &[], b"").await;
        assert_eq!(reply.status, 303, "{reply:?}");
        let location = Url::parse(reply.header("location").unwrap()).unwrap();
        assert_eq!(
            location[..url::Position::AfterPath],
            format!("{}/authorize", fake.issuer)
        );
        let cookie = set_cookie(&reply, "__Host-oidc").expect("a cookie that binds the login");
        let flow = reply.header("set-cookie").unwrap();
        assert!(
            flow.contains("HttpOnly") && flow.contains("SameSite=Lax"),
            "{flow}"
        );
        let query = location.query_pairs().into_owned().collect();
        Started { cookie, query }
    }

    async fn callback(&self, cookie: &str, query: &str) -> Reply {
        let path = format!("/auth/oidc/fake/callback?{query}");
        self.raw("GET", &path, &[("cookie", cookie)], b"").await
    }

    /// The provider lets the user in with `claims`, and the browser comes back with the code.
    async fn finish_oidc(&self, fake: &Fake, started: &Started, claims: Value) -> Reply {
        let code = format!("code-{}", fake.grants.lock().unwrap().len());
        let grant = Grant {
            challenge: started.query["code_challenge"].clone(),
            redirect_uri: started.query["redirect_uri"].clone(),
            claims,
        };
        fake.grants.lock().unwrap().insert(code.clone(), grant);
        let query = format!("code={code}&state={}", started.state());
        self.callback(&started.cookie, &query).await
    }

    async fn oidc_login(&self, fake: &Fake, subject: &str, email: &str) -> Reply {
        let started = self.start_oidc(fake).await;
        let claims = claims(fake, &started, subject, email);
        self.finish_oidc(fake, &started, claims).await
    }

    async fn identities(&self) -> i64 {
        self.number("SELECT count(*) FROM user_identities").await
    }
}

fn claims(fake: &Fake, started: &Started, subject: &str, email: &str) -> Value {
    let now = now_ms() / 1000;
    json!({
        "iss": fake.issuer,
        "aud": CLIENT_ID,
        "sub": subject,
        "email": email,
        "email_verified": true,
        "name": "Grace Hopper",
        "nonce": started.query["nonce"],
        "iat": now,
        "exp": now + 300,
    })
}

fn refused_with(reply: &Reply, problem: &str) {
    assert_eq!(reply.status, 303, "{reply:?}");
    assert_eq!(
        reply.header("location"),
        Some(format!("/login?oidc={problem}").as_str())
    );
    assert_eq!(set_cookie(reply, common::COOKIE), None, "no session");
    // The login is over either way: its cookie goes.
    assert_eq!(
        set_cookie(reply, "__Host-oidc").as_deref(),
        Some("__Host-oidc=")
    );
}

#[tokio::test]
async fn a_login_starts_with_pkce_a_state_and_a_nonce() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let started = api.start_oidc(&fake).await;
    let query = &started.query;
    assert_eq!(query["response_type"], "code");
    assert_eq!(query["client_id"], CLIENT_ID);
    assert_eq!(query["redirect_uri"], CALLBACK);
    assert_eq!(query["scope"], "openid email profile");
    assert_eq!(query["code_challenge_method"], "S256");
    for name in ["state", "nonce", "code_challenge"] {
        assert!(query[name].len() >= 43, "{name} has 256 bits");
    }
    // Each login gets its own.
    let again = api.start_oidc(&fake).await;
    assert_ne!(again.state(), started.state());
    assert_ne!(again.query["nonce"], query["nonce"]);
}

#[tokio::test]
async fn an_unknown_provider_is_not_found() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let reply = api.raw("GET", "/auth/oidc/other", &[], b"").await;
    assert_eq!(reply.status, 404);
    let reply = api.raw("GET", "/auth/oidc/other/callback", &[], b"").await;
    assert_eq!(reply.status, 404);
}

#[tokio::test]
async fn a_new_identity_makes_an_account_and_comes_back_to_it() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    api.setup().await;

    let reply = api.oidc_login(&fake, "grace-1", "Grace@Example.org").await;
    assert_eq!(reply.status, 303, "{reply:?}");
    assert_eq!(reply.header("location"), Some("/"));
    assert_eq!(
        set_cookie(&reply, "__Host-oidc").as_deref(),
        Some("__Host-oidc=")
    );
    let cookie = set_cookie(&reply, common::COOKIE).expect("a session");
    let session = api
        .raw("GET", "/session", &[("cookie", &cookie)], b"")
        .await;
    assert_eq!(session.status, 200, "{session:?}");
    let user = &session.body["user"];
    assert_eq!(user["email"], "grace@example.org");
    assert_eq!(user["name"], "Grace Hopper");
    assert_eq!(user["is_instance_admin"], false);
    assert_eq!(session.body["workspaces"], json!([]));
    let unproven = "SELECT count(*) FROM users WHERE password_hash IS NULL \
                    AND email_verified_at IS NOT NULL";
    assert_eq!(
        api.number(unproven).await,
        1,
        "no password, an address the provider proved"
    );

    // The subject is what counts, not the address it gives this time.
    let reply = api
        .oidc_login(&fake, "grace-1", "grace@elsewhere.org")
        .await;
    let cookie = set_cookie(&reply, common::COOKIE).expect("a session");
    let session = api
        .raw("GET", "/session", &[("cookie", &cookie)], b"")
        .await;
    assert_eq!(session.body["user"]["id"], user["id"]);
    assert_eq!(api.number("SELECT count(*) FROM users").await, 2);
    assert_eq!(api.identities().await, 1);
}

#[tokio::test]
async fn an_address_the_provider_did_not_prove_stays_unverified() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    api.setup().await;
    let started = api.start_oidc(&fake).await;
    let mut claims = claims(&fake, &started, "grace-1", "grace@example.org");
    claims["email_verified"] = json!(false);
    let reply = api.finish_oidc(&fake, &started, claims).await;
    assert_eq!(reply.header("location"), Some("/"));
    let verified = "SELECT count(*) FROM users WHERE email_verified_at IS NOT NULL";
    assert_eq!(api.number(verified).await, 0);
}

#[tokio::test]
async fn an_identity_never_takes_over_an_account_by_its_address() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    api.setup().await;
    // Ada's address, which the provider even says it proved.
    let reply = api.oidc_login(&fake, "mallory", "ADA@example.org").await;
    refused_with(&reply, "taken");
    assert_eq!(api.identities().await, 0);
    assert_eq!(api.number("SELECT count(*) FROM users").await, 1);
}

#[tokio::test]
async fn nobody_signs_up_through_a_provider_before_setup_or_with_registration_closed() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let reply = api.oidc_login(&fake, "grace-1", "grace@example.org").await;
    refused_with(&reply, "closed");
    assert_eq!(
        api.number("SELECT count(*) FROM users").await,
        0,
        "setup stays open"
    );

    let api = server(&fake, &[("CADOCS_REGISTRATION", "closed")]).await;
    api.setup().await;
    let reply = api.oidc_login(&fake, "grace-1", "grace@example.org").await;
    refused_with(&reply, "closed");
    assert_eq!(api.identities().await, 0);
}

#[tokio::test]
async fn a_disabled_account_gets_no_session_from_its_identity() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    api.setup().await;
    api.oidc_login(&fake, "grace-1", "grace@example.org").await;
    api.sql("UPDATE users SET disabled_at = 1 WHERE email = 'grace@example.org'")
        .await;
    let reply = api.oidc_login(&fake, "grace-1", "grace@example.org").await;
    refused_with(&reply, "failed");
}

#[tokio::test]
async fn a_callback_counts_only_in_the_browser_that_started_its_login() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    api.setup().await;
    let started = api.start_oidc(&fake).await;
    let claims = claims(&fake, &started, "grace-1", "grace@example.org");
    let grant = |code: &str| {
        let grant = Grant {
            challenge: started.query["code_challenge"].clone(),
            redirect_uri: started.query["redirect_uri"].clone(),
            claims: claims.clone(),
        };
        fake.grants.lock().unwrap().insert(code.to_owned(), grant);
    };
    grant("code");
    let state = started.state();

    // Login CSRF: a code and state sent to someone else's browser, which has no such login.
    let other = api.start_oidc(&fake).await;
    for (cookie, query) in [
        ("", format!("code=code&state={state}")),
        (other.cookie.as_str(), format!("code=code&state={state}")),
        (&started.cookie, "code=code&state=guess".to_owned()),
        (&started.cookie, "code=code".to_owned()),
        (
            &started.cookie,
            format!("error=access_denied&state={state}"),
        ),
    ] {
        refused_with(&api.callback(cookie, &query).await, "failed");
    }
    assert_eq!(fake.tokens.load(Ordering::SeqCst), 0, "no code was spent");

    // The provider of another login: its cookie names another provider.
    let foreign = started.cookie.replacen("=fake.", "=other.", 1);
    let reply = api
        .callback(&foreign, &format!("code=code&state={state}"))
        .await;
    refused_with(&reply, "failed");

    let reply = api
        .callback(&started.cookie, &format!("code=code&state={state}"))
        .await;
    assert_eq!(reply.header("location"), Some("/"), "{reply:?}");
}

#[tokio::test]
async fn an_id_token_for_another_login_opens_nothing() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    api.setup().await;
    let started = api.start_oidc(&fake).await;
    // A token the provider made for another login of the same client: replayed here.
    let mut claims = claims(&fake, &started, "grace-1", "grace@example.org");
    claims["nonce"] = json!("another login");
    let reply = api.finish_oidc(&fake, &started, claims).await;
    refused_with(&reply, "failed");
    assert_eq!(fake.tokens.load(Ordering::SeqCst), 1);
    assert_eq!(api.number("SELECT count(*) FROM users").await, 1);
}

#[tokio::test]
async fn a_callback_with_nothing_to_check_or_trade_spends_nothing() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let started = api.start_oidc(&fake).await;
    let state = started.state();
    // No code to trade, and a planted cookie whose empty state matches an empty one.
    let reply = api
        .callback(&started.cookie, &format!("state={state}"))
        .await;
    refused_with(&reply, "failed");
    let reply = api
        .callback("__Host-oidc=fake...", "code=code&state=")
        .await;
    refused_with(&reply, "failed");
    assert_eq!(fake.tokens.load(Ordering::SeqCst), 0);
}

impl Api {
    /// A logged-in user asks to link the provider: where to send the browser, and the cookie.
    async fn start_link(&self, fake: &Fake, login: &Login) -> Started {
        let reply = self
            .send(
                "POST",
                "/me/identities/fake",
                &login.headers(),
                json!({ "current_password": common::PASSWORD }),
            )
            .await;
        assert_eq!(reply.status, 200, "{reply:?}");
        let location = Url::parse(reply.body["url"].as_str().unwrap()).unwrap();
        assert_eq!(
            location[..url::Position::AfterPath],
            format!("{}/authorize", fake.issuer)
        );
        let cookie = set_cookie(&reply, "__Host-oidc").expect("a cookie that binds the link");
        let query = location.query_pairs().into_owned().collect();
        Started { cookie, query }
    }

    /// The browser comes back from the provider, with the session cookie it always sends.
    async fn finish_link(
        &self,
        fake: &Fake,
        login: &Login,
        started: &Started,
        subject: &str,
    ) -> Reply {
        let claims = claims(fake, started, subject, "grace@example.org");
        let together = Started {
            cookie: format!("{}; {}", started.cookie, login.cookie),
            query: started.query.clone(),
        };
        self.finish_oidc(fake, &together, claims).await
    }
}

fn linked(reply: &Reply, outcome: &str) {
    assert_eq!(reply.status, 303, "{reply:?}");
    assert_eq!(
        reply.header("location"),
        Some(format!("/account?oidc={outcome}").as_str())
    );
    assert_eq!(set_cookie(reply, common::COOKIE), None, "no new session");
}

#[tokio::test]
async fn the_login_screen_learns_the_providers() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let reply = api.raw("GET", "/auth/setup", &[], b"").await;
    assert_eq!(reply.body["oidc"], json!(["fake"]));
    let plain = common::start().await;
    let reply = plain.raw("GET", "/auth/setup", &[], b"").await;
    assert!(reply.body.get("oidc").is_none(), "{:?}", reply.body);
}

#[tokio::test]
async fn an_owner_links_a_provider_and_logs_in_with_it() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let login = Login::from(&api.setup().await);
    let before = api.get(&login, "/me/identities").await;
    assert_eq!(before.body, json!({ "providers": [], "password": true }));

    let started = api.start_link(&fake, &login).await;
    let reply = api.finish_link(&fake, &login, &started, "ada-sso").await;
    linked(&reply, "linked");
    assert_eq!(api.identities().await, 1);
    let after = api.get(&login, "/me/identities").await;
    assert_eq!(after.body["providers"], json!(["fake"]));

    // The same subject now logs into Ada, not into a new account.
    let reply = api.oidc_login(&fake, "ada-sso", "other@example.org").await;
    let cookie = set_cookie(&reply, common::COOKIE).expect("a session");
    let session = api
        .raw("GET", "/session", &[("cookie", &cookie)], b"")
        .await;
    assert_eq!(session.body["user"]["email"], "ada@example.org");
    assert_eq!(api.number("SELECT count(*) FROM users").await, 1);
}

#[tokio::test]
async fn a_subject_of_another_account_is_refused() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let ada = Login::from(&api.setup().await);
    let other = api.add_and_login("u2", "two@example.org").await;
    let started = api.start_link(&fake, &other).await;
    linked(
        &api.finish_link(&fake, &other, &started, "shared").await,
        "linked",
    );

    let started = api.start_link(&fake, &ada).await;
    linked(
        &api.finish_link(&fake, &ada, &started, "shared").await,
        "taken",
    );
    assert_eq!(api.identities().await, 1);
}

#[tokio::test]
async fn a_second_subject_of_the_same_provider_is_refused() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let ada = Login::from(&api.setup().await);
    let started = api.start_link(&fake, &ada).await;
    linked(
        &api.finish_link(&fake, &ada, &started, "one").await,
        "linked",
    );
    let started = api.start_link(&fake, &ada).await;
    linked(
        &api.finish_link(&fake, &ada, &started, "two").await,
        "taken",
    );
    assert_eq!(api.identities().await, 1);
}

#[tokio::test]
async fn a_link_is_for_the_user_who_asked_and_needs_a_session() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let ada = Login::from(&api.setup().await);
    let other = api.add_and_login("u2", "two@example.org").await;
    let started = api.start_link(&fake, &ada).await;
    // Someone else's session holds Ada's cookie: nothing gets linked to either.
    let reply = api.finish_link(&fake, &other, &started, "mallory").await;
    linked(&reply, "failed");
    let reply = api
        .finish_oidc(&fake, &started, claims(&fake, &started, "x", "x@e.org"))
        .await;
    linked(&reply, "failed");
    assert_eq!(api.identities().await, 0);
    let reply = api
        .send("POST", "/me/identities/fake", &[], json!({}))
        .await;
    assert_eq!(reply.status, 401);
    let reply = api
        .send("POST", "/me/identities/other", &ada.headers(), json!({}))
        .await;
    assert_eq!(reply.status, 404);
}

#[tokio::test]
async fn an_identity_can_go_unless_it_is_the_only_way_in() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let ada = Login::from(&api.setup().await);
    let started = api.start_link(&fake, &ada).await;
    linked(
        &api.finish_link(&fake, &ada, &started, "ada-sso").await,
        "linked",
    );
    let gone = api
        .send("DELETE", "/me/identities/fake", &ada.headers(), json!({}))
        .await;
    assert_eq!(gone.status, 204, "{gone:?}");
    assert_eq!(api.identities().await, 0);
    let again = api
        .send("DELETE", "/me/identities/fake", &ada.headers(), json!({}))
        .await;
    assert_eq!(again.status, 404);

    // An account the provider made has no password: its identity is all it has.
    let reply = api.oidc_login(&fake, "grace-1", "grace@example.org").await;
    let cookie = set_cookie(&reply, common::COOKIE).unwrap();
    let session = api
        .raw("GET", "/session", &[("cookie", &cookie)], b"")
        .await;
    let grace = Login {
        cookie,
        csrf: session.body["csrf_token"].as_str().unwrap().to_owned(),
    };
    let state = api.get(&grace, "/me/identities").await;
    assert_eq!(
        state.body,
        json!({ "providers": ["fake"], "password": false })
    );
    let kept = api
        .send("DELETE", "/me/identities/fake", &grace.headers(), json!({}))
        .await;
    assert_eq!(kept.status, 409, "{kept:?}");
    assert_eq!(api.identities().await, 1);
}

#[tokio::test]
async fn a_stolen_session_cannot_tie_its_own_provider_to_an_account_with_a_password() {
    let fake = fake_provider().await;
    let api = server(&fake, &[]).await;
    let ada = Login::from(&api.setup().await);
    let path = "/me/identities/fake";
    let none = api.send("POST", path, &ada.headers(), json!({})).await;
    assert_eq!(none.status, 400, "{none:?}");
    assert_eq!(none.body["error"]["fields"]["current_password"], "required");
    let wrong = json!({ "current_password": "not the password" });
    let wrong = api.send("POST", path, &ada.headers(), wrong).await;
    assert_eq!(wrong.status, 400, "{wrong:?}");
    assert_eq!(set_cookie(&wrong, "__Host-oidc"), None);
}
