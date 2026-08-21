use features::app_authz::constants;
use features::testing::{ORG_ID, token_with_scopes};
use nest_rs::testing::TestApp;
use poem::http::{StatusCode, header};
use serde_json::json;
use uuid::Uuid;

use crate::*;

fn narrow_bearer(org_id: &str, sub: Uuid, scopes: &[&str]) -> String {
    format!(
        "Bearer {}",
        token_with_scopes(
            Uuid::parse_str(org_id).expect("valid org uuid"),
            vec![features::Role::Admin],
            Some(sub),
            scopes.iter().map(|s| (*s).to_owned()).collect(),
        )
    )
}

async fn org_with_author(app: &TestApp, label: &str) -> (String, Uuid) {
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(app, &bootstrap, label).await;
    let admin = format!("Bearer {}", token_for(&org, "admin").await);
    let author = create_user(
        app,
        &admin,
        "Author",
        &format!("author@{}.test", label.to_lowercase()),
    )
    .await;
    let author = Uuid::parse_str(&author).expect("valid user uuid");
    (org, author)
}

async fn create_post_with(app: &TestApp, bearer: &str) -> poem::Response {
    app.http()
        .post("/posts")
        .header(header::AUTHORIZATION, bearer)
        .body_json(&json!({ "title": "Launch", "body": "Big news" }))
        .send()
        .await
        .0
}

#[tokio::test]
async fn a_read_only_delegation_reads_but_cannot_write() {
    let (_db, app) = boot().await;
    let (org, author) = org_with_author(&app, "ScopeAcme").await;

    let read_only = narrow_bearer(&org, author, &[constants::POSTS_READ]);

    app.http()
        .get("/posts")
        .header(header::AUTHORIZATION, &read_only)
        .send()
        .await
        .assert_status_is_ok();

    assert_eq!(
        create_post_with(&app, &read_only).await.status(),
        StatusCode::FORBIDDEN,
        "a read-only delegation must not be able to write",
    );
}

#[tokio::test]
async fn widening_the_delegation_is_what_unblocks_the_write() {
    let (_db, app) = boot().await;
    let (org, author) = org_with_author(&app, "ScopeGlobex").await;

    let full = narrow_bearer(
        &org,
        author,
        &[constants::POSTS_READ, constants::POSTS_WRITE],
    );

    let status = create_post_with(&app, &full).await.status();
    assert!(
        status.is_success(),
        "the same caller, one scope wider, may write — got {status}",
    );
}

#[tokio::test]
async fn a_delegation_with_no_post_scope_reads_nothing() {
    let (_db, app) = boot().await;
    let (org, author) = org_with_author(&app, "ScopeInitech").await;

    let unrelated = narrow_bearer(&org, author, &[constants::AUDIO_TRANSCODE]);

    assert_eq!(
        app.http()
            .get("/posts")
            .header(header::AUTHORIZATION, &unrelated)
            .send()
            .await
            .0
            .status(),
        StatusCode::FORBIDDEN,
    );
}
