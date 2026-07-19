use nest_rs_core::module;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_seaorm::DatabaseModule;
use nest_rs_testing::{EphemeralDatabase, TestApp};
use poem::http::{StatusCode, header};
use serde_json::json;
use uuid::Uuid;

use features::Claims;
use features::authn::AuthnModule;
use features::authz::AuthzHttpModule;
use features::identity::Role;
use features::orgs::ActiveModel as OrgActive;
use features::posts::PostsHttpModule;
use features::users::ActiveModel as UserActive;
use nest_rs_authn::{JwtConfig, JwtOptions, JwtService};
use sea_orm::{ActiveModelTrait, Set};

const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

#[module(
    imports = [
        OpenTelemetryModule,
        DatabaseModule::for_root(None),
        HttpModule::for_root(HttpConfig { port: 3005, ..Default::default() }),
        AuthnModule,
        AuthzHttpModule,
        PostsHttpModule,
    ],
)]
struct PostsHttpTestModule;

async fn boot() -> (EphemeralDatabase, TestApp, String, Uuid) {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let org_id = Uuid::now_v7();
    let author_id = Uuid::now_v7();
    OrgActive {
        id: Set(org_id),
        name: Set("Acme".into()),
        ..Default::default()
    }
    .insert(db.connection().as_ref())
    .await
    .expect("seed org");
    UserActive {
        id: Set(author_id),
        org_id: Set(org_id),
        name: Set("Ada".into()),
        email: Set("ada@acme.test".into()),
        role: Set("user".into()),
        password_hash: Set(None),
        ..Default::default()
    }
    .insert(db.connection().as_ref())
    .await
    .expect("seed author");

    let jwt = JwtService::new(JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY))
        .expect("the dev keypair parses");
    let bearer = jwt
        .sign(&Claims {
            sub: Some(author_id),
            org_id,
            roles: vec![Role::User],
            exp: jwt.expiry(),
        })
        .expect("sign the test token");

    let app = TestApp::builder()
        .module::<PostsHttpTestModule>()
        .with_test_telemetry()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("PostsHttpTestModule boots against the throwaway database");
    (db, app, bearer, org_id)
}

fn admin_bearer(org_id: Uuid) -> String {
    let jwt = JwtService::new(JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY))
        .expect("the dev keypair parses");
    jwt.sign(&Claims {
        sub: Some(Uuid::now_v7()),
        org_id,
        roles: vec![Role::Admin],
        exp: jwt.expiry(),
    })
    .expect("sign admin token")
}

#[tokio::test]
async fn posts_round_trip() {
    let (_db, app, bearer, _org_id) = boot().await;
    let auth = format!("Bearer {bearer}");

    let created = app
        .http()
        .post("/posts")
        .header(header::AUTHORIZATION, &auth)
        .body_json(&json!({ "title": "Hello", "body": "World" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let body = created.json().await;
    let id = body.value().object().get("id").string().to_owned();
    assert_eq!(body.value().object().get("title").string(), "Hello");

    let got = app
        .http()
        .get(format!("/posts/{id}"))
        .header(header::AUTHORIZATION, &auth)
        .send()
        .await;
    got.assert_status_is_ok();
    assert_eq!(
        got.json().await.value().object().get("body").string(),
        "World"
    );
}

#[tokio::test]
async fn create_without_a_subject_returns_403() {
    let (_db, app, _, _) = boot().await;
    let org_id = Uuid::now_v7();
    let jwt = JwtService::new(JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY))
        .expect("the dev keypair parses");
    let machine = jwt
        .sign(&Claims {
            sub: None,
            org_id,
            roles: vec![Role::Admin],
            exp: jwt.expiry(),
        })
        .expect("sign machine token");

    app.http()
        .post("/posts")
        .header(header::AUTHORIZATION, format!("Bearer {machine}"))
        .body_json(&json!({ "title": "Hello", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn create_with_empty_title_returns_400() {
    let (_db, app, bearer, _org_id) = boot().await;
    app.http()
        .post("/posts")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body_json(&json!({ "title": "", "body": "World" }))
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn audit_interceptor_is_transparent_to_posts_requests() {
    let (_db, app, bearer, _org_id) = boot().await;
    let auth = format!("Bearer {bearer}");

    let created = app
        .http()
        .post("/posts")
        .header(header::AUTHORIZATION, &auth)
        .body_json(&json!({ "title": "Audited", "body": "World" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let id = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    app.http()
        .get(format!("/posts/{id}"))
        .header(header::AUTHORIZATION, &auth)
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn publish_transitions_a_draft_to_published() {
    let (_db, app, bearer, org_id) = boot().await;

    let created = app
        .http()
        .post("/posts")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body_json(&json!({ "title": "Draft", "body": "World" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let body = created.json().await;
    let id = body.value().object().get("id").string().to_owned();
    assert_eq!(body.value().object().get("status").string(), "draft");

    let published = app
        .http()
        .post(format!("/posts/{id}/publish"))
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", admin_bearer(org_id)),
        )
        .send()
        .await;
    published.assert_status_is_ok();
    assert_eq!(
        published
            .json()
            .await
            .value()
            .object()
            .get("status")
            .string(),
        "published"
    );
}

#[tokio::test]
async fn re_publishing_returns_rfc9457_problem_json() {
    let (_db, app, bearer, org_id) = boot().await;

    let created = app
        .http()
        .post("/posts")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body_json(&json!({ "title": "Draft", "body": "World" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let id = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    let admin = format!("Bearer {}", admin_bearer(org_id));
    app.http()
        .post(format!("/posts/{id}/publish"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await
        .assert_status_is_ok();

    let conflict = app
        .http()
        .post(format!("/posts/{id}/publish"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await;
    conflict.assert_status(StatusCode::CONFLICT);
    conflict.assert_header(header::CONTENT_TYPE, "application/problem+json");

    let body = conflict.json().await;
    let problem = body.value().object();
    assert_eq!(problem.get("status").i64(), 409);
    assert_eq!(problem.get("title").string(), "Post already published");
    assert_eq!(
        problem.get("type").string(),
        "https://nestrs.dev/problems/post-already-published"
    );
    assert!(
        problem.get("detail").string().contains("already published"),
        "detail should describe the conflict"
    );
}
