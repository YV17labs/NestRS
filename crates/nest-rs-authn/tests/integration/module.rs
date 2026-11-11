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

#[derive(Serialize, Deserialize, Debug, PartialEq, nest_rs_authn::PrincipalIdentity)]
struct Claims {
    #[principal(actor)]
    sub: String,
    client_id: String,
}

/// The claims of `user-1`'s token, issued to the client `ci`.
fn user_1() -> Claims {
    Claims {
        sub: "user-1".into(),
        client_id: "ci".into(),
    }
}

fn pinned() -> AuthnSetup {
    AuthnModule::for_root(AuthnConfig {
        secret: Some("test-secret-padded-to-thirty-two-b".into()),
        issuer: Some(PINNED_ISSUER.into()),
        audience: Some(crate::AUDIENCE.into()),
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

    let token = jwt.sign(&user_1()).expect("the pinned secret signs");
    let verified: Claims = jwt
        .verify(&token)
        .await
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

/// What `OAuthResourceModule` resolves for the resource this app serves.
const RESOURCE: &str = "https://api.example";
const TRUSTED: [&str; 2] = ["https://as-1.example", "https://as-2.example"];
const SECRET: &str = "binding-secret-padded-to-thirty-2";

#[module(imports = [AuthnModule::for_root(AuthnConfig {
    secret: Some(SECRET.into()),
    ..AuthnConfig::default()
})])]
struct BoundAuthnHost;

/// A token a sibling issuer on the same key mints as `iss`, when it names one,
/// for `aud`.
fn minted(iss: Option<&str>, aud: &str) -> String {
    let mut claims = serde_json::json!({
        "sub": "user-1",
        "client_id": "ci",
        "aud": aud,
        "exp": jsonwebtoken::get_current_timestamp() + 3600,
    });
    if let Some(iss) = iss {
        claims["iss"] = iss.into();
    }
    let mut header = jsonwebtoken::Header::new(jsonwebtoken::Algorithm::HS256);
    header.typ = Some("at+jwt".into());
    jsonwebtoken::encode(
        &header,
        &claims,
        &jsonwebtoken::EncodingKey::from_secret(SECRET.as_bytes()),
    )
    .expect("encode")
}

/// RFC 9068 §4: with a resource binding in the container, the verifier the
/// module builds requires the resource as `aud` and one of the bound issuers
/// as `iss`, exactly.
#[tokio::test]
async fn a_resource_binding_is_the_audience_and_the_issuers_a_token_must_name() {
    let app = App::builder()
        .provide(nest_rs_authn::__private::ResourceBinding {
            audience: RESOURCE.into(),
            issuers: TRUSTED.iter().map(|&issuer| issuer.to_owned()).collect(),
        })
        .module::<BoundAuthnHost>()
        .build()
        .await
        .expect("the bound module boots");
    let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");

    for issuer in TRUSTED {
        let verified: Claims = jwt
            .verify(&minted(Some(issuer), RESOURCE))
            .await
            .expect("a trusted issuer's token for this resource verifies");
        assert_eq!(verified.sub, "user-1");
    }
    for (label, token) in [
        (
            "another resource",
            minted(Some(TRUSTED[0]), "https://billing.example"),
        ),
        (
            "an untrusted issuer",
            minted(Some("https://as-3.example"), RESOURCE),
        ),
        (
            "a trusted issuer but for a trailing slash",
            minted(Some("https://as-1.example/"), RESOURCE),
        ),
        ("no issuer", minted(None, RESOURCE)),
    ] {
        assert!(
            matches!(
                jwt.verify::<Claims>(&token).await,
                Err(nest_rs_authn::AuthError::InvalidToken)
            ),
            "a token from {label} must not verify"
        );
    }
}

