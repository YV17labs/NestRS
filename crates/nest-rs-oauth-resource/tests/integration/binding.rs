//! Covers `src/binding.rs`: the token verifier `AuthnModule` builds beside
//! `OAuthResourceModule` is bound to the resource — its audience defaults to
//! the resource (RFC 8707), and a token's `iss` must be one of the advertised
//! authorization servers (RFC 9068 §4).

use std::sync::Arc;

use nest_rs_authn::{AuthnConfig, AuthnModule, JwtOptions, JwtService};
use nest_rs_config::Namespaced;
use nest_rs_core::module;
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule};
use nest_rs_testing::TestApp;
use serde::{Deserialize, Serialize};

use crate::boot_error;

const RESOURCE: &str = "https://api.example.com";
const TRUSTED: [&str; 2] = ["https://auth.example.com", "https://login.example.com"];
/// 32 bytes: HS256's floor.
const SECRET: &str = "binding-flow-secret-0123456789ab";

#[derive(Serialize, Deserialize)]
struct Claims {
    sub: String,
    exp: u64,
}

fn resource() -> OAuthResourceConfig {
    OAuthResourceConfig {
        resource: Some(RESOURCE.into()),
        authorization_servers: TRUSTED.iter().map(|&issuer| issuer.into()).collect(),
        ..OAuthResourceConfig::default()
    }
}

fn authn(audience: Option<&str>, issuer: Option<&str>) -> AuthnConfig {
    AuthnConfig {
        secret: Some(SECRET.into()),
        audience: audience.map(str::to_owned),
        issuer: issuer.map(str::to_owned),
        ..AuthnConfig::default()
    }
}

/// A token another holder of the key mints as `iss`, for `aud`.
fn minted(iss: Option<&str>, aud: &str) -> String {
    let mut options = JwtOptions::new(SECRET);
    options.issuer = iss.map(str::to_owned);
    let issuer = JwtService::new(options).expect("issuer");
    issuer
        .sign_for(
            aud,
            &Claims {
                sub: "client-1".into(),
                exp: issuer.expiry(),
            },
        )
        .expect("sign")
}

fn authn_var(key: &str) -> String {
    nest_rs_config::var_name(AuthnConfig::NAMESPACE, key)
}

fn resource_var(key: &str) -> String {
    nest_rs_config::var_name(OAuthResourceConfig::NAMESPACE, key)
}

/// No `AUDIENCE` to copy: the verifier requires the resource, and only a
/// token minted for it by an advertised authorization server verifies.
#[tokio::test]
async fn a_token_is_accepted_only_for_this_resource_from_an_advertised_server() {
    #[module(imports = [
        AuthnModule::for_root(authn(None, None)),
        OAuthResourceModule::for_root(resource()),
    ])]
    struct Bound;

    let app = TestApp::for_module::<Bound>()
        .await
        .expect("an unset audience is the resource");
    let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");

    for issuer in TRUSTED {
        jwt.verify::<Claims>(&minted(Some(issuer), RESOURCE))
            .await
            .expect("an advertised server's token for this resource verifies");
    }
    for (label, token) in [
        (
            "another resource",
            minted(Some(TRUSTED[0]), "https://billing.example.com"),
        ),
        (
            "a server this resource does not advertise",
            minted(Some("https://evil.example.com"), RESOURCE),
        ),
        ("no issuer", minted(None, RESOURCE)),
    ] {
        assert!(
            jwt.verify::<Claims>(&token).await.is_err(),
            "a token from {label} must be refused"
        );
    }
}

/// A set `ISSUER` narrows the advertised servers to itself.
#[tokio::test]
async fn a_set_issuer_among_the_servers_is_the_one_accepted() {
    #[module(imports = [
        AuthnModule::for_root(authn(None, Some(TRUSTED[1]))),
        OAuthResourceModule::for_root(resource()),
    ])]
    struct Narrowed;

    let app = TestApp::for_module::<Narrowed>().await.expect("boots");
    let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");
    jwt.verify::<Claims>(&minted(Some(TRUSTED[1]), RESOURCE))
        .await
        .expect("the set issuer verifies");
    assert!(
        jwt.verify::<Claims>(&minted(Some(TRUSTED[0]), RESOURCE))
            .await
            .is_err(),
        "another advertised server is refused once ISSUER names one"
    );
}

/// RFC 9068 §4 has the `iss` match an advertised authorization server: an
/// `ISSUER` outside them would refuse every token, so the boot fails naming
/// both settings and neither value.
#[tokio::test]
async fn an_issuer_outside_the_advertised_servers_fails_the_boot_naming_both() {
    #[module(imports = [
        AuthnModule::for_root(authn(None, Some("nestrs-demo-auth"))),
        OAuthResourceModule::for_root(resource()),
    ])]
    struct Disagreeing;

    let refused = boot_error::<Disagreeing>("an issuer outside the servers must not boot").await;
    assert!(refused.contains(&authn_var("ISSUER")), "{refused}");
    assert!(
        refused.contains(&resource_var("AUTHORIZATION_SERVERS")),
        "{refused}"
    );
    for value in ["nestrs-demo-auth", "auth.example.com"] {
        assert!(!refused.contains(value), "{refused}");
    }
}

/// A differing `AUDIENCE` is honoured and warned, never refused: some
/// authorization servers mint an opaque audience by policy.
#[tokio::test]
async fn a_differing_audience_is_the_one_required() {
    #[module(imports = [
        AuthnModule::for_root(authn(Some("opaque-api"), None)),
        OAuthResourceModule::for_root(resource()),
    ])]
    struct Opaque;

    let app = TestApp::for_module::<Opaque>().await.expect("boots");
    let jwt: Arc<JwtService> = app.container().get().expect("a JwtService");
    jwt.verify::<Claims>(&minted(Some(TRUSTED[0]), "opaque-api"))
        .await
        .expect("the set audience verifies");
    assert!(
        jwt.verify::<Claims>(&minted(Some(TRUSTED[0]), RESOURCE))
            .await
            .is_err()
    );
}

/// An audience-agnostic verifier cannot protect a resource: the opt-out
/// beside the module fails the boot naming both settings.
#[tokio::test]
async fn any_audience_beside_the_module_fails_the_boot() {
    #[module(imports = [
        AuthnModule::for_root(AuthnConfig {
            allow_any_audience: true,
            ..authn(None, None)
        }),
        OAuthResourceModule::for_root(resource()),
    ])]
    struct Agnostic;

    let refused = boot_error::<Agnostic>("an audience-agnostic resource must not boot").await;
    assert!(
        refused.contains(&authn_var("ALLOW_ANY_AUDIENCE")),
        "{refused}"
    );
    assert!(refused.contains(&resource_var("RESOURCE")), "{refused}");
}

/// The module protects tokens a verifier checks: without one it fails the
/// boot naming the import it needs.
#[tokio::test]
async fn the_module_without_a_verifier_fails_the_boot() {
    #[module(imports = [OAuthResourceModule::for_root(resource())])]
    struct Unverified;

    let refused = boot_error::<Unverified>("a resource with no verifier must not boot").await;
    assert!(
        refused.contains("import `nest_rs::authn::AuthnModule` alongside it"),
        "{refused}"
    );
}
