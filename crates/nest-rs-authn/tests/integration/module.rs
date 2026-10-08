//! Covers `src/module.rs` — the `for_root` seam, executed: it queues a resolving
//! factory, so the pinned base and the env cascade meet only in the factory
//! phase a boot runs.

use std::sync::Arc;

use nest_rs_authn::{AuthnConfig, AuthnModule, AuthnSetup, JwtService};
use nest_rs_core::{App, module};
use serde::{Deserialize, Serialize};

/// An issuer distinct from every other fixture's, so an assertion below can
/// only pass by way of this call.
const PINNED_ISSUER: &str = "pinned-through-for-root";

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Claims {
    sub: String,
    // `exp` is the caller's to set; `JwtService` stamps only `iss` and `aud`.
    exp: u64,
}

fn pinned() -> AuthnSetup {
    AuthnModule::for_root(AuthnConfig {
        secret: Some("test-secret-padded-to-thirty-two-b".into()),
        issuer: Some(PINNED_ISSUER.into()),
        ..AuthnConfig::default()
    })
}

#[module(imports = [pinned()])]
struct PinnedAuthnHost;

#[tokio::test]
async fn for_root_pins_the_config_and_provides_a_service_built_from_it() {
    let app = App::builder()
        .module::<PinnedAuthnHost>()
        .build()
        .await
        .expect("the pinned-config module boots");

    let config: Arc<AuthnConfig> = app
        .container()
        .get()
        .expect("for_root registers the resolved AuthnConfig");
    assert_eq!(config.issuer.as_deref(), Some(PINNED_ISSUER));

    // A token the service mints proves the pinned base reached the constructor,
    // not merely the container.
    let jwt: Arc<JwtService> = app
        .container()
        .get()
        .expect("for_root queues the JwtService factory");

    let token = jwt
        .sign(&Claims {
            sub: "user-1".into(),
            exp: jwt.expiry(),
        })
        .expect("the pinned secret signs");
    let verified: Claims = jwt
        .verify(&token)
        .expect("and verifies through the same key");
    assert_eq!(verified.sub, "user-1");

    let payload = token.split('.').nth(1).expect("a JWT has a payload");
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        payload.as_bytes(),
    )
    .expect("base64url payload");
    let claims: serde_json::Value = serde_json::from_slice(&decoded).expect("json claims");
    assert_eq!(
        claims.get("iss").and_then(|v| v.as_str()),
        Some(PINNED_ISSUER),
        "the issuer the seam pinned is the one the service stamps",
    );
}
