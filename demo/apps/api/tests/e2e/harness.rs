use api::ApiModule;
use nest_rs_authn::JwtConfig;
use nest_rs_testing::{EphemeralDatabase, TestApp};
use poem::http::header;
use serde_json::json;
use uuid::Uuid;

pub(crate) use features::testing::{DEV_PUBLIC_KEY, ORG_ID};

pub(crate) async fn boot() -> (EphemeralDatabase, TestApp) {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<ApiModule>()
        .with_test_telemetry()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("ApiModule boots against the throwaway database");
    (db, app)
}

pub(crate) async fn login() -> String {
    token_for(ORG_ID, "admin").await
}

pub(crate) async fn token_for(org_id: &str, role: &str) -> String {
    features::testing::token_for(org_id, role, None)
}

pub(crate) async fn token_with_sub(org_id: &str, role: &str, sub: Uuid) -> String {
    features::testing::token_for(org_id, role, Some(sub))
}

pub(crate) async fn create_user(app: &TestApp, bearer: &str, name: &str, email: &str) -> String {
    let resp = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, bearer)
        .body_json(&json!({ "name": name, "email": email }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned()
}

pub(crate) async fn create_post(app: &TestApp, bearer: &str, title: &str, body: &str) -> String {
    let resp = app
        .http()
        .post("/posts")
        .header(header::AUTHORIZATION, bearer)
        .body_json(&json!({ "title": title, "body": body }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned()
}

pub(crate) async fn create_org(app: &TestApp, bearer: &str, name: &str) -> String {
    let resp = app
        .http()
        .post("/orgs")
        .header(header::AUTHORIZATION, bearer)
        .body_json(&json!({ "name": name }))
        .send()
        .await;
    resp.assert_status_is_ok();
    resp.json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned()
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
