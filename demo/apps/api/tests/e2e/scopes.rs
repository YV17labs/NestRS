use features::authz::constants;
use features::{Claims, Subject};
use nest_rs::guards::Scope;
use nest_rs::http::{StatusCode, header};
use nest_rs::testing::{LogCapture, TestApp};
use serde_json::json;
use uuid::Uuid;

use crate::*;

fn narrow_bearer(app: &TestApp, org_id: Uuid, sub: Subject, scopes: &[Scope]) -> String {
    app.bearer(&Claims {
        scopes: scopes.iter().copied().map(String::from).collect(),
        ..claims(org_id, Role::Admin, sub)
    })
}

async fn org_with_author(app: &TestApp, label: &str) -> (Uuid, Uuid) {
    let bootstrap = sign(app, ORG_ID, Role::Admin);
    let org = create_org(app, &bootstrap, label).await;
    let admin = sign(app, org, Role::Admin);
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

async fn create_post_with(app: &TestApp, bearer: &str) -> nest_rs::http::Response {
    app.http()
        .post("/posts")
        .header(header::AUTHORIZATION, bearer)
        .body_json(&json!({ "title": "Launch", "body": "Big news" }))
        .send()
        .await
        .0
}

fn www_authenticate(refused: &nest_rs::http::Response) -> &str {
    refused
        .headers()
        .get(header::WWW_AUTHENTICATE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or_default()
}

#[tokio::test]
async fn a_read_only_delegation_reads_but_cannot_write() {
    let app = boot().await;
    let (org, author) = org_with_author(&app, "ScopeAcme").await;

    let read_only = narrow_bearer(&app, org, Subject::User(author), &[constants::POSTS_READ]);

    app.http()
        .get("/posts")
        .header(header::AUTHORIZATION, &read_only)
        .send()
        .await
        .assert_status_is_ok();

    let refused = create_post_with(&app, &read_only).await;
    assert_eq!(
        refused.status(),
        StatusCode::FORBIDDEN,
        "a read-only delegation must not be able to write",
    );
    let challenge = www_authenticate(&refused);
    assert!(
        challenge.contains(r#"error="insufficient_scope""#)
            && challenge.contains(&format!(r#"scope="{}""#, constants::POSTS_WRITE)),
        "a user's write is refused for the scope a wider token carries: {challenge}"
    );
}

#[tokio::test]
async fn widening_the_delegation_is_what_unblocks_the_write() {
    let app = boot().await;
    let (org, author) = org_with_author(&app, "ScopeGlobex").await;

    let full = narrow_bearer(
        &app,
        org,
        Subject::User(author),
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
    let app = boot().await;
    let (org, author) = org_with_author(&app, "ScopeInitech").await;

    let unrelated = narrow_bearer(
        &app,
        org,
        Subject::User(author),
        &[constants::AUDIO_TRANSCODE],
    );

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

#[tokio::test]
async fn a_machine_token_delegated_reading_is_refused_a_write_by_an_insufficient_scope_challenge() {
    let app = boot().await;
    let (org, author) = org_with_author(&app, "ScopeHooli").await;
    let writer = narrow_bearer(
        &app,
        org,
        Subject::User(author),
        &[constants::POSTS_READ, constants::POSTS_WRITE],
    );
    let post = create_post(&app, &writer, "Launch", "Big news").await;

    let reader = narrow_bearer(&app, org, client(), &[constants::POSTS_READ]);

    app.http()
        .get(format!("/posts/{post}"))
        .header(header::AUTHORIZATION, &reader)
        .send()
        .await
        .assert_status_is_ok();

    let refused = app
        .http()
        .delete(format!("/posts/{post}"))
        .header(header::AUTHORIZATION, &reader)
        .send()
        .await;
    refused.assert_status(StatusCode::FORBIDDEN);
    let challenge = www_authenticate(&refused.0);
    assert!(
        challenge.contains(r#"error="insufficient_scope""#),
        "{challenge}"
    );
    assert!(
        challenge.contains(&format!(r#"scope="{}""#, constants::POSTS_WRITE)),
        "{challenge}"
    );

    app.http()
        .get(format!("/posts/{post}"))
        .header(header::AUTHORIZATION, &reader)
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn a_refused_read_names_the_one_scope_it_needs_on_every_edge() {
    let app = boot().await;
    let (org, author) = org_with_author(&app, "ScopeVandelay").await;
    let unrelated = narrow_bearer(
        &app,
        org,
        Subject::User(author),
        &[constants::AUDIO_TRANSCODE],
    );

    let refused = app
        .http()
        .get(format!("/posts/{}", Uuid::now_v7()))
        .header(header::AUTHORIZATION, &unrelated)
        .send()
        .await;
    refused.assert_status(StatusCode::FORBIDDEN);
    let challenge = www_authenticate(&refused.0);
    assert!(
        challenge.contains(&format!(r#"scope="{}""#, constants::POSTS_READ)),
        "{challenge}"
    );

    let refused = crate::graphql::graphql(&app, &unrelated, "{ posts { id } }").await;
    assert_eq!(
        refused["errors"][0]["extensions"]["requiredScopes"],
        json!([constants::POSTS_READ.as_str()]),
        "{refused}"
    );
}

#[tokio::test]
async fn a_service_token_narrowed_to_reading_posts_is_challenged_on_the_audio_routes() {
    let app = boot().await;
    let reader = narrow_bearer(&app, ORG_ID, client(), &[constants::POSTS_READ]);

    let logs = LogCapture::install();
    let refusals = [
        app.http()
            .post("/audio/uploads")
            .header(header::AUTHORIZATION, &reader)
            .send()
            .await,
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &reader)
            .body_json(&json!({ "file": Uuid::now_v7() }))
            .send()
            .await,
    ];
    for refused in refusals {
        refused.assert_status(StatusCode::FORBIDDEN);
        let challenge = www_authenticate(&refused.0);
        assert!(
            challenge.contains(r#"error="insufficient_scope""#)
                && challenge.contains(&format!(r#"scope="{}""#, constants::AUDIO_TRANSCODE)),
            "the advertised scope gates what it names, and the refusal says so: {challenge}",
        );
    }

    let denied = logs.find("nest_rs::authz", "authorization denied");
    assert_eq!(denied.len(), 2, "one line per denial: {denied:#?}");
    for line in &denied {
        assert_eq!(line.field("reason").as_deref(), Some("insufficient_scope"));
        assert_eq!(line.field("subject").as_deref(), Some("Transcoding"));
    }
    logs.expect_none("nest_rs::layers", "guard denied the request");
}
