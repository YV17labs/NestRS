#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::let_underscore_must_use,
    reason = "a suite fails by panicking and discards what it does not assert; clippy.toml's allow-*-in-tests reaches #[test] bodies, not their helpers"
)]
mod audio;
mod audit;
mod graphql;
mod health;
mod http;
mod module;
mod openapi;
mod orgs;
mod posts;
mod schema;
mod scopes;
mod subscription;
mod users;

use api::ApiModule;
use features::{Role, Subject};
use nest_rs::http::{StatusCode, header};
use nest_rs::testing::{TestApp, TestAppBuilder, TestResponse};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) use features::testing::{
    ORG_ID, RedisDatabase, claims, client, connection, delete_transcode,
};

pub(crate) async fn boot() -> TestApp {
    boot_with(TestApp::builder()).await
}

pub(crate) async fn boot_on(redis: RedisDatabase) -> TestApp {
    boot_with(TestApp::builder().provide(redis.config())).await
}

async fn boot_with(builder: TestAppBuilder) -> TestApp {
    builder
        .module::<ApiModule>()
        .ephemeral_database::<migrations::Migrator>()
        .ephemeral_signing_key()
        .build()
        .await
        .expect("ApiModule boots against a throwaway database")
}

pub(crate) fn sign(app: &TestApp, org_id: Uuid, role: Role) -> String {
    app.bearer(&claims(org_id, role, client()))
}

pub(crate) fn sign_as(app: &TestApp, org_id: Uuid, role: Role, sub: Uuid) -> String {
    app.bearer(&claims(org_id, role, Subject::User(sub)))
}

pub(crate) async fn create_user(app: &TestApp, bearer: &str, name: &str, email: &str) -> String {
    create(
        app,
        bearer,
        "/users",
        json!({ "name": name, "email": email }),
    )
    .await
}

pub(crate) async fn create_post(app: &TestApp, bearer: &str, title: &str, body: &str) -> String {
    create(
        app,
        bearer,
        "/posts",
        json!({ "title": title, "body": body }),
    )
    .await
}

pub(crate) async fn create_org(app: &TestApp, bearer: &str, name: &str) -> Uuid {
    create(app, bearer, "/orgs", json!({ "name": name }))
        .await
        .parse()
        .expect("an org id")
}

async fn create(app: &TestApp, bearer: &str, collection: &str, body: Value) -> String {
    let resp = app
        .http()
        .post(collection)
        .header(header::AUTHORIZATION, bearer)
        .body_json(&body)
        .send()
        .await;
    resp.assert_status(StatusCode::CREATED);
    let location = location_of(&resp);
    let id = resp
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();
    assert_created_location(location.as_deref(), collection, &id);
    id
}

pub(crate) fn location_of(resp: &TestResponse) -> Option<String> {
    resp.0
        .headers()
        .get(header::LOCATION)
        .and_then(|value| value.to_str().ok())
        .map(str::to_owned)
}

#[track_caller]
pub(crate) fn assert_created_location(location: Option<&str>, collection: &str, id: &str) {
    assert_eq!(
        location,
        Some(format!("{collection}/{id}").as_str()),
        "a `201` carries the URI of the row it minted",
    );
}

pub(crate) async fn user_names(app: &TestApp, bearer: &str) -> Vec<String> {
    let listed = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, bearer)
        .send()
        .await;
    listed.assert_status_is_ok();
    listed
        .json()
        .await
        .value()
        .array()
        .iter()
        .map(|u| u.object().get("name").string().to_owned())
        .collect()
}
