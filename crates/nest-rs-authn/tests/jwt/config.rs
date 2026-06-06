//! Covers `src/jwt/config.rs` — `JwtConfig::into_options`.

use std::time::Duration;

use nest_rs_authn::{AuthError, JwtConfig, JwtKey, JwtService};

#[test]
fn into_options_selects_hmac_from_secret() {
    let options = JwtConfig {
        secret: Some("s".into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(matches!(options.key, JwtKey::Hmac(_)));
}

#[test]
fn into_options_selects_eddsa_from_key_pair() {
    let options = JwtConfig {
        private_key: Some(crate::common::DEV_PRIVATE_KEY.into()),
        public_key: Some(crate::common::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(matches!(options.key, JwtKey::Pem { .. }));
    JwtService::new(options).expect("EdDSA service builds");
}

#[test]
fn into_options_verify_only_from_public_key() {
    let options = JwtConfig {
        public_key: Some(crate::common::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(matches!(
        options.key,
        JwtKey::Pem {
            private_pem: None,
            ..
        }
    ));
}

#[test]
fn into_options_private_key_without_public_fails() {
    assert!(matches!(
        JwtConfig {
            private_key: Some("pem".into()),
            ..Default::default()
        }
        .into_options(),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn into_options_without_any_key_fails() {
    assert!(matches!(
        JwtConfig::default().into_options(),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn leeway_and_audience_are_applied_from_config() {
    let options = JwtConfig {
        secret: Some("leeway".into()),
        leeway_secs: Some(45),
        audience: Some("api".into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert_eq!(options.leeway, Duration::from_secs(45));
    assert_eq!(options.audience.as_deref(), Some("api"));
}
