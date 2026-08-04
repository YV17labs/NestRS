//! The assistant as an OAuth 2.1 resource server: an unauthenticated MCP call
//! hands back everything a client needs to go get a token.

use nest_rs::http::poem::http::{StatusCode, header};
use serde_json::Value;

use features::testing::AUDIENCE as RESOURCE;

use crate::boot;

#[tokio::test]
async fn an_unauthenticated_tool_call_points_at_the_metadata_document() {
    let (_db, app) = boot().await;

    let resp = app.http().post("/mcp").send().await;
    resp.assert_status(StatusCode::UNAUTHORIZED);

    let challenge = resp
        .0
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .expect("the 401 carries a challenge")
        .to_str()
        .expect("ascii")
        .to_owned();
    assert!(
        challenge.contains(&format!(
            "resource_metadata=\"{RESOURCE}/.well-known/oauth-protected-resource\""
        )),
        "the challenge must point at this deployment's document: {challenge}",
    );
    let advertised = features::authz::constants::ALL.join(" ");
    assert!(
        challenge.contains(&advertised),
        "the challenge must advertise every scope the policy gates on \
         ({advertised}): {challenge}",
    );
}

#[tokio::test]
async fn the_metadata_document_names_the_resource_and_its_authorization_server() {
    let (_db, app) = boot().await;

    let resp = app
        .http()
        .get("/.well-known/oauth-protected-resource")
        .send()
        .await;
    resp.assert_status_is_ok();

    let body: Value =
        serde_json::from_slice(&resp.0.into_body().into_bytes().await.expect("a body"))
            .expect("the document is JSON");

    assert_eq!(body["resource"], RESOURCE);
    assert_eq!(
        body["authorization_servers"][0], "http://localhost:3001",
        "the demo's own auth app is the issuer, read from the environment",
    );
    assert_eq!(body["bearer_methods_supported"][0], "header");
}

#[tokio::test]
async fn discovery_is_reachable_without_a_token_but_tools_are_not() {
    let (_db, app) = boot().await;

    app.http()
        .get("/.well-known/oauth-protected-resource")
        .send()
        .await
        .assert_status_is_ok();

    app.http()
        .post("/mcp")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
