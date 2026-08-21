//! Covers `src/config.rs` — `OAuthClientConfig` validation.

use nest_rs_authn::AuthError;
use nest_rs_oauth_client::{OAuthClient, OAuthClientConfig};
use validator::Validate;

pub(crate) fn valid_config() -> OAuthClientConfig {
    OAuthClientConfig {
        client_id: "demo-client".into(),
        client_secret: "demo-secret".into(),
        auth_url: "https://provider.example/authorize".into(),
        token_url: "https://provider.example/token".into(),
        redirect_url: "https://app.example/callback".into(),
        userinfo_url: "https://provider.example/userinfo".into(),
        scopes: vec!["read:user".into()],
    }
}

#[test]
fn empty_config_fails_validation() {
    assert!(OAuthClientConfig::default().validate().is_err());
}

#[test]
fn valid_config_passes_validation() {
    valid_config().validate().expect("valid");
}

#[test]
fn missing_client_id_fails_validation() {
    let mut config = valid_config();
    config.client_id.clear();
    assert!(config.validate().is_err());
}

#[test]
fn empty_config_fails_at_client_construction() {
    assert!(matches!(
        OAuthClient::new(OAuthClientConfig::default()),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn valid_config_builds_client() {
    OAuthClient::new(valid_config()).expect("valid config");
}

#[test]
fn missing_client_id_fails_at_client_construction() {
    let mut config = valid_config();
    config.client_id.clear();
    assert!(OAuthClient::new(config).is_err());
}
