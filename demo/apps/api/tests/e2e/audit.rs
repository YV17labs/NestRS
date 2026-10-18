use nest_rs::http::{StatusCode, header};
use nest_rs::testing::LogCapture;
use serde_json::json;

use crate::*;

#[tokio::test]
async fn every_write_the_api_admits_is_audited_once_and_nothing_else_is() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    let logs = LogCapture::install();
    create_org(&app, &admin, "AuditedAcme").await;
    app.http()
        .get("/orgs")
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await
        .assert_status_is_ok();
    app.http()
        .post("/orgs")
        .body_json(&json!({ "name": "Anonymous" }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let audited = logs.find(features::audit::TARGET, "write audited");
    assert_eq!(audited.len(), 1, "the admitted create alone: {audited:#?}");
    assert_eq!(audited[0].field("host").as_deref(), Some("OrgsController"));
    assert_eq!(audited[0].field("status").as_deref(), Some("201"));
}
