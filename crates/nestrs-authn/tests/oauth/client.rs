//! Covers `src/oauth/client.rs` — authorize URL and pre-network exchange checks.

use nestrs_authn::{AuthError, JwtOptions, JwtService, OAuth2Client};
use serde::Deserialize;

use super::config::valid_config;

#[derive(Debug, Deserialize)]
struct Transaction {
    csrf: String,
    pkce: String,
    #[allow(dead_code)]
    exp: u64,
}

fn client() -> OAuth2Client {
    OAuth2Client::new(valid_config()).expect("client builds")
}

fn jwt() -> JwtService {
    JwtService::new(JwtOptions::new("test-secret")).expect("HMAC JwtService")
}

#[test]
fn authorize_url_carries_client_scope_and_pkce_and_a_verifiable_transaction() {
    let jwt = jwt();
    let auth = client().authorize(&jwt).expect("authorize");

    assert!(auth.url.starts_with("https://provider.example/authorize?"));
    assert!(auth.url.contains("client_id=demo-client"));
    assert!(auth.url.contains("scope=read%3Auser"));
    assert!(auth.url.contains("code_challenge="));
    assert!(auth.url.contains("code_challenge_method=S256"));

    let tx: Transaction = jwt.verify(&auth.transaction).expect("transaction verifies");
    assert!(auth.url.contains(&format!("state={}", tx.csrf)));
    assert!(!tx.pkce.is_empty());
}

#[tokio::test]
async fn exchange_rejects_a_state_that_does_not_match_the_transaction() {
    let jwt = jwt();
    let auth = client().authorize(&jwt).expect("authorize");

    let err = client()
        .exchange(&jwt, &auth.transaction, "not-the-csrf", "some-code")
        .await
        .expect_err("state mismatch is rejected");
    assert!(matches!(err, AuthError::Failed(_)));
}
