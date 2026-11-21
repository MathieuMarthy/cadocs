# ADR 0028: OIDC through a client made of what the binary already holds, and identities that never take an account by its address

Status: accepted.

## Context

Step 48 adds OpenID Connect login. The server must fetch a provider's discovery document and keys, trade a code for an ID token, and check that token's RS256 signature. That needs an HTTPS client and RSA, which the default build did not use as a client. The workspace plan listed `openidconnect` "or a smaller client" as an optional feature, to evaluate against a hand-written flow.

What the default build already linked: hyper 1 (as axum's server), `hyper-util`, `http-body-util`, rustls 0.23 with `ring`, `tokio-rustls` and `rustls-native-certs` (through `lettre`, ADR 0026), `url` and `serde_json`. `reqwest` is only in the `s3` build.

It also had to say how an identity meets an account that exists.

## Decision

**The client is hyper's, over the rustls and ring already linked.** `oidc.rs` opens one connection per request (`hyper::client::conn::http1`), with TLS from `tokio-rustls` and the system roots read for that request, as `lettre` does. It follows no redirect, gives up after 10 seconds, and reads at most 256 KiB. No crate is new: only hyper's `client` feature and direct dependencies on crates already in the tree. The ID token is checked by hand with `ring` (RS256 only), about 60 lines, against OpenID Connect Core 3.1.3.7: the signature by a key of the provider's set, the issuer, the audience and `azp`, the expiry and the nonce. In every build, not behind a feature: it costs 0.4 MB of binary (measured below), less than a second image flavour costs to maintain.

**Nothing is cached.** Discovery and keys are fetched at each login. Logins are rare, and a cache would hold memory at idle for a provider that may be used once a day. A provider costs its four settings in memory, and a login holds the roots and a few buffers while it runs.

**HTTPS only, except to this machine.** An issuer must be `https`, or `http` on a loopback address (a provider on the same host, and the tests). The endpoints its discovery document names must be `https` too, unless the issuer itself is such a local `http` one, and then they may only be `http` on a loopback address: a remote document cannot send the client secret in clear to a local service.

**The login is bound to its browser by a cookie.** The start sets a `__Host-oidc` cookie (`oidc` over plain HTTP), `HttpOnly`, `SameSite=Lax` so the provider's redirect back carries it, valid 10 minutes, holding the provider's id, the state, the nonce and the PKCE verifier (S256), 256 random bits each. The callback compares the state in constant time before any request to the provider, then sends the verifier with the code, and the token must carry the nonce. Nothing is kept on the server between the two.

**An identity is its provider's subject, never an address.** `user_identities` maps `(provider, sub)` to an account. A subject with no account makes one, with no password and no workspace, when registration is open (`CADOCS_REGISTRATION`, as mail registration) and setup is done. Its address is verified when the provider says `email_verified: true`. A subject whose address an account already holds is refused (`taken`), even when the provider says it verified that address: linking an identity to an existing account is done by its owner, logged in, from the account screen (step 49). Matching by address would hand an account to whoever controls an address at the provider, or to a provider that lets users set theirs.

**The provider's factors replace the account's.** A login through a provider asks for neither the code by mail nor the app: the provider authenticated the person, with its own second factor if the operator asked for one.

## Consequences

- Binary: 13,462,408 bytes before, 13,867,112 after (+0.4 MB), with the client, the routes and the checks.
- Idle RSS, 8 seconds after start with setup pending, three runs each from a local disk, one provider set: anonymous memory is the same (2.2 to 2.4 MB in both). The whole RSS moves between 11.8 and 14.6 MB in both, with the pages of the binary the kernel reads ahead, which a binary 0.4 MB larger shifts a little. Nothing is held between logins. Step 55 runs the full benchmark.
- An operator who wants only their own people sets the provider to admit only them, or closes registration and invites people, who then link the provider.
- An account made by a provider has no password, so the routes that need the current password (email change, password change, the authenticator app) are closed to it until it gets one through a reset link.
- A user who turned on the app and later links a provider can log in through the provider without the app. The account screen of step 49 says so.
- Only RS256 tokens pass. A provider that signs with HS256 or ES256 is refused with a line in the log.
- The TLS path is checked by hand against Authentik and Keycloak in step 49: the tests run against a fake provider over plain HTTP on loopback.

## Rejected alternatives

- **`openidconnect`.** It brings `oauth2`, its own HTTP client choice (reqwest by default), `rsa` or `ring`, `serde_with`, `chrono` and more, for the three requests and one signature check this needs.
- **`reqwest` in every build.** Already reviewed for S3 (ADR 0020), but it adds its own layer over hyper, a connection pool and 1.4 MB of idle RSS measured in the S3 build, where a handful of requests a day need neither.
- **`jsonwebtoken`.** A second JSON Web Token stack and its crypto backend, for one algorithm `ring` already verifies.
- **Trusting the ID token on TLS alone,** which OpenID Connect allows for a token from the token endpoint. The signature check is cheap, and it holds if the TLS check is ever wrong.
- **Linking by a verified address.** See above: it turns the provider's view of an address into a key to the account.
- **Keeping the flow on the server.** A map of logins in progress is memory any visitor can fill; the cookie holds it in the browser that needs it.
