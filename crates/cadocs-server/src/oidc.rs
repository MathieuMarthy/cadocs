//! OpenID Connect (V1): the authorization code flow with PKCE against the providers the operator
//! names, and the three requests it makes, over the hyper, rustls and ring already in the binary
//! (ADR 0028). Nothing is cached: discovery and keys are fetched at each login, so a provider
//! costs no memory between logins.

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, anyhow, bail, ensure};
use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use bytes::Bytes;
use http_body_util::{BodyExt, Full, Limited};
use hyper::header::{ACCEPT, AUTHORIZATION, CONTENT_TYPE, HOST};
use hyper::{Request, StatusCode};
use hyper_util::rt::TokioIo;
use ring::signature::{RSA_PKCS1_2048_8192_SHA256, RsaPublicKeyComponents};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::net::TcpStream;
use tokio_rustls::TlsConnector;
use tokio_rustls::rustls::pki_types::ServerName;
use tokio_rustls::rustls::{ClientConfig, RootCertStore};
use url::{Host, Url};

/// Each request, from the connection to the last byte of the answer.
const TIMEOUT: Duration = Duration::from_secs(10);
/// Far above a discovery document, a key set or a token answer.
const MAX_ANSWER_BYTES: usize = 256 * 1024;
/// What `user_identities` accepts as a subject. OpenID Connect allows 255 ASCII characters.
const MAX_SUBJECT_BYTES: usize = 255;

/// A provider from `CADOCS_OIDC_*`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Provider {
    /// Lowercase ASCII letters and digits: its routes and its variables are named after it.
    pub id: String,
    pub issuer: String,
    pub client_id: String,
    pub client_secret: String,
}

/// What a login keeps in the browser between its start and its callback: 256 random bits each.
pub struct Flow {
    pub state: String,
    pub nonce: String,
    pub verifier: String,
}

impl Flow {
    pub fn new() -> Result<Self, getrandom::Error> {
        let random = || {
            let mut bytes = [0; 32];
            getrandom::fill(&mut bytes).map(|()| URL_SAFE_NO_PAD.encode(bytes))
        };
        Ok(Self {
            state: random()?,
            nonce: random()?,
            verifier: random()?,
        })
    }
}

/// What the provider vouches for, once its ID token passed every check.
#[derive(Debug, PartialEq, Eq)]
pub struct Identity {
    pub subject: String,
    pub email: Option<String>,
    pub email_verified: bool,
    pub name: Option<String>,
}

/// The parts of a discovery document a login uses.
#[derive(Deserialize)]
pub struct Endpoints {
    issuer: String,
    authorization_endpoint: String,
    token_endpoint: String,
    jwks_uri: String,
}

