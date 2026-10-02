//! Covers `src/client.rs` — authorize URL and pre-network exchange checks.

use nest_rs_authn::{AuthError, JwtOptions, JwtService};
use nest_rs_oauth_client::{OAuthClient, TARGET};
use nest_rs_testing::LogCapture;
use serde::Deserialize;

use super::config::valid_config;

#[derive(Debug, Deserialize)]
struct Transaction {
    typ: String,
    provider: String,
    csrf: String,
    pkce: String,
    #[allow(dead_code)]
    exp: u64,
}

fn client() -> OAuthClient {
    OAuthClient::new(valid_config()).expect("client builds")
}

#[test]
fn authorize_url_carries_client_scope_and_pkce_and_a_verifiable_transaction() {
    let jwt = crate::jwt();
    let auth = client().authorize(&jwt, "acme").expect("authorize");

    assert!(auth.url.starts_with("https://provider.example/authorize?"));
    assert!(auth.url.contains("client_id=demo-client"));
    assert!(auth.url.contains("scope=read%3Auser"));
    assert!(auth.url.contains("code_challenge="));
    assert!(auth.url.contains("code_challenge_method=S256"));

    let tx: Transaction = jwt
        .verify_handshake("oauth-tx", &auth.transaction)
        .expect("transaction verifies as a handshake token");
    assert!(auth.url.contains(&format!("state={}", tx.csrf)));
    assert!(!tx.pkce.is_empty());
    // The transaction names what it is and which provider it belongs to, so a
    // shared cookie cannot carry it across flows.
    assert_eq!(tx.typ, "oauth_tx");
    assert_eq!(tx.provider, "acme");
}

/// The transaction cookie is handed to a user agent, so what matters is not
/// that *we* can read it back but that a **resource server cannot mistake it
/// for a credential**. RFC 9068 §2.1 gives `at+jwt` exactly that job —
/// "preventing … tokens issued for other purposes from being accepted as access
/// tokens by resource servers" — and the cookie is signed by the same service,
/// with the same key, carrying the same `aud`/`iss`. The media type is the only
/// thing separating them, so it is asserted rather than assumed.
#[test]
fn the_transaction_cookie_is_not_accepted_as_an_access_token() {
    let jwt = crate::jwt();
    let auth = client().authorize(&jwt, "acme").expect("authorize");

    assert!(
        matches!(
            jwt.verify::<Transaction>(&auth.transaction),
            Err(AuthError::InvalidToken)
        ),
        "a handshake token must not verify as an access token",
    );
}

/// The mirror direction: an access token replayed as the transaction cookie.
/// Without it the callback would accept any token this deployment ever minted.
#[test]
fn an_access_token_is_not_accepted_as_a_transaction() {
    let jwt = crate::jwt();
    let access = jwt
        .sign(&serde_json::json!({
            "typ": "oauth_tx",
            "provider": "acme",
            "csrf": "forged",
            "pkce": "forged",
            "exp": jwt.expiry(),
        }))
        .expect("sign an access token shaped like a transaction");

    assert!(
        matches!(
            jwt.verify_handshake::<Transaction>("oauth-tx", &access),
            Err(AuthError::InvalidToken)
        ),
        "an access token must not verify as a handshake token",
    );
}

#[tokio::test]
async fn exchange_rejects_a_state_that_does_not_match_the_transaction() {
    let logs = LogCapture::install();
    let jwt = crate::jwt();
    let auth = client().authorize(&jwt, "acme").expect("authorize");

    // `TokenSet` is intentionally not `Debug` (it carries tokens), so match
    // rather than `expect_err`.
    let Err(err) = client()
        .exchange(&jwt, "acme", &auth.transaction, "not-the-csrf", "some-code")
        .await
    else {
        panic!("state mismatch is rejected");
    };
    assert!(matches!(err, AuthError::Failed(_)));

    // A CSRF mismatch on a callback is an attack signature, not a user error:
    // the caller is told only "OAuth state mismatch", so the `reason` field is
    // what separates a replayed transaction from a forged one in the log an
    // incident queries.
    let event = logs.expect_one(TARGET, "OAuth callback rejected");
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("csrf_state_mismatch")
    );
}

