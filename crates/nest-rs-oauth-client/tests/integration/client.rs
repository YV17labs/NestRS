//! Covers `src/client.rs` — the authorize URL, the exchange's checks, and every
//! call against a provider served over TLS by an in-process double whose
//! certificate a `TestAuthority` issued.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use nest_rs_authn::{AuthError, JwtOptions, JwtService};
use nest_rs_config::{ClientTls, Config, ConfigService, Namespaced, var_name};
use nest_rs_core::{App, module};
use nest_rs_oauth_client::{OAuthClient, OAuthClientConfig, OAuthClientModule, TARGET, TokenSet};
use nest_rs_testing::{LogCapture, TestAuthority, TestCertificate};
use serde::Deserialize;
use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
use tokio::net::TcpListener;

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
    assert!(matches!(err, AuthError::Failed { .. }));

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

/// The authority every double's certificate chains to, and a client trusts
/// only through `TLS_CA_CERT`: the system's store does not hold it.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// What a double does with each request it reads.
#[derive(Clone)]
enum Behaviour {
    /// Holds the connection and never answers; the clock stops at the first
    /// request read, so the `CALL_TIMEOUT` wait is free.
    Silent,
    /// Writes this raw HTTP/1.1 answer, then closes.
    Answer(String),
    /// Streams a `200` of this many bytes, chunked and with no
    /// `Content-Length`, counting the bytes the client took.
    Chunked(usize),
}

/// An OAuth provider over TLS on loopback, one request per connection, every
/// request it reads counted.
struct Provider {
    addr: SocketAddr,
    requests: Arc<AtomicUsize>,
    written: Arc<AtomicUsize>,
    ended: Arc<AtomicBool>,
}

impl Provider {
    async fn serving(behaviour: Behaviour) -> Self {
        Self::serving_to(behaviour, None).await
    }

    /// A provider requiring a client certificate `clients` issued when it names
    /// an authority.
    async fn serving_to(behaviour: Behaviour, clients: Option<&TestAuthority>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
        let addr = listener.local_addr().expect("its address");
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        let written = Arc::new(AtomicUsize::new(0));
        let taken = Arc::clone(&written);
        let ended = Arc::new(AtomicBool::new(false));
        let stopped = Arc::clone(&ended);
        let acceptor = CERTIFICATE.acceptor(clients);
        let paused = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, behaviour, counted, taken, stopped, paused) = (
                    acceptor.clone(),
                    behaviour.clone(),
                    Arc::clone(&counted),
                    Arc::clone(&taken),
                    Arc::clone(&stopped),
                    Arc::clone(&paused),
                );
                tokio::spawn(async move {
                    let Ok(mut tls) = acceptor.accept(stream).await else {
                        return;
                    };
                    let mut head = Vec::new();
                    let mut buf = [0_u8; 4096];
                    while !head.windows(4).any(|window| window == b"\r\n\r\n") {
                        match tls.read(&mut buf).await {
                            Ok(0) | Err(_) => return,
                            Ok(read) => head.extend_from_slice(&buf[..read]),
                        }
                    }
                    counted.fetch_add(1, Ordering::SeqCst);
                    match behaviour {
                        Behaviour::Silent => {
                            if !paused.swap(true, Ordering::SeqCst) {
                                tokio::time::pause();
                            }
                            std::future::pending::<()>().await;
                        }
                        Behaviour::Answer(answer) => {
                            let _ = tls.write_all(answer.as_bytes()).await;
                            let _ = tls.shutdown().await;
                        }
                        Behaviour::Chunked(size) => {
                            stream_chunked(&mut tls, size, &taken).await;
                            stopped.store(true, Ordering::SeqCst);
                        }
                    }
                });
            }
        });
        Self {
            addr,
            requests,
            written,
            ended,
        }
    }

    /// A provider answering every request with `status_line`, `headers` and
    /// `body`.
    async fn answering(status_line: &str, headers: &[(&str, String)], body: &str) -> Self {
        let mut answer = format!(
            "HTTP/1.1 {status_line}\r\ncontent-length: {}\r\nconnection: close\r\n",
            body.len()
        );
        for (name, value) in headers {
            answer.push_str(&format!("{name}: {value}\r\n"));
        }
        answer.push_str("\r\n");
        answer.push_str(body);
        Self::serving(Behaviour::Answer(answer)).await
    }

    fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// The body bytes a [`Behaviour::Chunked`] double got written.
    fn written(&self) -> usize {
        self.written.load(Ordering::SeqCst)
    }

    /// Whether a [`Behaviour::Chunked`] double stopped writing, the whole body
    /// written or the connection gone.
    fn ended(&self) -> bool {
        self.ended.load(Ordering::SeqCst)
    }
}

