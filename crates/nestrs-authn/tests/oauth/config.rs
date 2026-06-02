//! Covers `src/oauth/config.rs` — `OAuth2Config` validation.

use nestrs_authn::{AuthError, OAuth2Client, OAuth2Config};
use validator::Validate;

pub(crate) fn valid_config() -> OAuth2Config {
    OAuth2Config {
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
    assert!(OAuth2Config::default().validate().is_err());
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
        OAuth2Client::new(OAuth2Config::default()),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn valid_config_builds_client() {
    OAuth2Client::new(valid_config()).expect("valid config");
}

#[test]
fn missing_client_id_fails_at_client_construction() {
    let mut config = valid_config();
    config.client_id.clear();
    assert!(OAuth2Client::new(config).is_err());
}
