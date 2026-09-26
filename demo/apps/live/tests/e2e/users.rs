use features::Role;
use nest_rs::http::poem::http::StatusCode;
use nest_rs::ws::CloseCode;
use sea_orm::ConnectionTrait;
use serde_json::Value;
use uuid::Uuid;

use super::harness::*;

#[tokio::test]
async fn users_list_over_ws_is_org_scoped_and_email_masked() {
    let (db, app) = serve().await;

    let org_a = Uuid::now_v7();
    let org_b = Uuid::now_v7();
    let (alice, bob, carol) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    let conn = db.connection();
    conn.execute_unprepared(&format!(
        "INSERT INTO org (id, name) VALUES ('{org_a}', 'WS A'), ('{org_b}', 'WS B')"
    ))
    .await
    .expect("seed orgs");
    conn.execute_unprepared(&format!(
        "INSERT INTO \"user\" (id, org_id, name, email, role) VALUES \
         ('{alice}', '{org_a}', 'Alice', 'alice@a.test', 'user'), \
         ('{bob}', '{org_a}', 'Bob', 'bob@a.test', 'user'), \
         ('{carol}', '{org_b}', 'Carol', 'carol@b.test', 'user')"
    ))
    .await
    .expect("seed users");

    let token = token_for_org(org_a, Role::User).await;
    let mut socket = app.socket("/users").bearer(&token).connect().await;
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
async fn users_gateway_refuses_an_unauthenticated_upgrade() {
    let (_db, app) = boot().await;
    app.http()
        .get("/users")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