/// A `200` of `size` body bytes, chunked, each chunk counted in `taken` once
/// written.
async fn stream_chunked(
    tls: &mut (impl tokio::io::AsyncWrite + Unpin),
    size: usize,
    taken: &AtomicUsize,
) {
    let head = "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\n\
                transfer-encoding: chunked\r\nconnection: close\r\n\r\n";
    if tls.write_all(head.as_bytes()).await.is_err() {
        return;
    }
    let chunk = [b' '; 16 * 1024];
    let mut left = size;
    while left > 0 {
        let part = &chunk[..left.min(chunk.len())];
        let framed = [format!("{:x}\r\n", part.len()).as_bytes(), part, b"\r\n"].concat();
        if tls.write_all(&framed).await.is_err() || tls.flush().await.is_err() {
            return;
        }
        taken.fetch_add(part.len(), Ordering::SeqCst);
        left -= part.len();
    }
    let _ = tls.write_all(b"0\r\n\r\n").await;
    let _ = tls.shutdown().await;
}

/// A client whose every endpoint is on `addr`, each URL carrying
/// [`QUERY_SECRET`] in its query, trusting [`AUTHORITY`].
fn client_for(addr: SocketAddr) -> OAuthClient {
    OAuthClient::new(endpoints_on(addr, AUTHORITY.client_tls())).expect("client builds")
}

fn endpoints_on(addr: SocketAddr, tls: ClientTls) -> OAuthClientConfig {
    OAuthClientConfig {
        client_secret: CLIENT_SECRET.into(),
        token_url: format!("https://{addr}/token?key={QUERY_SECRET}"),
        userinfo_url: format!("https://{addr}/userinfo?key={QUERY_SECRET}"),
        tls,
        ..valid_config()
    }
}

/// The sentence a provider's outage is reported under — the detail and its
/// cause beneath the constant — checked to quote no secret anywhere in the
/// line.
fn unavailable_sentence_of(error: &AuthError, extra: &[&str]) -> String {
    let AuthError::Unavailable { .. } = error else {
        panic!("a provider that gave no answer is unavailable, not a failed sign-in: {error:?}");
    };
    said_beneath(error, extra)
}

/// The sentence a provider's refusal is reported under, checked the same way.
fn sentence_of(error: &AuthError, extra: &[&str]) -> String {
    let AuthError::Failed { .. } = error else {
        panic!("a provider that answered and refused is a failed authentication: {error:?}");
    };
    said_beneath(error, extra)
}

/// What the operator's line says below `error`'s constant, once the whole
/// line is shown to quote none of the call's secrets.
fn said_beneath(error: &AuthError, extra: &[&str]) -> String {
    secret_free(&nest_rs_core::error_message(error), extra);
    let detail = std::error::Error::source(error).expect("the detail beneath the constant");
    nest_rs_core::error_message(detail)
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
    let provider = Provider::serving(Behaviour::Silent).await;
    let addr = provider.addr;
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
            "the OAuth provider's token endpoint (https://{addr}/token) did not answer within {:?}",
            OAuthClient::CALL_TIMEOUT
        ),
    );
}

#[tokio::test]
async fn a_userinfo_read_the_provider_never_answers_fails_at_the_call_timeout() {
    let provider = Provider::serving(Behaviour::Silent).await;
    let addr = provider.addr;
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
            "the OAuth provider's userinfo endpoint (https://{addr}/userinfo) did not answer within {:?}",
            OAuthClient::CALL_TIMEOUT
        ),
    );
}

#[tokio::test]
async fn a_fetch_the_provider_never_answers_fails_at_the_call_timeout() {
    let provider = Provider::serving(Behaviour::Silent).await;
    let addr = provider.addr;
    let client = client_for(addr);

    let sent = tokio::time::Instant::now();
    let error = within_twice(
        OAuthClient::CALL_TIMEOUT,
        client.fetch::<serde_json::Value>(
            &format!("https://{addr}/user/emails?key={QUERY_SECRET}"),
            ACCESS_TOKEN,
        ),
    )
    .await
    .expect_err("a provider that never answers returns nothing");

    assert!(sent.elapsed() >= OAuthClient::CALL_TIMEOUT);
    assert_eq!(
        unavailable_sentence_of(&error, &[]),
        format!(
            "the OAuth provider's endpoint (https://{addr}/user/emails) did not answer within {:?}",
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
            "the OAuth provider's userinfo endpoint (https://{addr}/userinfo) could not be called: "
        )),
        "{sentence}"
    );
    assert!(
        !sentence.contains("for url"),
        "reqwest's own URL is taken out: {sentence}"
    );
    let cause = std::error::Error::source(&error)
        .and_then(std::error::Error::source)
        .and_then(|cause| cause.downcast_ref::<reqwest::Error>())
        .expect("reqwest's error is the cause beneath the detail");
    assert!(cause.is_connect() && cause.url().is_none(), "{cause:?}");
}

