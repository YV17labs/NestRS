use features::Role;
use features::testing::{cancelling_reads_of, claims, client, seed_org, seed_user};
use features::users::UserRole;
use nest_rs::http::StatusCode;
use nest_rs::ws::CloseCode;
use serde_json::Value;

use super::harness::*;

#[tokio::test]
async fn users_list_over_ws_is_org_scoped_and_email_masked() {
    let app = serve().await;

    let conn = connection(app.container());
    let org_a = seed_org(&conn, "WS A").await;
    let org_b = seed_org(&conn, "WS B").await;
    for (org_id, name, email) in [
        (org_a, "Alice", "alice@a.test"),
        (org_a, "Bob", "bob@a.test"),
        (org_b, "Carol", "carol@b.test"),
    ] {
        seed_user(&conn, org_id, name, email, UserRole::User).await;
    }

    let member = app.bearer(&claims(org_a, Role::User, client()));
    let mut socket = open_as(&app, "/users", &member).await;
    socket.send("users.list", Value::Null).await;
    let reply = socket.next_envelope().await;

    assert_eq!(reply["event"], "users.list");
    let rows = reply["data"].as_array().expect("a list of users");
    assert_eq!(rows.len(), 2, "only org A's members are visible: {rows:?}");
    for row in rows {
        assert!(row.get("id").is_some(), "id is exposed: {row:?}");
        assert!(row.get("name").is_some(), "name is exposed: {row:?}");
        assert!(
            row.get("email").is_none(),
            "a member must not see email over WS: {row:?}",
        );
    }

    socket.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn a_cancelled_read_reaches_the_client_as_its_class() {
    let app = serve().await;
    let conn = connection(app.container());
    let mut socket = open(&app, "/users").await;

    let reply = cancelling_reads_of(&conn, "user", async {
        socket.send("users.list", Value::Null).await;
        socket.next_envelope().await
    })
    .await;

    assert_eq!(reply["event"], "users.list");
    assert_eq!(reply["data"]["code"], "UNAVAILABLE", "{reply}");

    socket.close(CloseCode::Normal, "done").await;
    app.shutdown().await.expect("transport shuts down");
}

#[tokio::test]
async fn users_gateway_refuses_an_unauthenticated_upgrade() {
    let app = boot().await;
    app.http()
        .get("/users")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