#[tokio::test]
async fn exchange_reports_a_transaction_cookie_that_does_not_verify() {
    // Regression: the third way a callback is refused. `JwtService` files its
    // typed decode reason at `debug` because on the *strategy* path `AuthnGuard`
    // emits the single `warn` — but nothing guards this path, and
    // `AuthError::render` logs only `Failed`/`Unavailable`, so a forged or
    // replayed handshake cookie produced an `InvalidSignature` that left no
    // `warn` anywhere while its two siblings both did.
    let logs = LogCapture::install();
    let attacker = JwtService::new(JwtOptions::new("attacker-secret-padded-to-32-byt"))
        .expect("HMAC JwtService");
    let forged = attacker
        .sign(&serde_json::json!({
            "typ": "oauth_tx",
            "provider": "acme",
            "csrf": "agreed-state",
            "pkce": "verifier",
            "exp": attacker.expiry(),
        }))
        .expect("sign with the wrong key");

    let Err(err) = client()
        .exchange(&crate::jwt(), "acme", &forged, "agreed-state", "some-code")
        .await
    else {
        panic!("a transaction signed by another key is rejected");
    };
    assert!(matches!(err, AuthError::InvalidSignature), "{err}");

    let event = logs.expect_one(TARGET, "OAuth callback rejected");
    assert_eq!(
        event.level, "warn",
        "a forged handshake cookie is a security event, not a debug line",
    );
    assert_eq!(
        event.field("reason").as_deref(),
        Some("invalid_transaction"),
        "grouped under its own low-cardinality reason: {event:?}",
    );
    assert_eq!(
        event.field("token_reason").as_deref(),
        Some("invalid_signature"),
        "and it carries which token check failed: {event:?}",
    );
}

#[tokio::test]
async fn an_expired_transaction_cookie_is_reported_the_same_way() {
    // The replay half of the same class: a cookie this service really did mint,
    // presented after its 10-minute window. `AuthError::Expired` is the one
    // decode outcome `JwtService` never even logs at `debug`, so without the
    // site's own `warn` it was silent end to end.
    let logs = LogCapture::install();
    let jwt = crate::jwt();
    let stale = jwt
        .sign(&serde_json::json!({
            "typ": "oauth_tx",
            "provider": "acme",
            "csrf": "agreed-state",
            "pkce": "verifier",
            "exp": jsonwebtoken::get_current_timestamp() - 3600,
        }))
        .expect("sign");

    let Err(err) = client()
        .exchange(&jwt, "acme", &stale, "agreed-state", "some-code")
        .await
    else {
        panic!("an expired transaction is rejected");
    };
    assert!(matches!(err, AuthError::Expired), "{err}");

    let event = logs.expect_one(TARGET, "OAuth callback rejected");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("token_reason").as_deref(), Some("expired"));
}

// --- the client's bounds -------------------------------------------------------
//
// Every call the client makes to a provider is bounded — `CONNECT_TIMEOUT` to
// reach it, `CALL_TIMEOUT` for the whole call — and a call that ends without an
// answer, or with the provider saying it cannot answer now, is the provider's
// outage, `AuthError::Unavailable` (a `503`: the caller did nothing wrong);
// a provider that answers and refuses is `AuthError::Failed`. Both name the
// endpoint in their sentence. The providers below are local listeners;
// none of them answers the way a provider would. The connect bound is driven in
// the unit suite (`src/client.rs`), on the backend `new` builds with a resolver
// that never answers: no local listener can leave a handshake hanging.

/// Secrets a failed call must never repeat. The client secret and the code
/// travel in the exchange's body, the access token in a read's header, and the
/// query in the configured URL, where a deployment may keep an API key.
const CLIENT_SECRET: &str = "client-secret-never-quoted";
const CODE: &str = "authorization-code-never-quoted";
const ACCESS_TOKEN: &str = "access-token-never-quoted";
const QUERY_SECRET: &str = "query-key-never-quoted";

