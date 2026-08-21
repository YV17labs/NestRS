//! Covers `src/module.rs` — the `for_root` seam, executed.
//!
//! `AuthnModule::for_root` is the only in-code path a consumer has to pin a
//! `JwtConfig`, and it was the one seam in this crate with no test asserting
//! what a caller gets back: the discovery suite booted it, but only ever
//! read the *config* back through the audience check, never the service the
//! seam actually queues.
//!
//! What a compile could never show is what matters here: the seam queues a
//! *resolving* factory rather than the struct verbatim, so the pinned base and
//! the `NESTRS_AUTHN__*` cascade are reconciled during the builder's factory
//! phase — a phase only a boot runs.

use std::sync::Arc;

use nest_rs_authn::{AuthnModule, AuthnSetup, JwtConfig, JwtService};
use nest_rs_core::{App, module};
use serde::{Deserialize, Serialize};

/// An issuer distinct from every other fixture's, so an assertion below can
/// only pass by way of this call.
const PINNED_ISSUER: &str = "pinned-through-for-root";

#[derive(Serialize, Deserialize, Debug, PartialEq)]
struct Claims {
    sub: String,
    // `exp` belongs to the caller's claims type — `JwtService` stamps `iss` and
    // `aud` because verification requires them when configured, and leaves the
    // lifetime to whoever mints. `expiry()` is the value it hands over.
    exp: u64,
}

fn pinned() -> AuthnSetup {
    AuthnModule::for_root(JwtConfig {
        secret: Some("test-secret-padded-to-thirty-two-b".into()),
        issuer: Some(PINNED_ISSUER.into()),
        ..JwtConfig::default()
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

    let config: Arc<JwtConfig> = app
        .container()
        .get()
        .expect("for_root registers the resolved JwtConfig");
    assert_eq!(config.issuer.as_deref(), Some(PINNED_ISSUER));

    // The service is the factory output, not the config. Asserting on a token
    // it *mints* is what proves the pinned base reached the constructor rather
    // than merely being registered beside it — and round-tripping through the
    // same instance proves the key material survived the factory phase.
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