#[tokio::test]
async fn a_read_answered_with_an_error_status_names_the_endpoint_and_the_status() {
    let provider = Provider::answering("401 Unauthorized", &[], "").await;
    let addr = provider.addr;

    let error = client_for(addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a refused read returns no profile");
    assert_eq!(
        sentence_of(&error, &[]),
        format!(
            "the OAuth provider's userinfo endpoint (https://{addr}/userinfo) answered 401 Unauthorized"
        ),
    );
}

/// A local provider answering every request with `status_line`, an optional
/// `Retry-After`, and an HTML outage page, which no §5.2 error parses from.
async fn provider_answering(status_line: &str, retry_after: Option<&str>) -> Provider {
    let mut headers = vec![("content-type", "text/html".to_owned())];
    if let Some(wait) = retry_after {
        headers.push(("retry-after", wait.to_owned()));
    }
    Provider::answering(status_line, &headers, "<html>down for maintenance</html>").await
}

/// A `5xx` or `429` is the provider's outage, never the caller's failed sign-in,
/// on the read and on the exchange; a delay-seconds `Retry-After` travels.
#[tokio::test]
async fn a_provider_saying_it_cannot_answer_now_is_unavailable_with_its_wait() {
    let provider = provider_answering("503 Service Unavailable", Some("7")).await;
    let addr = provider.addr;
    let error = client_for(addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("an outage returns no profile");
    assert_eq!(
        unavailable_sentence_of(&error, &[]),
        format!("the OAuth provider's userinfo endpoint (https://{addr}/userinfo) answered 503"),
    );
    assert_eq!(error.retry_after_secs(), Some(7));

    let provider = provider_answering("429 Too Many Requests", None).await;
    let error = client_for(provider.addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a throttled read returns no profile");
    unavailable_sentence_of(&error, &[]);
    assert_eq!(error.retry_after_secs(), None, "no wait is invented");

    let provider =
        provider_answering("502 Bad Gateway", Some("Wed, 21 Oct 2015 07:28:00 GMT")).await;
    let addr = provider.addr;
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
        format!("the OAuth provider's token endpoint (https://{addr}/token) answered 502"),
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
    const PROVIDER_TOKEN: &str = "ya29.provider-token-never-quoted";
    let provider = Provider::answering(
        "200 OK",
        &[("content-type", "application/json".to_owned())],
        &format!(r#"{{"id":"{PROVIDER_TOKEN}"}}"#),
    )
    .await;
    let addr = provider.addr;
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
            "the OAuth provider's userinfo endpoint (https://{addr}/userinfo) answered a body \
             that does not parse: invalid type: a string, expected u64 at line 1 column 40"
        ),
    );
    debug_free(&error, &[PROVIDER_TOKEN]);
}

/// A token answer that does not decode is the provider's refusal, said as a
/// read's undecodable body is, without a byte of the answer: neither in the
/// operator's line nor in the error's `Debug`, which `oauth2`'s own error
/// would fill with the whole answer.
#[tokio::test]
async fn a_token_answer_that_does_not_decode_quotes_none_of_it() {
    const ISSUED: &str = "ya29.issued-access-token-never-quoted";
    const PROVIDER_TOKEN: &str = "ya29.provider-token-never-quoted";
    let provider = Provider::answering(
        "200 OK",
        &[("content-type", "application/json".to_owned())],
        &format!(
            r#"{{"access_token":"{ISSUED}","token_type":"bearer","expires_in":"{PROVIDER_TOKEN}"}}"#
        ),
    )
    .await;
    let Err(error) = exchange(&client_for(provider.addr)).await else {
        panic!("an answer of another shape is no token");
    };
    assert_eq!(
        sentence_of(&error, &[ISSUED, PROVIDER_TOKEN]),
        format!(
            "the OAuth provider's token endpoint (https://{}/token) answered a body that does \
             not parse: invalid type: a string, expected u64 at line 1 column 125",
            provider.addr
        ),
    );
    debug_free(&error, &[ISSUED, PROVIDER_TOKEN]);
}

/// `error`'s `Debug` — what a `{:?}` or an `expect` prints — once shown to
/// hold none of `secrets`, as text or as the bytes a `Vec<u8>` prints.
fn debug_free(error: &AuthError, secrets: &[&str]) {
    let debug = format!("{error:?}");
    for secret in secrets {
        let bytes = format!("{:?}", secret.as_bytes());
        assert!(
            !debug.contains(secret) && !debug.contains(bytes.trim_matches(['[', ']'])),
            "the Debug must not hold {secret:?}: {debug}"
        );
    }
}

#[module(imports = [OAuthClientModule::for_root(None)])]
struct OAuthClientHost;

/// The client [`OAuthClientModule`] builds over `config`, seeded in place of
/// the one `<PREFIX>_OAUTH__CLIENT__*` would resolve.
async fn module_built(config: OAuthClientConfig) -> Arc<OAuthClient> {
    let app = App::builder()
        .provide(config)
        .module::<OAuthClientHost>()
        .build()
        .await
        .expect("the module boots over the seeded config");
    app.container()
        .get()
        .expect("the module provides the client")
}

/// A full exchange through `client`, its transaction minted by `client` itself.
async fn exchange(client: &OAuthClient) -> Result<TokenSet, AuthError> {
    let jwt = crate::jwt();
    let auth = client.authorize(&jwt, "acme").expect("authorize");
    let tx: Transaction = jwt
        .verify_handshake("oauth-tx", &auth.transaction)
        .expect("the transaction verifies");
    client
        .exchange(&jwt, "acme", &auth.transaction, &tx.csrf, CODE)
        .await
}

/// A provider whose certificate a private authority signed is trusted only
/// once `<PREFIX>_OAUTH__CLIENT__TLS_CA_CERT` names that authority; without it
/// the handshake fails, and no request reaches the provider.
#[tokio::test]
async fn a_token_endpoint_behind_a_private_authority_is_trusted_only_with_its_authority() {
    let provider = Provider::answering(
        "200 OK",
        &[("content-type", "application/json".to_owned())],
        r#"{"access_token":"issued","token_type":"bearer"}"#,
    )
    .await;

    let system_only = module_built(endpoints_on(provider.addr, ClientTls::default())).await;
    let Err(refused) = exchange(&system_only).await else {
        panic!("a certificate no trusted authority signed must not be talked to");
    };
    let sentence = unavailable_sentence_of(&refused, &[]);
    assert!(
        sentence.starts_with(&format!(
            "the OAuth provider's token endpoint (https://{}/token) could not be called: the \
             OAuth provider's certificate does not chain to an authority this app trusts",
            provider.addr
        )) && sentence.contains(&var_name(OAuthClientConfig::NAMESPACE, "TLS_CA_CERT")),
        "the refusal says what to set: {sentence}"
    );
    assert_eq!(
        provider.requests(),
        0,
        "no request crossed the failed handshake"
    );

    let env = ConfigService::with_vars(
        OAuthClientConfig::NAMESPACE,
        [("TLS_CA_CERT", AUTHORITY.pem())],
    );
    let configured =
        OAuthClientConfig::from_env(&env, endpoints_on(provider.addr, ClientTls::default()))
            .expect("the authority reads");
    let Ok(tokens) = exchange(&OAuthClient::new(configured).expect("client builds")).await else {
        panic!("the authority the deployment named is trusted");
    };
    assert_eq!(tokens.access_token, "issued");
    assert_eq!(provider.requests(), 1);
}

/// A provider requiring a client certificate is handed the one the deployment
/// sets under `<PREFIX>_OAUTH__CLIENT__TLS_CERT` and `TLS_KEY`; without one,
/// no request reaches it.
#[tokio::test]
async fn a_provider_requiring_a_client_certificate_is_handed_the_configured_one() {
    let clients = TestAuthority::new();
    let issued = clients.client("nestrs-test-client");
    let provider = Provider::serving_to(
        Behaviour::Answer(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: 2\r\n\
             connection: close\r\n\r\n{}"
                .to_owned(),
        ),
        Some(&clients),
    )
    .await;

    let error = module_built(endpoints_on(provider.addr, AUTHORITY.client_tls()))
        .await
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a provider requiring a certificate refuses a client presenting none");
    let sentence = unavailable_sentence_of(&error, &[]);
    assert!(
        sentence.contains("could not be called: the OAuth provider refused the handshake")
            && sentence.contains(&var_name(OAuthClientConfig::NAMESPACE, "TLS_CERT")),
        "the refusal says what to set: {sentence}"
    );
    assert_eq!(
        provider.requests(),
        0,
        "no request crossed the refused handshake"
    );

    let env = ConfigService::with_vars(
        OAuthClientConfig::NAMESPACE,
        [
            ("TLS_CA_CERT", AUTHORITY.pem()),
            ("TLS_CERT", issued.cert.as_str()),
            ("TLS_KEY", issued.key.as_str()),
        ],
    );
    let presenting =
        OAuthClientConfig::from_env(&env, endpoints_on(provider.addr, ClientTls::default()))
            .expect("the deployment's certificate over the trusted authority");
    OAuthClient::new(presenting)
        .expect("client builds")
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect("the profile is read presenting the certificate");
    assert_eq!(provider.requests(), 1);
}

/// A client its caller composed — a social provider's, whose URLs are
/// constants and whose TLS no variable sets — reads none of
/// `<PREFIX>_OAUTH__CLIENT__*`: a refused handshake keeps rustls's reason and
/// names none of them.
#[tokio::test]
async fn a_composed_client_refused_a_handshake_names_no_variable_it_never_reads() {
    let provider = Provider::answering(
        "200 OK",
        &[("content-type", "application/json".to_owned())],
        r#"{"access_token":"issued","token_type":"bearer"}"#,
    )
    .await;
    let composed =
        OAuthClient::new(endpoints_on(provider.addr, ClientTls::default())).expect("client builds");

    let Err(refused) = exchange(&composed).await else {
        panic!("a certificate no trusted authority signed must not be talked to");
    };
    let sentence = unavailable_sentence_of(&refused, &[]);
    assert!(
        sentence.starts_with(&format!(
            "the OAuth provider's token endpoint (https://{}/token) could not be called: ",
            provider.addr
        )) && sentence.contains("UnknownIssuer")
            && !sentence.contains(&var_name(OAuthClientConfig::NAMESPACE, "")),
        "the refusal keeps rustls's reason and names no variable: {sentence}"
    );
    assert_eq!(
        provider.requests(),
        0,
        "no request crossed the failed handshake"
    );
}

/// A redirect is an answer, never followed: a provider's `3xx` could send the
/// bearer token anywhere.
#[tokio::test]
async fn a_redirect_is_answered_and_never_followed() {
    let provider = Provider::answering(
        "302 Found",
        &[("location", "https://127.0.0.1:9/elsewhere".to_owned())],
        "",
    )
    .await;
    let addr = provider.addr;

    let error = client_for(addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a redirect returns no profile");
    assert_eq!(
        sentence_of(&error, &[]),
        format!(
            "the OAuth provider's userinfo endpoint (https://{addr}/userinfo) answered 302 Found"
        ),
    );
    assert_eq!(provider.requests(), 1, "the redirect was not followed");
}

/// `fetch` carries the bearer token, so a URL that is not `https` is refused
/// before a connection is opened.
#[tokio::test]
async fn a_fetch_over_plain_http_is_refused_before_anything_is_sent() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("a port");
    let addr = listener.local_addr().expect("its address");
    let accepted = Arc::new(AtomicUsize::new(0));
    let counted = Arc::clone(&accepted);
    tokio::spawn(async move {
        while let Ok((_held, _)) = listener.accept().await {
            counted.fetch_add(1, Ordering::SeqCst);
        }
    });

    let error = client_for(addr)
        .fetch::<serde_json::Value>(
            &format!("http://u:{QUERY_SECRET}@{addr}/user/emails"),
            ACCESS_TOKEN,
        )
        .await
        .expect_err("plain http carries no bearer token");
    assert_eq!(
        sentence_of(&error, &[]),
        format!(
            "the OAuth provider's endpoint (http://{addr}/user/emails) is not an https URL, and \
             RFC 6750 §5.3 sends a bearer token over TLS alone"
        ),
    );
    tokio::task::yield_now().await;
    assert_eq!(accepted.load(Ordering::SeqCst), 0, "nothing was dialled");
}

fn plaintext() -> OAuthClientConfig {
    OAuthClientConfig {
        token_url: "http://provider.example/token".into(),
        ..valid_config()
    }
}

#[module(imports = [OAuthClientModule::for_root(plaintext())])]
struct PlaintextHost;

/// A provider endpoint over plain http is refused when the client is built:
/// a client its caller composed names the field, and the module's names the
/// variable that sets it beside the field.
#[tokio::test]
async fn a_provider_endpoint_over_plain_http_is_refused_at_construction() {
    let Err(sentence @ AuthError::Failed { .. }) = OAuthClient::new(plaintext()) else {
        panic!("a plaintext token endpoint must not build a client");
    };
    let sentence = nest_rs_core::error_message(&sentence);
    assert!(
        sentence.contains("`OAuthClientConfig::token_url`, must be an https URL")
            && sentence.contains("RFC 6749 §3.2")
            && !sentence.contains(&var_name(OAuthClientConfig::NAMESPACE, "")),
        "{sentence}"
    );

    let Err(refused) = App::builder().module::<PlaintextHost>().build().await else {
        panic!("a plaintext token endpoint must not boot");
    };
    let sentence = nest_rs_core::error_message(refused.as_ref());
    assert!(
        sentence.contains(&format!(
            "{}, or `OAuthClientConfig::token_url` in code, must be an https URL",
            nest_rs_config::spellings(OAuthClientConfig::NAMESPACE, "TOKEN_URL")
        )),
        "{sentence}"
    );
}

/// The refusal of an answer past [`OAuthClient::ANSWER_MAX_BYTES`] from the
/// endpoint `shown`, its body quoted nowhere.
fn assert_oversized(error: &AuthError, shown: &str) {
    let sentence = unavailable_sentence_of(error, &["xxxx", "    "]);
    assert_eq!(
        sentence,
        format!(
            "{shown} answered more than {} bytes, the most an OAuth answer is read to",
            OAuthClient::ANSWER_MAX_BYTES
        ),
    );
    assert_eq!(error.retry_after_secs(), None, "no wait is invented");
}

/// An answer a byte past the cap, its `Content-Length` saying so.
fn oversized_document() -> String {
    format!(
        r#"{{"name":"{}"}}"#,
        "x".repeat(OAuthClient::ANSWER_MAX_BYTES)
    )
}

#[tokio::test]
async fn a_userinfo_answer_declared_past_the_cap_is_refused_unread() {
    let provider = Provider::answering(
        "200 OK",
        &[("content-type", "application/json".to_owned())],
        &oversized_document(),
    )
    .await;
    let addr = provider.addr;

    let error = client_for(addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a profile past the cap is no profile");
    assert_oversized(
        &error,
        &format!("the OAuth provider's userinfo endpoint (https://{addr}/userinfo)"),
    );
}

/// With no `Content-Length`, the read stops at the cap: the double could not
/// write the rest.
#[tokio::test]
async fn a_streamed_answer_is_cut_at_the_cap_and_never_read_whole() {
    const STREAMED: usize = 32 * 1024 * 1024;
    let provider = Provider::serving(Behaviour::Chunked(STREAMED)).await;
    let addr = provider.addr;

    let error = client_for(addr)
        .fetch::<serde_json::Value>(&format!("https://{addr}/user/emails"), ACCESS_TOKEN)
        .await
        .expect_err("an endless answer is no answer");
    assert_oversized(
        &error,
        &format!("the OAuth provider's endpoint (https://{addr}/user/emails)"),
    );
    nest_rs_testing::wait_until(std::time::Duration::from_secs(10), || provider.ended()).await;
    assert!(
        provider.written() < STREAMED / 4,
        "the client stopped near the cap, not at the end: {} of {STREAMED} bytes written",
        provider.written()
    );
}

#[tokio::test]
async fn a_token_answer_past_the_cap_fails_the_exchange_the_same_way() {
    let provider = Provider::answering(
        "200 OK",
        &[("content-type", "application/json".to_owned())],
        &oversized_document(),
    )
    .await;
    let addr = provider.addr;
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
        panic!("a token answer past the cap completes no exchange");
    };
    assert_oversized(
        &error,
        &format!("the OAuth provider's token endpoint (https://{addr}/token)"),
    );
}

/// A profile of a few KiB, the size every standard document has, still reads.
#[tokio::test]
async fn a_userinfo_document_of_a_few_kib_reads() {
    let bio = "b".repeat(2 * 1024);
    let provider = Provider::answering(
        "200 OK",
        &[("content-type", "application/json".to_owned())],
        &format!(r#"{{"bio":"{bio}"}}"#),
    )
    .await;

    let profile = client_for(provider.addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect("a document under the cap reads");
    assert_eq!(profile["bio"], bio.as_str());
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
