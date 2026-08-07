use nest_rs::testing::mcp::{call_method, call_tool, open_session};
use serde_json::json;

use crate::*;

#[tokio::test]
async fn a_repo_backed_tool_reads_rows_through_the_ambient_executor() {
    let (db, app) = boot().await;
    let org_id = seed_org_with_post(&db.connection(), "Acme", "acme-only-post").await;

    let body = call_tool(
        app.http(),
        "/posts/mcp",
        "list_posts",
        Some(&bearer_for(&org_id.to_string())),
    )
    .await;

    assert!(
        body.contains("acme-only-post"),
        "the tool must reach the database through `Repo` — no ambient executor \
         would fail closed instead. Body: {body}",
    );
}

#[tokio::test]
async fn a_tool_never_sees_another_orgs_rows() {
    let (db, app) = boot().await;
    let conn = db.connection();
    let acme = seed_org_with_post(&conn, "Acme", "acme-only-post").await;
    seed_org_with_post(&conn, "Globex", "globex-only-post").await;

    let body = call_tool(
        app.http(),
        "/posts/mcp",
        "list_posts",
        Some(&bearer_for(&acme.to_string())),
    )
    .await;

    assert!(
        body.contains("acme-only-post"),
        "the caller's own org row is readable: {body}",
    );
    assert!(
        !body.contains("globex-only-post"),
        "row-level filtering must apply inside the tool body — the tool writes \
         no filter, the ambient ability does. Body: {body}",
    );
}

#[tokio::test]
async fn a_prompt_is_row_filtered_like_a_tool() {
    let (db, app) = boot().await;
    let conn = db.connection();
    let acme = seed_org_with_post(&conn, "Acme", "acme-only-post").await;
    seed_org_with_post(&conn, "Globex", "globex-only-post").await;

    let bearer = bearer_for(&acme.to_string());
    let session = open_session(app.http(), "/posts/mcp", Some(&bearer)).await;
    let body = call_method(
        app.http(),
        "/posts/mcp",
        &session,
        Some(&bearer),
        "prompts/get",
        json!({ "name": "draft_follow_up" }),
    )
    .await;

    assert!(
        body.contains("acme-only-post"),
        "the prompt reads the caller's own rows: {body}",
    );
    assert!(
        !body.contains("globex-only-post"),
        "a prompt is a read like any other — the ambient ability must scope it \
         too, or the framework leaks another org's data through `prompts/get`. \
         Body: {body}",
    );
}

#[tokio::test]
async fn a_resource_read_cannot_reach_another_orgs_row() {
    let (db, app) = boot().await;
    let conn = db.connection();
    let acme = seed_org_with_post(&conn, "Acme", "acme-only-post").await;
    let globex = seed_org_with_post(&conn, "Globex", "globex-only-post").await;

    let acme_bearer = bearer_for(&acme.to_string());
    let session = open_session(app.http(), "/posts/mcp", Some(&acme_bearer)).await;
    let listed = call_method(
        app.http(),
        "/posts/mcp",
        &session,
        Some(&acme_bearer),
        "resources/list",
        json!({}),
    )
    .await;
    assert!(
        listed.contains("acme-only-post") && !listed.contains("globex-only-post"),
        "`resources/list` is scoped by the ambient ability: {listed}",
    );

    let uri = listed
        .split("\"uri\":\"")
        .nth(1)
        .and_then(|rest| rest.split('"').next())
        .expect("the listing carries a post:// uri")
        .to_owned();

    let globex_bearer = bearer_for(&globex.to_string());
    let session = open_session(app.http(), "/posts/mcp", Some(&globex_bearer)).await;
    let denied = call_method(
        app.http(),
        "/posts/mcp",
        &session,
        Some(&globex_bearer),
        "resources/read",
        json!({ "uri": uri }),
    )
    .await;

    assert!(
        !denied.contains("seeded") && denied.contains("error"),
        "`resources/read` must fail closed on another org's row — the handler \
         writes no org check, `CrudService::access` and the ambient ability do. \
         Body: {denied}",
    );
}