/// A client whose every endpoint is on `addr`, each URL carrying
/// [`QUERY_SECRET`] in its query.
fn client_for(addr: std::net::SocketAddr) -> OAuthClient {
    OAuthClient::new(nest_rs_oauth_client::OAuthClientConfig {
        client_secret: CLIENT_SECRET.into(),
        token_url: format!("http://{addr}/token?key={QUERY_SECRET}"),
        userinfo_url: format!("http://{addr}/userinfo?key={QUERY_SECRET}"),
        ..valid_config()
    })
    .expect("client builds")
}

/// The sentence a provider's outage is reported under — no answer, or an answer
/// saying it cannot answer now — checked to quote none of the call's secrets.
fn unavailable_sentence_of(error: &AuthError, extra: &[&str]) -> String {
    let AuthError::Unavailable { detail, .. } = error else {
        panic!("a provider that gave no answer is unavailable, not a failed sign-in: {error:?}");
    };
    secret_free(detail, extra)
}

/// The sentence a provider's refusal is reported under, checked the same way.
fn sentence_of(error: &AuthError, extra: &[&str]) -> String {
    let AuthError::Failed(sentence) = error else {
        panic!("a provider that answered and refused is a failed authentication: {error:?}");
    };
    secret_free(sentence, extra)
}

/// `sentence`, once it is shown to quote none of the call's secrets — `extra`
/// names the ones only this call carried.
fn secret_free(sentence: &str, extra: &[&str]) -> String {
    for secret in [CLIENT_SECRET, CODE, ACCESS_TOKEN, QUERY_SECRET]
        .iter()
        .chain(extra)
    {
        assert!(
            !sentence.contains(secret),
            "the sentence must not quote {secret:?}: {sentence}"
        );
    }
    sentence.to_owned()
}

/// A provider that accepts every connection and never answers — a process
/// wedged behind a healthy socket.
///
/// The clock stops the moment the first connection is accepted. The handshake
/// is complete then, on both sides, so what is left to wait is exactly the part
/// `CALL_TIMEOUT` governs — and on a stopped clock that wait costs the suite
/// nothing, while the connection itself was made in real time. It stays stopped,
/// so a test drives one call: a second connection made on a stopped clock races
/// the clock's jump to its connect bound.
async fn silent_provider() -> std::net::SocketAddr {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local listener");
    let addr = listener.local_addr().expect("a bound address");
    tokio::spawn(async move {
        let mut held = Vec::new();
        loop {
            let (socket, _) = listener.accept().await.expect("accept");
            if held.is_empty() {
                tokio::time::pause();
            }
            held.push(socket);
        }
    });
    addr
}

/// Await `call` for twice `bound` and no longer, so a client that stopped
/// bounding its calls fails here, naming the bound, instead of holding the
/// suite the way it held the callback.
async fn within_twice<T>(
    bound: std::time::Duration,
    call: impl std::future::Future<Output = T>,
) -> T {
    tokio::time::timeout(bound * 2, call)
        .await
        .unwrap_or_else(|_| panic!("no answer within twice the bound ({bound:?})"))
}

/// The exchange is the call a login cannot do without: a provider that accepts
/// it and never answers fails it at `CALL_TIMEOUT`, in a sentence naming the
/// token endpoint and the bound — and nothing the exchange carried.
#[tokio::test]
async fn an_exchange_the_provider_never_answers_fails_at_the_call_timeout() {
    let addr = silent_provider().await;
    let client = client_for(addr);
    let jwt = crate::jwt();
    let auth = client.authorize(&jwt, "acme").expect("authorize");
    let tx: Transaction = jwt
        .verify_handshake("oauth-tx", &auth.transaction)
        .expect("the transaction verifies");

    let sent = tokio::time::Instant::now();
    let Err(error) = within_twice(
        OAuthClient::CALL_TIMEOUT,
        client.exchange(&jwt, "acme", &auth.transaction, &tx.csrf, CODE),
    )
    .await
    else {
        panic!("a provider that never answers cannot complete the exchange");
    };
    let waited = sent.elapsed();

    assert!(
        waited >= OAuthClient::CALL_TIMEOUT,
        "failed at the bound, not before: {waited:?}"
    );
    assert_eq!(
        unavailable_sentence_of(&error, &[&tx.pkce]),
        format!(
            "the OAuth provider's token endpoint (http://{addr}/token) did not answer within {:?}",
            OAuthClient::CALL_TIMEOUT
        ),
    );
}

