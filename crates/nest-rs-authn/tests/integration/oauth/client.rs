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
