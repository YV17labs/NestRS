//! RFC 6749 §4.4.2 and §5.1 on the wire.

use nest_rs_core::{Discovery, module};
use nest_rs_http::{HttpControllerMeta, HttpModule, controller, routes};
use nest_rs_oauth_server::{AccessTokenRequest, AccessTokenResponse};
use nest_rs_testing::TestApp;

#[test]
fn the_response_carries_exactly_the_members_5_1_marks_required() {
    let body = serde_json::to_value(AccessTokenResponse::bearer("t", 3600)).expect("serializes");

    assert_eq!(
        body,
        serde_json::json!({ "access_token": "t", "token_type": "Bearer", "expires_in": 3600 })
    );
}

/// §5.1 and §3.3: an issuer that narrows or defaults the grant says so.
#[test]
fn a_granted_scope_is_written_as_the_scope_member() {
    let body =
        serde_json::to_value(AccessTokenResponse::bearer("t", 3600).with_scope(["posts:read"]))
            .expect("serializes");

    assert_eq!(body["scope"], "posts:read");
}

#[test]
fn several_granted_scopes_are_written_space_delimited() {
    let body = serde_json::to_value(
        AccessTokenResponse::bearer("t", 3600).with_scope(["posts:read", "posts:write"]),
    )
    .expect("serializes");

    assert_eq!(body["scope"], "posts:read posts:write");
}

#[test]
fn a_request_omitting_scope_deserializes_because_3_3_makes_it_optional() {
    let request: AccessTokenRequest =
        serde_json::from_str(r#"{"grant_type":"client_credentials"}"#).expect("body");

    assert_eq!(request.grant_type, "client_credentials");
    assert_eq!(request.scope, None);
}

#[test]
fn an_unknown_grant_reaches_the_issuer_rather_than_failing_to_deserialize() {
    let request: AccessTokenRequest =
        serde_json::from_str(r#"{"grant_type":"urn:ietf:params:oauth:grant-type:device_code"}"#)
            .expect("body");

    assert_eq!(
        request.grant_type,
        "urn:ietf:params:oauth:grant-type:device_code"
    );
}

/// `#[api(response = …)]` documents the body a bare return no longer spells.
#[controller(path = "/")]
struct IssuerController;

#[routes]
impl IssuerController {
    #[post("/token")]
    #[public]
    #[api(response = AccessTokenResponse)]
    async fn token(&self) -> poem::Result<AccessTokenResponse> {
        Ok(AccessTokenResponse::bearer("t", 3600).with_scope(["posts:read"]))
    }
}

#[module(imports = [HttpModule::for_root(None)], providers = [IssuerController])]
struct IssuerApp;

/// RFC 6749 §5.1's `no-store` and `no-cache` reach the client through the edge.
#[tokio::test]
async fn a_token_route_answers_no_store_through_the_transport() {
    let app = TestApp::for_module::<IssuerApp>()
        .await
        .expect("the issuer app boots");

    let response = app.http().post("/token").send().await;
    response.assert_status_is_ok();
    response.assert_header("cache-control", "no-store");
    response.assert_header("pragma", "no-cache");
    response
        .assert_json(serde_json::json!({
            "access_token": "t",
            "token_type": "Bearer",
            "expires_in": 3600,
            "scope": "posts:read",
        }))
        .await;
}

#[tokio::test]
async fn the_route_documents_the_access_token_response_it_returns() {
    let app = TestApp::for_module::<IssuerApp>()
        .await
        .expect("the issuer app boots");

    let controllers = Discovery::new(app.container()).meta::<HttpControllerMeta>();
    let route = controllers
        .iter()
        .flat_map(|controller| controller.meta.routes.iter())
        .find(|route| route.handler == "token")
        .expect("the token route is discovered");
    let schema_of = route.response.expect("a documented response");

    let mut generator = schemars::SchemaGenerator::default();
    let documented = schema_of(&mut generator);
    assert_eq!(
        documented.get("$ref").and_then(serde_json::Value::as_str),
        Some("#/$defs/AccessTokenResponse"),
    );
    let definition = generator
        .definitions()
        .get("AccessTokenResponse")
        .expect("the response's schema is a named component");
    let mut members: Vec<&str> = definition["properties"]
        .as_object()
        .expect("an object schema")
        .keys()
        .map(String::as_str)
        .collect();
    members.sort_unstable();
    assert_eq!(
        members,
        ["access_token", "expires_in", "scope", "token_type"]
    );
}
