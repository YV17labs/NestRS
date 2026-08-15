//! Covers `src/jwt/config.rs` — `JwtConfig::into_options`.

use std::time::Duration;

use nest_rs_authn::{AuthError, JwtConfig, JwtKey, JwtService};

// HS256 secrets must clear the 32-byte (256-bit) floor.
const STRONG_SECRET: &str = "this-is-a-32-byte-test-secret!!!";

#[test]
fn into_options_selects_hmac_from_secret() {
    let options = JwtConfig {
        secret: Some(STRONG_SECRET.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(matches!(options.key, JwtKey::Hmac(_)));
}

#[test]
fn into_options_rejects_a_short_hmac_secret() {
    assert!(matches!(
        JwtConfig {
            secret: Some("too-short".into()),
            ..Default::default()
        }
        .into_options(),
        Err(AuthError::Failed(_))
    ));
}

#[test]
fn into_options_selects_eddsa_from_key_pair() {
    let options = JwtConfig {
        private_key: Some(crate::DEV_PRIVATE_KEY.into()),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
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
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
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
        secret: Some(STRONG_SECRET.into()),
        leeway_secs: Some(45),
        audience: Some("api".into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert_eq!(options.leeway, Duration::from_secs(45));
    assert_eq!(options.audience.as_deref(), Some("api"));
}

/// A deployment that sets both an HS256 secret and an EdDSA pair gets the
/// asymmetric key, and the secret is dropped. Silently dropping a *credential*
/// is the kind of thing an operator finds out about during an incident, so the
/// warning names the variable it ignored — asserted here because a dropped
/// field is invisible to every other test.
#[test]
fn a_secret_beside_eddsa_keys_is_ignored_and_says_so() {
    let logs = nest_rs_testing::LogCapture::install();
    let options = JwtConfig {
        secret: Some(STRONG_SECRET.into()),
        private_key: Some(crate::DEV_PRIVATE_KEY.into()),
        public_key: Some(crate::DEV_PUBLIC_KEY.into()),
        ..Default::default()
    }
    .into_options()
    .expect("options");
    assert!(
        matches!(options.key, JwtKey::Pem { .. }),
        "the asymmetric pair wins",
    );

    let event = logs
        .find(
            "nest_rs::authn",
            "ignoring the shared secret in favour of EdDSA keys",
        )
        .into_iter()
        .next()
        .expect("dropping a configured credential is reported");
    assert_eq!(event.level, "warn");
    assert!(
        event
            .field("secret_var")
            .is_some_and(|v| v.ends_with("AUTHN__SECRET")),
        "and it names the variable whose value was dropped, built rather than \
         spelled so a renamed prefix still points at the right one: {event:?}",
    );
}
