use features::authz::constants;
use features::testing::{ORG_ID, claims, client};
use features::{Claims, Role};
use nest_rs::testing::mcp::{call_method, call_tool, open_session, result};
use serde_json::json;

use crate::*;

#[tokio::test]
async fn a_repo_backed_tool_reads_rows_through_the_ambient_executor() {
    let app = boot().await;
    let org_id = seed_org_with_post(&app, "Acme", "acme-only-post").await;

    let body = call_tool(
        app.http(),
        POSTS_ENDPOINT,
        "list_posts",
        Some(&sign(&app, org_id)),
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
    let app = boot().await;
    let acme = seed_org_with_post(&app, "Acme", "acme-only-post").await;
    seed_org_with_post(&app, "Globex", "globex-only-post").await;

    let body = call_tool(
        app.http(),
        POSTS_ENDPOINT,
        "list_posts",
        Some(&sign(&app, acme)),
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
    let app = boot().await;
    let acme = seed_org_with_post(&app, "Acme", "acme-only-post").await;
    seed_org_with_post(&app, "Globex", "globex-only-post").await;

    let bearer = sign(&app, acme);
    let session = open_session(app.http(), POSTS_ENDPOINT, Some(&bearer)).await;
    let body = call_method(
        app.http(),
        POSTS_ENDPOINT,
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
async fn a_resource_read_of_another_orgs_row_or_a_malformed_uri_is_not_found_quoting_neither() {
    let app = boot().await;
    let acme = seed_org_with_post(&app, "Acme", "acme-only-post").await;
    let globex = seed_org_with_post(&app, "Globex", "globex-only-post").await;

    let acme_bearer = sign(&app, acme);
    let session = open_session(app.http(), POSTS_ENDPOINT, Some(&acme_bearer)).await;
    let listed = call_method(
        app.http(),
        POSTS_ENDPOINT,
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

    let own = call_method(
        app.http(),
        POSTS_ENDPOINT,
        &session,
        Some(&acme_bearer),
        "resources/read",
        json!({ "uri": uri }),
    )
    .await;
    let contents = &result(&own)["result"]["contents"][0];
    assert_eq!(contents["uri"], uri.as_str(), "{own}");
    assert_eq!(contents["mimeType"], "application/json", "{own}");
    assert!(
        contents["text"]
            .as_str()
            .is_some_and(|text| text.contains("acme-only-post") && text.contains("seeded")),
        "the caller reads its own org's post through the template: {own}",
    );

    let globex_bearer = sign(&app, globex);
    let session = open_session(app.http(), POSTS_ENDPOINT, Some(&globex_bearer)).await;
    for asked in [uri.as_str(), "post://not-a-uuid"] {
        let denied = call_method(
            app.http(),
            POSTS_ENDPOINT,
            &session,
            Some(&globex_bearer),
            "resources/read",
            json!({ "uri": asked }),
        )
        .await;

        assert!(
            !denied.contains("seeded")
                && denied.contains("-32002")
                && denied.contains(r#""message":"Resource not found""#),
            "`resources/read` must fail closed on another org's row and on a malformed \
             uri alike. Body: {denied}",
        );
        assert!(!denied.contains(asked), "{denied}");
    }
}

#[tokio::test]
async fn a_token_without_the_read_scope_is_refused_every_posts_operation_by_name() {
    let app = boot().await;
    let narrowed = app.bearer(&Claims {
        scopes: vec![constants::AUDIO_TRANSCODE.into()],
        ..claims(ORG_ID, Role::Admin, client())
    });

    let listed = call_tool(app.http(), POSTS_ENDPOINT, "list_posts", Some(&narrowed)).await;
    let session = open_session(app.http(), POSTS_ENDPOINT, Some(&narrowed)).await;
    let mut bodies = vec![listed];
    for (method, params) in [
        ("prompts/get", json!({ "name": "draft_follow_up" })),
        ("resources/list", json!({})),
        (
            "resources/read",
            json!({ "uri": format!("post://{ORG_ID}") }),
        ),
    ] {
        bodies.push(
            call_method(
                app.http(),
                POSTS_ENDPOINT,
                &session,
                Some(&narrowed),
                method,
                params,
            )
            .await,
        );
    }

    for body in bodies {
        assert!(
            body.contains("insufficient_scope") && body.contains(constants::POSTS_READ.as_str()),
            "the posture gates every operation of the host, and the refusal names the \
             scope it requires rather than answering an empty list: {body}",
        );
    }
}
