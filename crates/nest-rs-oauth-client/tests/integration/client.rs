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
    #[expect(
        dead_code,
        reason = "the field exists for serde to require; the test reads the others"
    )]
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
    assert_eq!(tx.typ, "oauth_tx");
    assert_eq!(tx.provider, "acme");
}

/// The cookie is signed by the same service, key, `aud` and `iss` as an access
/// token: the media type (RFC 9068 §2.1) is the only thing telling them apart.
#[tokio::test]
async fn the_transaction_cookie_is_not_accepted_as_an_access_token() {
    let jwt = crate::jwt();
    let auth = client().authorize(&jwt, "acme").expect("authorize");

    assert!(
        matches!(
            jwt.verify::<Transaction>(&auth.transaction).await,
            Err(AuthError::InvalidToken)
        ),
        "a handshake token must not verify as an access token",
    );
}

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

    // `TokenSet` is not `Debug` (it carries tokens).
    let Err(err) = client()
        .exchange(&jwt, "acme", &auth.transaction, "not-the-csrf", "some-code")
        .await
    else {
        panic!("state mismatch is rejected");
    };
    assert!(matches!(err, AuthError::Failed(_)));

    let event = logs.expect_one(TARGET, "OAuth callback rejected");
    assert_eq!(event.level, "warn");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("csrf_state_mismatch")
    );
}

#[tokio::test]
async fn exchange_reports_a_transaction_cookie_that_does_not_verify() {
    // On this path no guard emits the `warn`, and `JwtService` logs its reason at `debug`.
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

/// Secrets a failed call must never repeat.
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

/// The sentence a provider's outage is reported under, checked to quote no secret.
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

/// `sentence`, once shown to quote none of the call's secrets (`extra`: this call's own).
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

/// A provider that accepts every connection and never answers.
///
/// The clock stops once the first connection is accepted, so the `CALL_TIMEOUT`
/// wait is free; a test drives one call, since a second connection would race the
/// clock's jump to its connect bound.
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

/// Await `call` for twice `bound` and no longer, so an unbounded client fails here.
async fn within_twice<T>(
    bound: std::time::Duration,
    call: impl std::future::Future<Output = T>,
) -> T {
    tokio::time::timeout(bound * 2, call)
        .await
        .unwrap_or_else(|_| panic!("no answer within twice the bound ({bound:?})"))
}

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

/// A refused connection says why, without the URL reqwest would have quoted whole.
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

/// A local provider answering its first request with `status_line`, an optional
/// `Retry-After`, and an HTML outage page, which no §5.2 error parses from.
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

/// A `5xx` or `429` is the provider's outage, never the caller's failed sign-in,
/// on the read and on the exchange; a delay-seconds `Retry-After` travels.
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

/// A profile that does not decode is reported by where and what kind, never by the value.
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
    #[expect(
        dead_code,
        reason = "the fields exist for serde to read; the test asserts the error, never a value"
    )]
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

#[test]
fn the_connection_is_bounded_inside_the_call() {
    assert!(
        OAuthClient::CONNECT_TIMEOUT < OAuthClient::CALL_TIMEOUT,
        "{:?} vs {:?}",
        OAuthClient::CONNECT_TIMEOUT,
        OAuthClient::CALL_TIMEOUT,
    );
}
