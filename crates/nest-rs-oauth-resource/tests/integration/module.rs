//! Covers `src/module.rs`: what importing the module adds beyond its document
//! and challenge — the MCP endpoint answers the `Host` its resource names —
//! and the scope set an import declares, alone or beside a pinned config,
//! which the document advertises when the deployment names none.

use nest_rs_authn::{AuthnConfig, AuthnModule};
use nest_rs_core::{ContestedDeclarationError, module};
use nest_rs_guards::Scope;
use nest_rs_http::StatusCode;
use nest_rs_mcp::{McpAccess, McpConfig, McpModule};
use nest_rs_oauth_resource::{OAuthResourceConfig, OAuthResourceModule, WELL_KNOWN_PATH};
use nest_rs_testing::TestApp;

use crate::EchoTool;

const RESOURCE: &str = "https://mcp.example.com/mcp";

#[module(
    imports = [
        AuthnModule::for_root(AuthnConfig {
            secret: Some("allowlist-secret-padded-to-32-byt".into()),
            ..AuthnConfig::default()
        }),
        OAuthResourceModule::for_root(OAuthResourceConfig {
            resource: Some(RESOURCE.into()),
            authorization_servers: vec!["https://auth.example.com".into()],
            ..OAuthResourceConfig::default()
        }),
        McpModule::for_root(McpAccess::Public),
    ],
    providers = [EchoTool],
)]
struct ResourceServer;

async fn boot(mcp: McpConfig) -> TestApp {
    TestApp::builder()
        .provide(mcp)
        .module::<ResourceServer>()
        .build()
        .await
        .expect("boots")
}

/// `tools/list` sent to `/mcp` with `Host: host`, the status rmcp's host check
/// answers.
async fn status_for(app: &TestApp, host: &str) -> StatusCode {
    app.http()
        .post("/mcp")
        .header("host", host)
        .header("content-type", "application/json")
        .header("accept", "application/json, text/event-stream")
        .body_json(&serde_json::json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" }))
        .send()
        .await
        .0
        .status()
}

/// The resource is the host a client reaches the server by: the allowlist
/// admits it beside its own entries, so a deployment names its host once.
#[tokio::test]
async fn the_mcp_allowlist_admits_the_host_the_resource_names() {
    let app = boot(McpConfig::default()).await;

    for host in ["mcp.example.com", "mcp.example.com:443", "localhost"] {
        assert_ne!(
            status_for(&app, host).await,
            StatusCode::FORBIDDEN,
            "{host} is admitted"
        );
    }
    assert_eq!(
        status_for(&app, "rebound.example").await,
        StatusCode::FORBIDDEN,
        "a host neither lists stays refused"
    );
}

/// An empty allowlist is a deliberate opening: the resource's host never
/// turns the check back on behind it.
#[tokio::test]
async fn an_emptied_allowlist_stays_open() {
    let app = boot(McpConfig {
        allowed_hosts: Vec::new(),
        ..McpConfig::default()
    })
    .await;

    assert_ne!(
        status_for(&app, "rebound.example").await,
        StatusCode::FORBIDDEN
    );
}

const POSTS_READ: Scope = Scope::new("posts:read");
const POSTS_WRITE: Scope = Scope::new("posts:write");

fn authn() -> AuthnConfig {
    AuthnConfig {
        secret: Some("declared-scopes-secret-padded-32b".into()),
        ..AuthnConfig::default()
    }
}

#[module(
    imports = [
        AuthnModule::for_root(authn()),
        OAuthResourceModule::for_root([POSTS_READ, POSTS_WRITE]),
    ],
)]
struct DeclaringServer;

#[module(imports = [OAuthResourceModule::for_root([POSTS_READ])])]
struct AnotherDeclaration;

fn pinned() -> OAuthResourceConfig {
    OAuthResourceConfig {
        resource: Some(RESOURCE.into()),
        authorization_servers: vec!["https://auth.example.com".into()],
        ..OAuthResourceConfig::default()
    }
}

#[module(
    imports = [
        AuthnModule::for_root(authn()),
        OAuthResourceModule::for_root(([POSTS_READ], pinned())),
    ],
)]
struct PinningAndDeclaringServer;

#[module(imports = [DeclaringServer, AnotherDeclaration])]
struct TwiceDeclaringServer;

/// The deployment's identity, with the scopes it advertises, seeded as a
/// deployment would set them.
fn deployed(scopes_supported: &[&str]) -> OAuthResourceConfig {
    OAuthResourceConfig {
        resource: Some("http://localhost".into()),
        authorization_servers: vec!["http://localhost/issuer".into()],
        scopes_supported: scopes_supported
            .iter()
            .map(|&scope| scope.to_owned())
            .collect(),
        ..OAuthResourceConfig::default()
    }
}

async fn advertised(scopes_supported: &[&str]) -> serde_json::Value {
    let app = TestApp::builder()
        .provide(deployed(scopes_supported))
        .module::<DeclaringServer>()
        .build()
        .await
        .expect("boots");
    let document = app.http().get(WELL_KNOWN_PATH).send().await;
    document.assert_status_is_ok();
    document
        .json()
        .await
        .value()
        .deserialize::<serde_json::Value>()["scopes_supported"]
        .clone()
}

/// The scopes the app's rules gate on are declared once, in code: the
/// document advertises them, with no variable repeating them.
#[tokio::test]
async fn the_declared_scopes_are_advertised_when_the_deployment_names_none() {
    assert_eq!(
        advertised(&[]).await,
        serde_json::json!(["posts:read", "posts:write"])
    );
}

/// `<PREFIX>_OAUTH__RESOURCE__SCOPES_SUPPORTED`, when the deployment sets it,
/// is what the document advertises.
#[tokio::test]
async fn the_deployment_s_scopes_supported_replace_the_declared_set() {
    assert_eq!(
        advertised(&["posts:read"]).await,
        serde_json::json!(["posts:read"])
    );
}

/// One import both pins the config and declares the scopes: the document
/// serves the pinned resource and advertises the declared set.
#[tokio::test]
async fn a_pinned_config_and_the_scopes_declared_beside_it_make_one_document() {
    let app = TestApp::for_module::<PinningAndDeclaringServer>()
        .await
        .expect("boots");
    let document = app.http().get(WELL_KNOWN_PATH).send().await;
    document.assert_status_is_ok();
    let document = document
        .json()
        .await
        .value()
        .deserialize::<serde_json::Value>();
    assert_eq!(document["resource"], RESOURCE, "{document}");
    assert_eq!(
        document["scopes_supported"],
        serde_json::json!([POSTS_READ.as_str()]),
        "{document}"
    );
}

/// An app advertises one set: a second declaration fails the boot naming both
/// imports, before any factory runs.
#[tokio::test]
async fn two_declared_scope_sets_fail_the_boot() {
    let Err(refused) = TestApp::builder()
        .provide(deployed(&[]))
        .module::<TwiceDeclaringServer>()
        .build()
        .await
    else {
        panic!("two scope sets for one document");
    };
    assert!(
        refused
            .downcast_ref::<ContestedDeclarationError>()
            .is_some(),
        "{refused:#}"
    );
}
