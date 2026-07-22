use nest_rs_core::module;
use nest_rs_http::{HttpConfig, HttpModule};
use nest_rs_opentelemetry::OpenTelemetryModule;
use nest_rs_seaorm::DatabaseModule;
use nest_rs_testing::{EphemeralDatabase, TestApp};
use poem::http::{StatusCode, header};
use serde_json::json;
use uuid::Uuid;

use features::authn::AuthnModule;
use features::authz::AuthzHttpModule;
use features::identity::Role;
use features::orgs::ActiveModel as OrgActive;
use features::posts::{PostsHttpModule, publication};
use features::testing::{DEV_PUBLIC_KEY, token};
use features::users::{ActiveModel as UserActive, UserRole};
use nest_rs_authn::JwtConfig;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

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
        role: Set(UserRole::User),
        password_hash: Set(None),
        ..Default::default()
    }
    .insert(db.connection().as_ref())
    .await
    .expect("seed author");

    let bearer = token(org_id, vec![Role::User], Some(author_id));

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
    token(org_id, vec![Role::Admin], Some(Uuid::now_v7()))
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
    let machine = token(org_id, vec![Role::Admin], None);

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

// Count the audit rows a publish should write for a post — asserted directly
// against the connection, the same way this suite seeds its fixtures.
async fn publication_count(db: &EphemeralDatabase, post_id: Uuid) -> usize {
    publication::Entity::find()
        .filter(publication::Column::PostId.eq(post_id))
        .all(db.connection().as_ref())
        .await
        .expect("query the publication audit log")
        .len()
}

#[tokio::test]
async fn publish_writes_the_status_and_an_audit_row_atomically() {
    let (db, app, bearer, org_id) = boot().await;

    let created = app
        .http()
        .post("/posts")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body_json(&json!({ "title": "Draft", "body": "World" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let id_str = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();
    let id = Uuid::parse_str(&id_str).expect("valid post uuid");

    assert_eq!(
        publication_count(&db, id).await,
        0,
        "a fresh draft has no audit row yet",
    );

    let published = app
        .http()
        .post(format!("/posts/{id_str}/publish"))
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
        "published",
    );

    // Both writes landed: the status flipped AND exactly one audit row exists.
    assert_eq!(
        publication_count(&db, id).await,
        1,
        "publish wrote exactly one audit row alongside the status update",
    );
}

#[tokio::test]
async fn a_failing_audit_insert_rolls_back_the_status_update() {
    let (db, app, bearer, org_id) = boot().await;

    let created = app
        .http()
        .post("/posts")
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .body_json(&json!({ "title": "Draft", "body": "World" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let id_str = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();
    let id = Uuid::parse_str(&id_str).expect("valid post uuid");

    // Pre-insert a conflicting audit row for this fresh draft. The post is not
    // yet published, so `publish` clears the already-published check and reaches
    // the SECOND write — where the unique `post_id` constraint rejects the
    // duplicate with a real Postgres error (no DB mocking).
    publication::ActiveModel {
        id: Set(Uuid::now_v7()),
        post_id: Set(id),
        actor_id: Set(Uuid::now_v7()),
        published_at: Set(chrono::Utc::now().fixed_offset()),
    }
    .insert(db.connection().as_ref())
    .await
    .expect("seed the conflicting publication row");

    let conflict = app
        .http()
        .post(format!("/posts/{id_str}/publish"))
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", admin_bearer(org_id)),
        )
        .send()
        .await;
    // The audit insert failed, so the request errors.
    conflict.assert_status(StatusCode::INTERNAL_SERVER_ERROR);

    // The first write rolled back with it: the post is still a draft.
    let got = app
        .http()
        .get(format!("/posts/{id_str}"))
        .header(header::AUTHORIZATION, format!("Bearer {bearer}"))
        .send()
        .await;
    got.assert_status_is_ok();
    assert_eq!(
        got.json().await.value().object().get("status").string(),
        "draft",
        "the failed second write rolled back the status update",
    );

    // And no orphan: only the pre-seeded row remains, the rolled-back insert
    // left nothing behind.
    assert_eq!(
        publication_count(&db, id).await,
        1,
        "the rolled-back publish added no audit row",
    );
}
