//! Covers `src/oauth/client.rs` — authorize URL and pre-network exchange checks.

use nest_rs_authn::{AuthError, JwtOptions, JwtService, OAuth2Client};
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

fn client() -> OAuth2Client {
    OAuth2Client::new(valid_config()).expect("client builds")
}

fn jwt() -> JwtService {
    JwtService::new(JwtOptions::new("test-secret-padded-to-thirty-two-b")).expect("HMAC JwtService")
}

#[test]
fn authorize_url_carries_client_scope_and_pkce_and_a_verifiable_transaction() {
    let jwt = jwt();
    let auth = client().authorize(&jwt, "acme").expect("authorize");

    assert!(auth.url.starts_with("https://provider.example/authorize?"));
    assert!(auth.url.contains("client_id=demo-client"));
    assert!(auth.url.contains("scope=read%3Auser"));
    assert!(auth.url.contains("code_challenge="));
    assert!(auth.url.contains("code_challenge_method=S256"));

    let tx: Transaction = jwt.verify(&auth.transaction).expect("transaction verifies");
    assert!(auth.url.contains(&format!("state={}", tx.csrf)));
    assert!(!tx.pkce.is_empty());
    // The transaction names what it is and which provider it belongs to, so a
    // shared cookie cannot carry it across flows.
    assert_eq!(tx.typ, "oauth_tx");
    assert_eq!(tx.provider, "acme");
}

#[tokio::test]
async fn exchange_rejects_a_state_that_does_not_match_the_transaction() {
    let logs = LogCapture::install();
    let jwt = jwt();
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
    let event = logs.expect_one("nest_rs::authn", "OAuth callback rejected");
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
        .exchange(&jwt(), "acme", &forged, "agreed-state", "some-code")
        .await
    else {
        panic!("a transaction signed by another key is rejected");
    };
    assert!(matches!(err, AuthError::InvalidSignature), "{err}");

    let event = logs.expect_one("nest_rs::authn", "OAuth callback rejected");
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
    let jwt = jwt();
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

    let event = logs.expect_one("nest_rs::authn", "OAuth callback rejected");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("token_reason").as_deref(), Some("expired"));
}
