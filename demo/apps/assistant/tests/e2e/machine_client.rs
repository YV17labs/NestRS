use nest_rs::http::{StatusCode, header};
use nest_rs::testing::mcp::{call_method, open_session, post_message};
use serde_json::json;

use features::testing::{ORG_ID, claims, client, user};
use features::{Role, Subject};

use crate::{SHARED_ENDPOINT, boot};

#[tokio::test]
async fn a_client_credentials_token_opens_a_session_no_other_principal_reaches() {
    let app = boot().await;
    let machine = app.bearer(&claims(ORG_ID, Role::Admin, client()));
    let other_client = app.bearer(&claims(
        ORG_ID,
        Role::Admin,
        Subject::Client("reporting".into()),
    ));
    let colleague = app.bearer(&claims(ORG_ID, Role::Admin, user()));

    let session = open_session(app.http(), SHARED_ENDPOINT, Some(&machine)).await;

    for stranger in [&other_client, &colleague] {
        post_message(
            app.http(),
            SHARED_ENDPOINT,
            Some(&session),
            Some(stranger),
            &json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }),
        )
        .await
        .assert_status(StatusCode::NOT_FOUND);
    }
    app.http()
        .delete(SHARED_ENDPOINT)
        .header("host", "localhost")
        .header("mcp-session-id", session.as_str())
        .header(header::AUTHORIZATION, other_client.as_str())
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let listed = call_method(
        app.http(),
        SHARED_ENDPOINT,
        &session,
        Some(&machine),
        "tools/list",
        json!({}),
    )
    .await;
    assert!(
        listed.contains("list_people"),
        "the session still serves the client that opened it: {listed}"
    );
}