/// A handshake is this service's own: a binding never widens what it meets,
/// so the client half of an app serving a resource keeps its transactions
/// with no issuer configured.
#[tokio::test]
async fn a_bound_service_still_verifies_its_own_handshakes() {
    let app = App::builder()
        .provide(nest_rs_authn::__private::ResourceBinding {
            audience: RESOURCE.into(),
            issuers: TRUSTED.iter().map(|&issuer| issuer.to_owned()).collect(),
        })
        .module::<BoundAuthnHost>()
        .build()
        .await
        .expect("boots");
    let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");

    #[derive(Serialize)]
    struct Transaction {
        sub: &'static str,
        exp: u64,
    }
    let transaction = jwt
        .sign_handshake(
            "oauth-tx",
            &Transaction {
                sub: "state",
                exp: jwt.expiry_in(600),
            },
        )
        .expect("sign");
    let back: serde_json::Value = jwt
        .verify_handshake("oauth-tx", &transaction)
        .expect("its own handshake verifies");
    assert_eq!(back["sub"], "state");
    assert!(
        jwt.verify::<Claims>(&transaction).await.is_err(),
        "and is never an access token"
    );
}

#[nest_rs_core::injectable]
struct SignsTokens {
    #[inject]
    _jwt: Arc<JwtService>,
}

#[module(providers = [SignsTokens])]
struct SignsWithoutAuthnModule;

/// A provider injecting `JwtService` in an app that does not import
/// `AuthnModule` is refused naming the import, never the kernel's generic
/// remedy.
#[tokio::test]
async fn a_jwt_service_no_import_provides_is_refused_naming_the_authn_module() {
    let Err(refused) = App::builder()
        .module::<SignsWithoutAuthnModule>()
        .build()
        .await
    else {
        panic!("no module provides the JwtService");
    };
    let missing = refused
        .downcast::<nest_rs_core::MissingDependencyError>()
        .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"));
    assert!(
        missing
            .remedy
            .is_some_and(|remedy| remedy.contains("nest_rs::authn::AuthnModule")),
        "{missing}"
    );
}

/// RFC 9068 §4 has a resource server check every token's `iss` and `aud`, and
/// §2.2 has every token minted carry both: a service with neither to check or
/// stamp does not boot, the refusal naming both settings.
#[tokio::test]
async fn a_service_naming_no_issuer_and_no_audience_does_not_boot() {
    let refused = match App::builder().module::<BoundAuthnHost>().build().await {
        Err(refused) => format!("{refused:#}"),
        Ok(app) => {
            let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");
            let minted = jwt.sign(&user_1());
            panic!(
                "booted, and minted {:?}",
                minted.map(|token| claims_of(&token))
            )
        }
    };
    for setting in ["ISSUER", "AUDIENCE"] {
        assert!(
            refused.contains(&nest_rs_config::var_name("authn", setting)),
            "{refused}"
        );
    }
}

/// A bound service reads what it checks off the resource, so it boots naming no
/// issuer of its own — and then has none to stamp: it mints no access token,
/// naming the setting, while its handshakes stand.
#[tokio::test]
async fn a_bound_service_naming_no_issuer_of_its_own_mints_no_access_token() {
    let app = App::builder()
        .provide(nest_rs_authn::__private::ResourceBinding {
            audience: RESOURCE.into(),
            issuers: TRUSTED.iter().map(|&issuer| issuer.to_owned()).collect(),
        })
        .module::<BoundAuthnHost>()
        .build()
        .await
        .expect("the bound module boots");
    let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");
    let claims = user_1();
    for (method, minted) in [
        ("sign", jwt.sign(&claims)),
        ("sign_for", jwt.sign_for("https://billing.example", &claims)),
    ] {
        let refused = match minted {
            Ok(token) => panic!("`{method}` minted {}", claims_of(&token)),
            Err(refused) => nest_rs_core::error_message(&refused),
        };
        assert!(
            refused.contains(&nest_rs_config::var_name("authn", "ISSUER")),
            "{method}: {refused}"
        );
    }
}

/// The claims `token` carries, read without verifying it.
fn claims_of(token: &str) -> serde_json::Value {
    let payload = token.split('.').nth(1).expect("a JWT has a payload");
    let decoded = base64::Engine::decode(
        &base64::engine::general_purpose::URL_SAFE_NO_PAD,
        payload.as_bytes(),
    )
    .expect("base64url payload");
    serde_json::from_slice(&decoded).expect("json claims")
}
