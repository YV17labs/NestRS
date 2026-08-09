use nest_rs::mcp::endpoint_identity;
use nest_rs::testing::mcp::{call_method, call_tool, initialize, open_session, result};
use serde_json::json;

use crate::*;

const SHARED_TOOLS: [&str; 2] = ["transcode_status", "list_people"];
const POSTS_TOOL: &str = "list_posts";

#[tokio::test]
async fn one_endpoint_lists_both_features_tools() {
    let (_db, app) = boot().await;
    let auth = bearer();

    let session = open_session(app.http(), SHARED_ENDPOINT, Some(&auth)).await;
    let body = call_method(
        app.http(),
        SHARED_ENDPOINT,
        &session,
        Some(&auth),
        "tools/list",
        json!({}),
    )
    .await;

    for tool in SHARED_TOOLS {
        assert!(
            body.contains(tool),
            "`{tool}` is contributed by its own feature's `mcp/` adapter and must \
             appear in the one listing a client reads: {body}",
        );
    }
    assert!(
        !body.contains(POSTS_TOOL),
        "the posts host is a separate endpoint — merging every host in the process \
         would destroy the spec's per-endpoint namespacing: {body}",
    );
}

#[tokio::test]
async fn the_second_endpoint_serves_only_its_own_feature() {
    let (_db, app) = boot().await;
    let auth = bearer();

    let session = open_session(app.http(), POSTS_ENDPOINT, Some(&auth)).await;
    let body = call_method(
        app.http(),
        POSTS_ENDPOINT,
        &session,
        Some(&auth),
        "tools/list",
        json!({}),
    )
    .await;

    assert!(body.contains(POSTS_TOOL), "its own tool is listed: {body}");
    for tool in SHARED_TOOLS {
        assert!(
            !body.contains(tool),
            "`{tool}` belongs to the shared endpoint, not here: {body}",
        );
    }
}

#[tokio::test]
async fn the_app_names_the_endpoint_its_features_share() {
    let (_db, app) = boot().await;

    let resolved = endpoint_identity(app.container(), SHARED_ENDPOINT);
    let declared = resolved
        .implementation()
        .expect("the assistant names itself for every endpoint it exposes");

    let body = initialize(app.http(), SHARED_ENDPOINT, Some(&bearer())).await;
    let advertised = &result(&body)["result"];

    assert_eq!(
        advertised["serverInfo"]["name"].as_str(),
        Some(declared.name.as_str()),
        "the endpoint introduces itself as the app: {advertised}",
    );
    assert_eq!(
        advertised["serverInfo"]["version"].as_str(),
        Some(declared.version.as_str()),
        "…at the app's own version, not the framework's: {advertised}",
    );

    let instructions = advertised["instructions"].as_str().unwrap_or_default();
    assert!(
        instructions.contains("scoped to the caller's token"),
        "…and the model reads the app's one paragraph about how to use the \
         server, which is the only party that sees every feature on this \
         endpoint: {advertised}",
    );
    for tool in SHARED_TOOLS {
        assert!(
            !instructions.contains(tool),
            "`{tool}` is described by its own #[tool(description)] — restating it \
             here would duplicate what the model already reads when it picks a \
             tool: {instructions:?}",
        );
    }
}

#[tokio::test]
async fn a_feature_names_the_endpoint_it_owns() {
    let (_db, app) = boot().await;

    let shared = endpoint_identity(app.container(), SHARED_ENDPOINT);
    let app_identity = shared.implementation().expect("the app names itself");

    let body = initialize(app.http(), POSTS_ENDPOINT, Some(&bearer())).await;
    let advertised = &result(&body)["result"];

    assert_eq!(
        advertised["serverInfo"]["name"].as_str(),
        Some("nestrs-assistant-posts"),
        "the posts endpoint carries the name its own `#[mcp]` host declared: {advertised}",
    );
    assert_eq!(
        advertised["serverInfo"]["version"].as_str(),
        Some(app_identity.version.as_str()),
        "…at the version only the app can know: {advertised}",
    );

    assert_eq!(
        advertised["instructions"].as_str(),
        shared.instructions(),
        "…and reads the same instructions as every other endpoint: how to use \
         this server is the app's word, declared once: {advertised}",
    );
}

#[tokio::test]
async fn a_merged_endpoint_still_row_filters_the_tool_body() {
    let (db, app) = boot().await;
    let conn = db.connection();
    let acme = seed_org_with_post(&conn, "Acme", "acme-only-post").await;
    seed_org_with_post(&conn, "Globex", "globex-only-post").await;

    let body = call_tool(
        app.http(),
        SHARED_ENDPOINT,
        "list_people",
        Some(&bearer_for(&acme.to_string())),
    )
    .await;

    assert!(
        body.contains("Acme author"),
        "the caller's own organisation is readable through the shared \
         endpoint: {body}",
    );
    assert!(
        !body.contains("Globex author"),
        "row-level filtering must survive the merge — the tool writes no filter, \
         the ambient ability does. Body: {body}",
    );
}
