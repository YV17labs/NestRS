//! Two features' tools on one MCP endpoint, and a second endpoint that stays
//! its own.
//!
//! `assistant` mounts `AudioMcpModule` **and** `UsersMcpModule` at `/mcp`, and
//! `PostsMcpModule` at `/posts/mcp`. Both halves matter: an MCP client config
//! points at a single URL, so a product needs several features on one path; and
//! the spec namespaces tools per endpoint, so an app that deliberately serves
//! two paths must keep them apart.
//!
//! What this proves that the framework suite cannot: the merge holds with the
//! **real** authz bridge in front of it — the ambient ability reaches a tool
//! body that was reached through the composite, so row filtering is not lost in
//! the routing.

use nest_rs::mcp::declared_endpoint;
use nest_rs::testing::mcp::{call_method, call_tool, initialize, open_session, result};
use serde_json::json;

use crate::*;

/// The tools each feature contributes, by the endpoint they contribute to.
const SHARED_TOOLS: [&str; 2] = ["transcode_status", "list_people"];
const POSTS_TOOL: &str = "list_posts";

#[tokio::test]
async fn one_endpoint_lists_both_features_tools() {
    let (_db, app) = boot().await;
    let auth = bearer();

    let session = open_session(app.http(), "/mcp", Some(&auth)).await;
    let body = call_method(
        app.http(),
        "/mcp",
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
        "`/posts/mcp` is a separate endpoint — merging every host in the process \
         would destroy the spec's per-endpoint namespacing: {body}",
    );
}

#[tokio::test]
async fn the_second_endpoint_serves_only_its_own_feature() {
    let (_db, app) = boot().await;
    let auth = bearer();

    let session = open_session(app.http(), "/posts/mcp", Some(&auth)).await;
    let body = call_method(
        app.http(),
        "/posts/mcp",
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
            "`{tool}` belongs to `/mcp`, not here: {body}",
        );
    }
}

/// A client sees **one** server on `/mcp`, so the app names it — otherwise the
/// endpoint would introduce itself as whichever feature `imports = [..]` listed
/// first, and hand the model that one feature's instructions for the whole
/// surface.
///
/// Asserted against the declaration read back from the container, never a
/// literal copied out of `module.rs`: a copy passes while the two drift.
#[tokio::test]
async fn the_app_names_the_endpoint_its_features_share() {
    let (_db, app) = boot().await;

    let declared = declared_endpoint(app.container(), "/mcp")
        .expect("the assistant declares the identity of the endpoint two features share");

    let body = initialize(app.http(), "/mcp", Some(&bearer())).await;
    let advertised = &result(&body)["result"];

    assert_eq!(
        advertised["serverInfo"]["name"].as_str(),
        Some(declared.implementation().name.as_str()),
        "the endpoint introduces itself as the app: {advertised}",
    );
    assert_eq!(
        advertised["serverInfo"]["version"].as_str(),
        Some(declared.implementation().version.as_str()),
        "…at the app's own version, not the framework's: {advertised}",
    );
    assert_eq!(
        advertised["instructions"].as_str(),
        declared.declared_instructions(),
        "…and the model reads the instructions that frame the whole endpoint: {advertised}",
    );
}

/// The merged endpoint keeps the guarantee a single host had: the tool reads
/// through `Repo`, so the caller's ability filters the rows — the composite
/// routes the call, it does not sit between the body and its ambient state.
#[tokio::test]
async fn a_merged_endpoint_still_row_filters_the_tool_body() {
    let (db, app) = boot().await;
    let conn = db.connection();
    let acme = seed_org_with_post(&conn, "Acme", "acme-only-post").await;
    seed_org_with_post(&conn, "Globex", "globex-only-post").await;

    let body = call_tool(
        app.http(),
        "/mcp",
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