/// The read after the exchange — the userinfo endpoint — is bounded the same
/// way, and names itself.
#[tokio::test]
async fn a_userinfo_read_the_provider_never_answers_fails_at_the_call_timeout() {
    let addr = silent_provider().await;
    let client = client_for(addr);

    let sent = tokio::time::Instant::now();
    let error = within_twice(
        OAuthClient::CALL_TIMEOUT,
        client.userinfo::<serde_json::Value>(ACCESS_TOKEN),
    )
    .await
    .expect_err("a provider that never answers returns no profile");

    assert!(sent.elapsed() >= OAuthClient::CALL_TIMEOUT);
    assert_eq!(
        unavailable_sentence_of(&error, &[]),
        format!(
            "the OAuth provider's userinfo endpoint (http://{addr}/userinfo) did not answer within {:?}",
            OAuthClient::CALL_TIMEOUT
        ),
    );
}

/// …and so is a provider's own second read through `fetch`, GitHub's verified
/// emails being the one shipped, which no standard names.
#[tokio::test]
async fn a_fetch_the_provider_never_answers_fails_at_the_call_timeout() {
    let addr = silent_provider().await;
    let client = client_for(addr);

    let sent = tokio::time::Instant::now();
    let error = within_twice(
        OAuthClient::CALL_TIMEOUT,
        client.fetch::<serde_json::Value>(
            &format!("http://{addr}/user/emails?key={QUERY_SECRET}"),
            ACCESS_TOKEN,
        ),
    )
    .await
    .expect_err("a provider that never answers returns nothing");

    assert!(sent.elapsed() >= OAuthClient::CALL_TIMEOUT);
    assert_eq!(
        unavailable_sentence_of(&error, &[]),
        format!(
            "the OAuth provider's endpoint (http://{addr}/user/emails) did not answer within {:?}",
            OAuthClient::CALL_TIMEOUT
        ),
    );
}

/// The family the bounds belong to: every call that gets no usable answer names
/// its endpoint the same way. A refused connection says why, without the URL
/// reqwest would have quoted whole.
#[tokio::test]
async fn a_refused_connection_names_the_endpoint_and_the_cause() {
    let addr = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
        listener.local_addr().expect("a bound address")
    };
    let client = client_for(addr);

    let error = client
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("nothing listens there");
    let sentence = unavailable_sentence_of(&error, &[]);
    assert!(
        sentence.starts_with(&format!(
            "the OAuth provider's userinfo endpoint (http://{addr}/userinfo) could not be called: "
        )),
        "{sentence}"
    );
    assert!(
        !sentence.contains("for url"),
        "reqwest's own URL is taken out: {sentence}"
    );
}

/// A provider that answers, and refuses the token: the status is the cause,
/// after the endpoint that gave it.
#[tokio::test]
async fn a_read_answered_with_an_error_status_names_the_endpoint_and_the_status() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local listener");
    let addr = listener.local_addr().expect("a bound address");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        socket
            .write_all(
                b"HTTP/1.1 401 Unauthorized\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
            )
            .await
            .expect("answer");
    });
    let client = client_for(addr);

    let error = client
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a refused read returns no profile");
    assert_eq!(
        sentence_of(&error, &[]),
        format!(
            "the OAuth provider's userinfo endpoint (http://{addr}/userinfo) answered 401 Unauthorized"
        ),
    );
}

/// A local provider answering its first request with `status_line`, the
/// `Retry-After` `retry_after` when given, and an HTML page — the body a
/// provider's outage page has, which no §5.2 error parses from.
async fn provider_answering(
    status_line: &'static str,
    retry_after: Option<&'static str>,
) -> std::net::SocketAddr {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local listener");
    let addr = listener.local_addr().expect("a bound address");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        let body = "<html>down for maintenance</html>";
        let wait = retry_after
            .map(|wait| format!("retry-after: {wait}\r\n"))
            .unwrap_or_default();
        let answer = format!(
            "HTTP/1.1 {status_line}\r\ncontent-type: text/html\r\n{wait}content-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(answer.as_bytes()).await.expect("answer");
    });
    addr
}