/// An `https` URL, or an `http` one to this machine: where a password, a code, a secret or a
/// token goes.
pub fn check_url(url: &str) -> anyhow::Result<Url> {
    let parsed = Url::parse(url).map_err(|error| anyhow!("{url:?} is not a URL: {error}"))?;
    let local = match parsed.host() {
        Some(Host::Domain(name)) => name == "localhost",
        Some(Host::Ipv4(ip)) => ip.is_loopback(),
        Some(Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    match parsed.scheme() {
        "https" => {}
        "http" if local => {}
        _ => bail!("{url:?} must use https, or http to this machine only"),
    }
    Ok(parsed)
}

/// OpenID Connect Discovery 1.0. The document must name the issuer it was fetched from.
pub async fn discover(provider: &Provider) -> anyhow::Result<Endpoints> {
    let issuer = provider.issuer.trim_end_matches('/');
    let url = format!("{issuer}/.well-known/openid-configuration");
    let endpoints: Endpoints = get_json(&url).await?;
    endpoints.check(provider)?;
    Ok(endpoints)
}

impl Endpoints {
    /// The document must name the issuer it was fetched from, and http is for an issuer on this
    /// machine only: a remote document cannot send the secret in clear to a local service.
    fn check(&self, provider: &Provider) -> anyhow::Result<()> {
        ensure!(
            self.issuer == provider.issuer,
            "the discovery document names another issuer: {:?}",
            self.issuer
        );
        let local = provider.issuer.starts_with("http:");
        for url in [
            &self.authorization_endpoint,
            &self.token_endpoint,
            &self.jwks_uri,
        ] {
            let scheme = check_url(url)?;
            ensure!(
                local || scheme.scheme() == "https",
                "{url:?} must use https"
            );
        }
        Ok(())
    }
}

/// Where the browser goes to log in, with the PKCE challenge of RFC 7636.
pub fn authorization_url(
    endpoints: &Endpoints,
    provider: &Provider,
    redirect_uri: &str,
    flow: &Flow,
) -> anyhow::Result<String> {
    let mut url = check_url(&endpoints.authorization_endpoint)?;
    let challenge = URL_SAFE_NO_PAD.encode(Sha256::digest(&flow.verifier));
    url.query_pairs_mut()
        .append_pair("response_type", "code")
        .append_pair("client_id", &provider.client_id)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("scope", "openid email profile")
        .append_pair("state", &flow.state)
        .append_pair("nonce", &flow.nonce)
        .append_pair("code_challenge", &challenge)
        .append_pair("code_challenge_method", "S256");
    Ok(url.into())
}

#[derive(Deserialize)]
struct TokenAnswer {
    id_token: String,
}

/// Trades the code for an ID token, and checks it (OpenID Connect Core 1.0, 3.1.3.7).
pub async fn identify(
    endpoints: &Endpoints,
    provider: &Provider,
    redirect_uri: &str,
    flow: &Flow,
    code: &str,
) -> anyhow::Result<Identity> {
    let form = url::form_urlencoded::Serializer::new(String::new())
        .append_pair("grant_type", "authorization_code")
        .append_pair("code", code)
        .append_pair("redirect_uri", redirect_uri)
        .append_pair("code_verifier", &flow.verifier)
        .finish();
    // client_secret_basic, which every provider must accept, each half form-encoded (RFC 6749).
    let encode =
        |text: &str| -> String { url::form_urlencoded::byte_serialize(text.as_bytes()).collect() };
    let basic: String = [encode(&provider.client_id), encode(&provider.client_secret)].join(":");
    let basic = format!("Basic {}", STANDARD.encode(basic));
    let request = request(&endpoints.token_endpoint)?
        .method("POST")
        .header(AUTHORIZATION, basic)
        .header(CONTENT_TYPE, "application/x-www-form-urlencoded")
        .body(Full::new(Bytes::from(form)))?;
    let answer: TokenAnswer = json(&endpoints.token_endpoint, request).await?;
    let keys: KeySet = get_json(&endpoints.jwks_uri).await?;
    let now = cadocs_store::now_ms() / 1000;
    check_id_token(&answer.id_token, &keys, provider, &flow.nonce, now)
}

#[derive(Deserialize)]
struct KeySet {
    keys: Vec<Key>,
}

#[derive(Deserialize)]
struct Key {
    kty: String,
    kid: Option<String>,
    #[serde(rename = "use")]
    usage: Option<String>,
    n: Option<String>,
    e: Option<String>,
}

#[derive(Deserialize)]
struct Header {
    alg: String,
    kid: Option<String>,
}

#[derive(Deserialize)]
struct Claims {
    iss: String,
    sub: String,
    aud: Value,
    azp: Option<String>,
    exp: i64,
    nonce: Option<String>,
    email: Option<String>,
    email_verified: Option<Value>,
    name: Option<String>,
    preferred_username: Option<String>,
}

fn check_id_token(
    token: &str,
    keys: &KeySet,
    provider: &Provider,
    nonce: &str,
    now: i64,
) -> anyhow::Result<Identity> {
    let (signed, signature) = token
        .rsplit_once('.')
        .context("an ID token has three parts")?;
    let (header, payload) = signed
        .split_once('.')
        .context("an ID token has three parts")?;
    let header: Header = serde_json::from_slice(&decode(header)?)?;
    // RS256 only: the one every provider offers. `none` and HMAC never pass.
    ensure!(
        header.alg == "RS256",
        "ID token signed with {:?}, not RS256",
        header.alg
    );
    let signature = decode(signature)?;
    let signed_by = |key: &Key| {
        let (Some(n), Some(e)) = (&key.n, &key.e) else {
            return false;
        };
        let (Ok(n), Ok(e)) = (decode(n), decode(e)) else {
            return false;
        };
        let key = RsaPublicKeyComponents { n, e };
        let algorithm = &RSA_PKCS1_2048_8192_SHA256;
        key.verify(algorithm, signed.as_bytes(), &signature).is_ok()
    };
    let verified = keys
        .keys
        .iter()
        .filter(|key| key.kty == "RSA" && key.usage.as_deref().is_none_or(|u| u == "sig"))
        .filter(|key| header.kid.is_none() || key.kid.is_none() || key.kid == header.kid)
        .any(signed_by);
    ensure!(
        verified,
        "the ID token's signature matches no key of the provider"
    );

    let claims: Claims = serde_json::from_slice(&decode(payload)?)?;
    ensure!(
        claims.iss == provider.issuer,
        "ID token from another issuer: {:?}",
        claims.iss
    );
    let client = provider.client_id.as_str();
    let audience = match &claims.aud {
        Value::String(one) => one == client,
        Value::Array(many) => many.iter().any(|one| one.as_str() == Some(client)),
        _ => false,
    };
    ensure!(audience, "ID token for another client: {}", claims.aud);
    ensure!(
        claims.azp.as_deref().is_none_or(|azp| azp == client),
        "ID token authorized for another client"
    );
    ensure!(now < claims.exp, "the ID token expired");
    ensure!(
        claims.nonce.as_deref() == Some(nonce),
        "ID token for another login"
    );
    ensure!(
        !claims.sub.is_empty() && claims.sub.len() <= MAX_SUBJECT_BYTES,
        "ID token with no usable subject"
    );
    Ok(Identity {
        subject: claims.sub,
        email: claims.email,
        email_verified: claims.email_verified == Some(Value::Bool(true)),
        name: claims.name.or(claims.preferred_username),
    })
}

fn decode(part: &str) -> anyhow::Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(part)
        .context("not base64url in an ID token or a key")
}

/// A request to `url`, which `check_url` lets through, with its `Host`.
fn request(url: &str) -> anyhow::Result<hyper::http::request::Builder> {
    let parsed = check_url(url)?;
    let host = &parsed[url::Position::BeforeHost..url::Position::AfterPort];
    Ok(Request::builder()
        .uri(&parsed[url::Position::BeforePath..url::Position::AfterQuery])
        .header(HOST, host)
        .header(ACCEPT, "application/json"))
}

async fn get_json<T: DeserializeOwned>(url: &str) -> anyhow::Result<T> {
    let request = request(url)?.method("GET").body(Full::default())?;
    json(url, request).await
}

async fn json<T: DeserializeOwned>(url: &str, request: Request<Full<Bytes>>) -> anyhow::Result<T> {
    let answer = tokio::time::timeout(TIMEOUT, send(url, request))
        .await
        .map_err(|_| anyhow!("{url} did not answer in {TIMEOUT:?}"))??;
    serde_json::from_slice(&answer).with_context(|| format!("{url} answered no usable JSON"))
}

/// One connection per request, as mail does: nothing stays open or loaded between logins.
async fn send(url: &str, request: Request<Full<Bytes>>) -> anyhow::Result<Bytes> {
    let parsed = check_url(url)?;
    let port = parsed.port_or_known_default().context("no port")?;
    let host = match parsed.host().context("no host")? {
        Host::Domain(name) => name.to_owned(),
        Host::Ipv4(ip) => ip.to_string(),
        Host::Ipv6(ip) => ip.to_string(),
    };
    let tcp = TcpStream::connect((host.as_str(), port))
        .await
        .with_context(|| format!("cannot reach {url}"))?;
    if parsed.scheme() == "http" {
        return exchange(tcp, request).await;
    }
    let name = match host.parse::<IpAddr>() {
        Ok(ip) => ServerName::from(ip),
        Err(_) => ServerName::try_from(host)?,
    };
    let tls = TlsConnector::from(Arc::new(tls_config()?))
        .connect(name, tcp)
        .await
        .with_context(|| format!("no TLS with {url}"))?;
    exchange(tls, request).await
}

/// The system's roots, read for each request as `lettre` does, so none stay in memory.
fn tls_config() -> anyhow::Result<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.add_parsable_certificates(rustls_native_certs::load_native_certs().certs);
    ensure!(
        !roots.is_empty(),
        "no root certificate found on this system"
    );
    let ring = Arc::new(tokio_rustls::rustls::crypto::ring::default_provider());
    Ok(ClientConfig::builder_with_provider(ring)
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots)
        .with_no_client_auth())
}

