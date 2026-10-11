//! Covers `src/client.rs` — the authorize URL, the exchange's checks, and every
//! call against a provider served over TLS by an in-process double whose
//! certificate a `TestAuthority` issued.

use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock};

use nest_rs_authn::{AuthError, JwtOptions, JwtService};
use nest_rs_config::{ClientTls, Config, ConfigService, Material, Namespaced, var_name};
use nest_rs_oauth_client::{OAuthClient, OAuthClientConfig, TARGET};
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

/// The authority every double's certificate chains to, and a client trusts
/// only through `TLS_CA_CERT`: the system's store does not hold it.
static AUTHORITY: LazyLock<TestAuthority> = LazyLock::new(TestAuthority::new);

static CERTIFICATE: LazyLock<TestCertificate> = LazyLock::new(|| AUTHORITY.server(&["127.0.0.1"]));

/// The client's TLS settings trusting [`AUTHORITY`].
fn trusting() -> ClientTls {
    ClientTls::new(
        Some(Material {
            bytes: AUTHORITY.pem().as_bytes().to_vec(),
            path: None,
        }),
        None,
    )
}

/// What a double does with each request it reads.
#[derive(Clone)]
enum Behaviour {
    /// Holds the connection and never answers; the clock stops at the first
    /// request read, so the `CALL_TIMEOUT` wait is free.
    Silent,
    /// Writes this raw HTTP/1.1 answer, then closes.
    Answer(String),
}

/// An OAuth provider over TLS on loopback, one request per connection, every
/// request it reads counted.
struct Provider {
    addr: SocketAddr,
    requests: Arc<AtomicUsize>,
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
        let acceptor = CERTIFICATE.acceptor(clients);
        let paused = Arc::new(AtomicBool::new(false));
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let (acceptor, behaviour, counted, paused) = (
                    acceptor.clone(),
                    behaviour.clone(),
                    Arc::clone(&counted),
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
                    }
                });
            }
        });
        Self { addr, requests }
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
}

/// A client whose every endpoint is on `addr`, each URL carrying
/// [`QUERY_SECRET`] in its query, trusting [`AUTHORITY`].
fn client_for(addr: SocketAddr) -> OAuthClient {
    OAuthClient::new(endpoints_on(addr, trusting())).expect("client builds")
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
    let exchange = |client: OAuthClient| async move {
        let jwt = crate::jwt();
        let auth = client.authorize(&jwt, "acme").expect("authorize");
        let tx: Transaction = jwt
            .verify_handshake("oauth-tx", &auth.transaction)
            .expect("the transaction verifies");
        client
            .exchange(&jwt, "acme", &auth.transaction, &tx.csrf, CODE)
            .await
    };

    let system_only =
        OAuthClient::new(endpoints_on(provider.addr, ClientTls::default())).expect("client builds");
    let Err(refused) = exchange(system_only).await else {
        panic!("a certificate no trusted authority signed must not be talked to");
    };
    assert!(
        unavailable_sentence_of(&refused, &[]).contains("could not be called"),
        "{refused:?}"
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
    let Ok(tokens) = exchange(OAuthClient::new(configured).expect("client builds")).await else {
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

    let error = client_for(provider.addr)
        .userinfo::<serde_json::Value>(ACCESS_TOKEN)
        .await
        .expect_err("a provider requiring a certificate refuses a client presenting none");
    assert!(
        unavailable_sentence_of(&error, &[]).contains("could not be called"),
        "{error:?}"
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

/// A provider endpoint over plain http is refused when the client is built,
/// naming the variable that sets it.
#[test]
fn a_provider_endpoint_over_plain_http_is_refused_at_construction() {
    let Err(AuthError::Failed(sentence)) = OAuthClient::new(OAuthClientConfig {
        token_url: "http://provider.example/token".into(),
        ..valid_config()
    }) else {
        panic!("a plaintext token endpoint must not build a client");
    };
    assert!(
        sentence.contains(&var_name(OAuthClientConfig::NAMESPACE, "TOKEN_URL"))
            && sentence.contains("RFC 6749 §3.2"),
        "{sentence}"
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
