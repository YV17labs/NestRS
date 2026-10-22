use nest_rs::http::{StatusCode, header};
use nest_rs::testing::mcp::{call_method, open_session, post_message};
use serde_json::{Value, json};
use uuid::Uuid;

use features::testing::{AUDIENCE as RESOURCE, ORG_ID, token_for};

use crate::{SHARED_ENDPOINT, boot};

#[tokio::test]
async fn an_unauthenticated_tool_call_points_at_the_metadata_document() {
    let (_db, app) = boot().await;

    let resp = app.http().post(SHARED_ENDPOINT).send().await;
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
        .post(SHARED_ENDPOINT)
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn another_users_bearer_reaches_no_session_it_did_not_open() {
    let (_db, app) = boot().await;
    let owner = format!(
        "Bearer {}",
        token_for(ORG_ID, "admin", Some(Uuid::now_v7()))
    );
    let colleague = format!("Bearer {}", token_for(ORG_ID, "user", Some(Uuid::now_v7())));
    let session = open_session(app.http(), SHARED_ENDPOINT, Some(&owner)).await;

    post_message(
        app.http(),
        SHARED_ENDPOINT,
        Some(&session),
        Some(&colleague),
        &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
    )
    .await
    .assert_status(StatusCode::NOT_FOUND);
    app.http()
        .delete(SHARED_ENDPOINT)
        .header("host", "localhost")
        .header("mcp-session-id", session.as_str())
        .header(header::AUTHORIZATION, colleague.as_str())
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let listed = call_method(
        app.http(),
        SHARED_ENDPOINT,
        &session,
        Some(&owner),
        "tools/list",
        json!({}),
    )
    .await;
    assert!(
        listed.contains("list_people"),
        "the session still serves the user who opened it: {listed}"
    );
}