/// Q12: a provider saying it cannot answer now — a `5xx`, or `429` — is its
/// outage, never the caller's failed sign-in, on the read and on the exchange
/// alike; its own `Retry-After` travels when it gave one in seconds. The
/// exchange's case was a `401`: `oauth2` drops the status, and an outage page
/// parses as no §5.2 error.
#[tokio::test]
async fn a_provider_saying_it_cannot_answer_now_is_unavailable_with_its_wait() {
    let addr = provider_answering("503 Service Unavailable", Some("7")).await;
    let error = client_for(addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("an outage returns no profile");
    assert_eq!(
        unavailable_sentence_of(&error, &[]),
        format!("the OAuth provider's userinfo endpoint (http://{addr}/userinfo) answered 503"),
    );
    assert_eq!(error.retry_after_secs(), Some(7));

    let addr = provider_answering("429 Too Many Requests", None).await;
    let error = client_for(addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a throttled read returns no profile");
    unavailable_sentence_of(&error, &[]);
    assert_eq!(error.retry_after_secs(), None, "no wait is invented");

    let addr = provider_answering("502 Bad Gateway", Some("Wed, 21 Oct 2015 07:28:00 GMT")).await;
    let client = client_for(addr);
    let jwt = crate::jwt();
    let auth = client.authorize(&jwt, "acme").expect("authorize");
    let tx: Transaction = jwt
        .verify_handshake("oauth-tx", &auth.transaction)
        .expect("the transaction verifies");
    let Err(error) = client
        .exchange(&jwt, "acme", &auth.transaction, &tx.csrf, CODE)
        .await
    else {
        panic!("an outage completes no exchange");
    };
    assert_eq!(
        unavailable_sentence_of(&error, &[&tx.pkce]),
        format!("the OAuth provider's token endpoint (http://{addr}/token) answered 502"),
    );
    assert_eq!(
        error.retry_after_secs(),
        None,
        "an HTTP-date is not converted against this clock"
    );
}

/// A profile that does not decode as the app's shape is reported by where and
/// what kind of value was found — never by the value, which is the caller's
/// profile or a token the provider put where the app expected something else.
#[tokio::test]
async fn a_read_whose_body_does_not_decode_quotes_none_of_it() {
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    const PROVIDER_TOKEN: &str = "ya29.provider-token-never-quoted";
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind a local listener");
    let addr = listener.local_addr().expect("a bound address");
    tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.expect("accept");
        let mut request = [0_u8; 4096];
        let _ = socket.read(&mut request).await;
        let body = format!(r#"{{"id":"{PROVIDER_TOKEN}"}}"#);
        let answer = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\
             connection: close\r\n\r\n{body}",
            body.len()
        );
        socket.write_all(answer.as_bytes()).await.expect("answer");
    });
    #[derive(Debug, serde::Deserialize)]
    #[allow(dead_code)]
    struct Profile {
        id: u64,
    }
    let error = client_for(addr)
        .userinfo::<Profile>(ACCESS_TOKEN)
        .await
        .expect_err("a profile of another shape is no profile");
    assert_eq!(
        sentence_of(&error, &[PROVIDER_TOKEN]),
        format!(
            "the OAuth provider's userinfo endpoint (http://{addr}/userinfo) answered a body \
             that does not parse: invalid type: a string, expected u64 at line 1 column 40"
        ),
    );
}

/// The bounds against each other: the connection has to fit inside the call,
/// or a slow handshake would be reported as a call that did not answer.
#[test]
fn the_connection_is_bounded_inside_the_call() {
    assert!(
        OAuthClient::CONNECT_TIMEOUT < OAuthClient::CALL_TIMEOUT,
        "{:?} vs {:?}",
        OAuthClient::CONNECT_TIMEOUT,
        OAuthClient::CALL_TIMEOUT,
    );
}
