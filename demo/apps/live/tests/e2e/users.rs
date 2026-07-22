use features::Role;
use futures_util::SinkExt;
use nest_rs_http::HttpTransport;
use nest_rs_http::poem::http::StatusCode;
use serde_json::json;
use tokio_tungstenite::tungstenite::Message;
use uuid::Uuid;

use super::harness::*;

#[tokio::test]
async fn users_list_over_ws_is_org_scoped_and_email_masked() {
    use sea_orm::{ConnectionTrait, Database};

    let bind = "127.0.0.1:13348";
    nest_rs_testing::load_project_env();
    let url = std::env::var("NESTRS_DATABASE__URL")
        .expect("NESTRS_DATABASE__URL must point at a reachable Postgres for this test");
    let db = Database::connect(&url).await.expect("connect to Postgres");

    let org_a = Uuid::now_v7();
    let org_b = Uuid::now_v7();
    let (alice, bob, carol) = (Uuid::now_v7(), Uuid::now_v7(), Uuid::now_v7());
    db.execute_unprepared(&format!(
        "INSERT INTO org (id, name) VALUES ('{org_a}', 'WS A {org_a}'), ('{org_b}', 'WS B {org_b}')"
    ))
    .await
    .expect("seed orgs");
    db.execute_unprepared(&format!(
        "INSERT INTO \"user\" (id, org_id, name, email, role) VALUES \
         ('{alice}', '{org_a}', 'Alice', 'alice-{alice}@a.test', 'user'), \
         ('{bob}', '{org_a}', 'Bob', 'bob-{bob}@a.test', 'user'), \
         ('{carol}', '{org_b}', 'Carol', 'carol-{carol}@b.test', 'user')"
    ))
    .await
    .expect("seed users");

    let app = boot_builder()
        .build_headless()
        .await
        .expect("LiveModule boots headless");
    let handle = app
        .spawn_transport(HttpTransport::new().bind(bind))
        .await
        .expect("HTTP transport serves");

    let token = token_for_org(org_a, Role::User).await;
    let mut socket = connect_with_retry(&format!("ws://{bind}/users"), &token).await;
    socket
        .send(Message::Text(
            json!({ "event": "users.list" }).to_string().into(),
        ))
        .await
        .expect("request users.list");
    let reply = next_json(&mut socket).await;

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

    socket.close(None).await.ok();
    handle.shutdown().await.expect("transport shuts down");

    db.execute_unprepared(&format!(
        "DELETE FROM \"user\" WHERE org_id IN ('{org_a}', '{org_b}')"
    ))
    .await
    .ok();
    db.execute_unprepared(&format!(
        "DELETE FROM org WHERE id IN ('{org_a}', '{org_b}')"
    ))
    .await
    .ok();
}

#[tokio::test]
async fn users_gateway_refuses_an_unauthenticated_upgrade() {
    let app = boot_builder().build().await.expect("LiveModule boots");
    app.http()
        .get("/users")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}