async fn exchange<T>(io: T, request: Request<Full<Bytes>>) -> anyhow::Result<Bytes>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (mut sender, connection) = hyper::client::conn::http1::handshake(TokioIo::new(io)).await?;
    let answer = async {
        let response = sender.send_request(request).await?;
        let status = response.status();
        ensure!(status == StatusCode::OK, "answered {status}");
        let body = Limited::new(response.into_body(), MAX_ANSWER_BYTES);
        let body = body.collect().await.map_err(|error| anyhow!(error))?;
        Ok(body.to_bytes())
    };
    // The connection does the reading and writing: it runs beside the answer, and ends with it.
    tokio::select! {
        answer = answer => answer,
        Err(error) = connection => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use ring::rand::SystemRandom;
    use ring::signature::{RSA_PKCS1_SHA256, RsaKeyPair};
    use serde_json::json;

    use super::*;

    const NOW: i64 = 1_800_000_000;

    fn provider() -> Provider {
        Provider {
            id: "sso".to_owned(),
            issuer: "https://sso.example.org".to_owned(),
            client_id: "wiki".to_owned(),
            client_secret: "s3cret".to_owned(),
        }
    }

    fn key() -> RsaKeyPair {
        RsaKeyPair::from_pkcs8(include_bytes!("../tests/oidc-key.der")).unwrap()
    }

    fn keys() -> KeySet {
        let public = RsaPublicKeyComponents::<Vec<u8>>::from(key().public());
        KeySet {
            keys: vec![Key {
                kty: "RSA".to_owned(),
                kid: Some("k1".to_owned()),
                usage: Some("sig".to_owned()),
                n: Some(URL_SAFE_NO_PAD.encode(public.n)),
                e: Some(URL_SAFE_NO_PAD.encode(public.e)),
            }],
        }
    }

    fn claims() -> Value {
        json!({
            "iss": "https://sso.example.org",
            "aud": "wiki",
            "sub": "u-1",
            "exp": NOW + 60,
            "nonce": "n",
            "email": "grace@example.org",
            "email_verified": true,
            "preferred_username": "grace",
        })
    }

    fn token(header: &Value, claims: &Value) -> String {
        let header = URL_SAFE_NO_PAD.encode(header.to_string());
        let payload = URL_SAFE_NO_PAD.encode(claims.to_string());
        let signed = format!("{header}.{payload}");
        let key = key();
        let mut signature = vec![0; key.public().modulus_len()];
        key.sign(
            &RSA_PKCS1_SHA256,
            &SystemRandom::new(),
            signed.as_bytes(),
            &mut signature,
        )
        .unwrap();
        format!("{signed}.{}", URL_SAFE_NO_PAD.encode(signature))
    }

    fn check(token: &str) -> anyhow::Result<Identity> {
        check_id_token(token, &keys(), &provider(), "n", NOW)
    }

    fn signed(claims: &Value) -> String {
        token(&json!({ "alg": "RS256", "kid": "k1" }), claims)
    }

    #[test]
    fn a_signed_token_for_this_login_gives_its_identity() {
        let identity = check(&signed(&claims())).unwrap();
        let expected = Identity {
            subject: "u-1".to_owned(),
            email: Some("grace@example.org".to_owned()),
            email_verified: true,
            name: Some("grace".to_owned()),
        };
        assert_eq!(identity, expected);
        // An audience list that holds this client, and no `kid` at all.
        let mut more = claims();
        more["aud"] = json!(["other", "wiki"]);
        more["azp"] = json!("wiki");
        more["email_verified"] = json!("true");
        let token = token(&json!({ "alg": "RS256" }), &more);
        assert!(
            !check(&token).unwrap().email_verified,
            "only a JSON true proves it"
        );
    }

    #[test]
    fn a_token_fails_any_check_of_openid_connect_core() {
        let changed = |name: &str, value: Value| {
            let mut claims = claims();
            claims[name] = value;
            signed(&claims)
        };
        for (token, why) in [
            (changed("iss", json!("https://evil.example.org")), "issuer"),
            (changed("aud", json!("other")), "client"),
            (changed("aud", json!(["other"])), "client"),
            (changed("azp", json!("other")), "client"),
            (changed("exp", json!(NOW)), "expired"),
            (changed("nonce", json!("other")), "login"),
            (changed("nonce", Value::Null), "login"),
            (changed("sub", json!("")), "subject"),
            (changed("sub", json!("x".repeat(256))), "subject"),
        ] {
            let error = check(&token).unwrap_err().to_string();
            assert!(error.contains(why), "{why}: {error}");
        }
    }

    #[test]
    fn only_an_rs256_signature_of_a_provider_key_counts() {
        let good = signed(&claims());
        let (signed_part, signature) = good.rsplit_once('.').unwrap();
        let mut forged = URL_SAFE_NO_PAD.decode(signature).unwrap();
        forged[0] ^= 1;
        let forged = format!("{signed_part}.{}", URL_SAFE_NO_PAD.encode(forged));
        let other_claims = signed_part.split_once('.').unwrap().0.to_owned()
            + "."
            + &URL_SAFE_NO_PAD.encode(json!({ "sub": "admin" }).to_string())
            + "."
            + signature;
        let unsigned = |alg: &str| {
            let header = URL_SAFE_NO_PAD.encode(json!({ "alg": alg }).to_string());
            let payload = URL_SAFE_NO_PAD.encode(claims().to_string());
            format!("{header}.{payload}.")
        };
        let unknown_key = token(&json!({ "alg": "RS256", "kid": "k2" }), &claims());
        for token in [
            forged,
            other_claims,
            unsigned("none"),
            unsigned("HS256"),
            unknown_key,
            "a.b".to_owned(),
        ] {
            assert!(check(&token).is_err(), "{token}");
        }
        // A key meant for encryption signs nothing.
        let mut keys = keys();
        keys.keys[0].usage = Some("enc".to_owned());
        assert!(check_id_token(&good, &keys, &provider(), "n", NOW).is_err());
    }

    #[test]
    fn a_key_with_no_kid_still_counts_for_a_token_that_names_one() {
        let mut keys = keys();
        keys.keys[0].kid = None;
        let token = signed(&claims());
        assert!(check_id_token(&token, &keys, &provider(), "n", NOW).is_ok());
    }

    #[test]
    fn a_request_sends_no_fragment() {
        let request = request("https://sso.example.org:8443/token?a=1#x").unwrap();
        let request = request.body(Full::<Bytes>::default()).unwrap();
        assert_eq!(request.uri(), "/token?a=1");
        assert_eq!(request.headers()[HOST], "sso.example.org:8443");
    }

    #[test]
    fn endpoints_are_https_unless_the_issuer_is_on_this_machine() {
        let endpoints = |issuer: &str, token: &str| Endpoints {
            issuer: issuer.to_owned(),
            authorization_endpoint: "https://sso.example.org/auth".to_owned(),
            token_endpoint: token.to_owned(),
            jwks_uri: "https://sso.example.org/jwks".to_owned(),
        };
        let https = provider();
        let found = endpoints(&https.issuer, "https://sso.example.org/token");
        assert!(found.check(&https).is_ok());
        // A secret in clear to a local service, named by a remote document.
        let found = endpoints(&https.issuer, "http://127.0.0.1:8080/token");
        assert!(found.check(&https).is_err());
        let found = endpoints("https://other.example.org", "https://sso.example.org/token");
        assert!(found.check(&https).is_err(), "another issuer");
        let local = Provider {
            issuer: "http://127.0.0.1:9000".to_owned(),
            ..provider()
        };
        let found = endpoints(&local.issuer, "http://127.0.0.1:9000/token");
        assert!(found.check(&local).is_ok());
    }

    #[test]
    fn only_https_or_this_machine_is_reached() {
        for allowed in [
            "https://sso.example.org/realms/main",
            "http://localhost:8080",
            "http://127.0.0.1:9000/o/",
            "http://[::1]/",
        ] {
            assert!(check_url(allowed).is_ok(), "{allowed}");
        }
        for refused in [
            "http://sso.example.org",
            "http://10.0.0.1",
            "ftp://sso.example.org",
            "sso.example.org",
        ] {
            assert!(check_url(refused).is_err(), "{refused}");
        }
    }

    #[test]
    fn a_login_sends_the_s256_challenge_of_its_verifier() {
        let endpoints = Endpoints {
            issuer: String::new(),
            authorization_endpoint: "https://sso.example.org/auth?tenant=1".to_owned(),
            token_endpoint: String::new(),
            jwks_uri: String::new(),
        };
        let flow = Flow {
            state: "s".to_owned(),
            nonce: "n".to_owned(),
            // RFC 7636, appendix B.
            verifier: "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk".to_owned(),
        };
        let url = authorization_url(&endpoints, &provider(), "https://wiki/cb", &flow).unwrap();
        let url = Url::parse(&url).unwrap();
        let query: Vec<_> = url.query_pairs().into_owned().collect();
        assert!(query.contains(&("tenant".to_owned(), "1".to_owned())));
        let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM".to_owned();
        assert!(query.contains(&("code_challenge".to_owned(), challenge)));
    }
}
