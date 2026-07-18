use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use api::ApiModule;
use features::audio::AudioQueueModule;
use features::notifications::NotificationsQueueModule;
use features::{Claims, Role};
use nest_rs_authn::{JwtConfig, JwtOptions, JwtService};
use nest_rs_authz::{AbilityBuilder, Action, with_ability};
use nest_rs_core::module;
use nest_rs_http::HttpTransport;
use nest_rs_redis::{QueueModule, QueueWorker, QueueWorkerModule};
use nest_rs_seaorm::{DatabaseModule, Executor, Repo, with_executor};
use nest_rs_storage::{Storage, StorageConfig};
use nest_rs_testing::{EphemeralDatabase, TestApp};
use poem::http::{StatusCode, header};
use poem::test::{TestForm, TestFormField};
use sea_orm::{EntityTrait, IntoActiveModel, Set};
use serde_json::json;
use uuid::Uuid;

const ORG_ID: &str = "018f0000-0000-7000-8000-000000000000";

const DEV_PRIVATE_KEY: &str = "-----BEGIN PRIVATE KEY-----\nMC4CAQAwBQYDK2VwBCIEIEYTRN4vmCuIfaUslO5G9pKyxkDJn3q3t9WDHo2FCfw3\n-----END PRIVATE KEY-----\n";
const DEV_PUBLIC_KEY: &str = "-----BEGIN PUBLIC KEY-----\nMCowBQYDK2VwAyEAHfPOjd2Y3m1BLM5nBJBMZFAlfWt69WL1NY8XyYeGfeo=\n-----END PUBLIC KEY-----\n";

async fn boot() -> (EphemeralDatabase, TestApp) {
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

async fn login() -> String {
    token_for(ORG_ID, "admin").await
}

async fn token_for(org_id: &str, role: &str) -> String {
    let jwt = JwtService::new(JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY))
        .expect("the dev keypair parses");
    let roles = match role {
        "admin" => vec![Role::Admin],
        _ => vec![Role::User],
    };
    jwt.sign(&Claims {
        sub: None,
        org_id: Uuid::parse_str(org_id).expect("valid org uuid"),
        roles,
        exp: jwt.expiry(),
    })
    .expect("sign the test token")
}

/// Sign a token carrying a `sub` — required to author a post (`PostAuthorGuard`
/// rejects a subject-less machine token on `POST /posts`).
async fn token_with_sub(org_id: &str, role: &str, sub: Uuid) -> String {
    let jwt = JwtService::new(JwtOptions::eddsa(DEV_PRIVATE_KEY, DEV_PUBLIC_KEY))
        .expect("the dev keypair parses");
    let roles = match role {
        "admin" => vec![Role::Admin],
        _ => vec![Role::User],
    };
    jwt.sign(&Claims {
        sub: Some(sub),
        org_id: Uuid::parse_str(org_id).expect("valid org uuid"),
        roles,
        exp: jwt.expiry(),
    })
    .expect("sign the test token")
}

async fn create_user(app: &TestApp, bearer: &str, name: &str, email: &str) -> String {
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

async fn create_post(app: &TestApp, bearer: &str, title: &str, body: &str) -> String {
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

async fn create_org(app: &TestApp, bearer: &str, name: &str) -> String {
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

async fn user_names(app: &TestApp, bearer: &str) -> Vec<String> {
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

#[tokio::test]
async fn health_live_probe_is_ok() {
    let (_db, app) = boot().await;
    app.http()
        .get("/health/live")
        .send()
        .await
        .assert_status_is_ok();
}

#[tokio::test]
async fn health_ready_probe_reports_db_indicator_up() {
    let (_db, app) = boot().await;
    app.init()
        .await
        .expect("lifecycle init wires the indicator registry");
    let resp = app.http().get("/health/ready").send().await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    let body = body.value().object();
    assert_eq!(body.get("status").string(), "up");
    assert!(
        body.get("info").object().get_opt("db").is_some(),
        "ready probe info bucket carries the `db` indicator",
    );
    assert!(
        body.get("error").object().is_empty(),
        "ready probe error bucket is empty against a live database",
    );
}

#[tokio::test]
async fn openapi_document_describes_the_routes() {
    let (_db, app) = boot().await;
    let resp = app.http().get("/api-json").send().await;
    resp.assert_status_is_ok();
    let bytes = resp.0.into_body().into_bytes().await.expect("body");
    let doc: serde_json::Value = serde_json::from_slice(&bytes).expect("api-json is JSON");

    let paths = doc["paths"].as_object().expect("paths object");
    assert!(paths.contains_key("/orgs"), "paths include /orgs");
    assert!(paths.contains_key("/users"), "paths include /users");

    // Security scheme declared, and applied to the guarded list route.
    assert_eq!(
        doc["components"]["securitySchemes"]["bearerAuth"]["scheme"], "bearer",
        "bearerAuth security scheme is declared",
    );
    assert_eq!(
        doc["paths"]["/orgs"]["get"]["security"][0]["bearerAuth"],
        json!([]),
        "the guarded list route requires bearerAuth",
    );

    // #[crud] pagination surfaces as query parameters.
    let params = doc["paths"]["/orgs"]["get"]["parameters"]
        .as_array()
        .expect("list op has parameters");
    let query_names: Vec<&str> = params
        .iter()
        .filter(|p| p["in"] == "query")
        .filter_map(|p| p["name"].as_str())
        .collect();
    assert!(
        query_names.contains(&"first") && query_names.contains(&"after"),
        "pagination cursor is documented as query params: {query_names:?}",
    );

    // Path param carries its real type, not a bare string.
    assert_eq!(
        doc["paths"]["/orgs/{id}"]["get"]["parameters"][0]["schema"]["format"], "uuid",
        "the :id path param is typed uuid",
    );

    // Error responses are the RFC 9457 problem+json shape.
    let create = &doc["paths"]["/orgs"]["post"]["responses"];
    for status in ["401", "403", "409", "422"] {
        assert_eq!(
            create[status]["content"]["application/problem+json"]["schema"]["$ref"],
            "#/components/schemas/ProblemDetails",
            "create advertises a problem+json {status} response",
        );
    }
}

// A client that advertises `Accept-Encoding: gzip` gets a gzip-encoded body;
// one that does not gets plain JSON. Proves the transport's compression layer
// (`HttpConfig.compression`, on for the api app) negotiates per request. The
// test harness drives a bare default transport, so the compression-on transport
// is supplied explicitly — the same knob `HttpModule` flips from the config.
#[tokio::test]
async fn responses_are_gzip_compressed_when_the_client_accepts_it() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let app = TestApp::builder()
        .module::<ApiModule>()
        .http(HttpTransport::new().compression(true))
        .with_test_telemetry()
        .provide_arc(db.connection())
        .provide(JwtConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("ApiModule boots on a compression-enabled transport");
    let _db = db;
    let bearer = format!("Bearer {}", login().await);

    let compressed = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &bearer)
        .header(header::ACCEPT_ENCODING, "gzip")
        .send()
        .await;
    compressed.assert_status_is_ok();
    compressed.assert_header(header::CONTENT_ENCODING, "gzip");

    let plain = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    plain.assert_status_is_ok();
    plain.assert_header_is_not_exist(header::CONTENT_ENCODING);
}

#[tokio::test]
async fn protected_route_rejects_a_missing_or_bogus_bearer_token() {
    let (_db, app) = boot().await;

    app.http()
        .get("/orgs")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    app.http()
        .get("/orgs")
        .header(header::AUTHORIZATION, "Bearer not-a-real-jwt")
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);
}

/// Every modelled HTTP failure — a guard denial (401), an edge-validation
/// rejection (400), a not-found (404), and a conflict (409) — comes back on the
/// single RFC-9457 `application/problem+json` envelope with `type`/`title`/
/// `status`. The acceptance criterion for the one-error-format unification.
#[tokio::test]
async fn every_modelled_failure_returns_rfc9457_problem_json() {
    let (_db, app) = boot().await;
    let bearer = format!("Bearer {}", login().await);

    // 401 — a guard denial (no bearer token) on a protected route.
    let unauthorized = app.http().get("/orgs").send().await;
    unauthorized.assert_status(StatusCode::UNAUTHORIZED);
    unauthorized.assert_header(header::CONTENT_TYPE, "application/problem+json");
    let body = unauthorized.json().await;
    let problem = body.value().object();
    assert_eq!(problem.get("status").i64(), 401);
    assert_eq!(problem.get("title").string(), "Unauthorized");
    assert!(
        problem.get_opt("type").is_some(),
        "problem carries a type URI"
    );

    // 400 — a malformed path id rejected by the edge pipe before the handler.
    let bad_request = app
        .http()
        .get("/users/not-a-uuid")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    bad_request.assert_status(StatusCode::BAD_REQUEST);
    bad_request.assert_header(header::CONTENT_TYPE, "application/problem+json");
    assert_eq!(
        bad_request
            .json()
            .await
            .value()
            .object()
            .get("status")
            .i64(),
        400,
    );

    // 404 — a well-formed but absent id, mapped from `ServiceError::NotFound`.
    let not_found = app
        .http()
        .get("/users/018f0000-0000-7000-8000-0000000000ff")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    not_found.assert_status(StatusCode::NOT_FOUND);
    not_found.assert_header(header::CONTENT_TYPE, "application/problem+json");
    assert_eq!(
        not_found.json().await.value().object().get("status").i64(),
        404,
    );

    // 409 — a duplicate unique org name, mapped from `ServiceError::Conflict`.
    create_org(&app, &bearer, "Conflict Co").await;
    let conflict = app
        .http()
        .post("/orgs")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "name": "Conflict Co" }))
        .send()
        .await;
    conflict.assert_status(StatusCode::CONFLICT);
    conflict.assert_header(header::CONTENT_TYPE, "application/problem+json");
    let conflict_body = conflict.json().await;
    let conflict_problem = conflict_body.value().object();
    assert_eq!(conflict_problem.get("status").i64(), 409);
    assert_eq!(conflict_problem.get("title").string(), "Conflict");
}

#[tokio::test]
async fn create_org_persists_and_is_listed_with_a_bearer_token() {
    let (_db, app) = boot().await;
    let token = login().await;
    let bearer = format!("Bearer {token}");
    let name = "Acme E2E";

    let created = app
        .http()
        .post("/orgs")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "name": name }))
        .send()
        .await;
    created.assert_status_is_ok();
    let created_json = created.json().await;
    assert_eq!(created_json.value().object().get("name").string(), name);

    let listed = app
        .http()
        .get("/orgs")
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    listed.assert_status_is_ok();
    let names: Vec<String> = listed
        .json()
        .await
        .value()
        .array()
        .iter()
        .map(|org| org.object().get("name").string().to_owned())
        .collect();
    assert!(
        names.contains(&name.to_string()),
        "the freshly created org appears in the list: {names:?}",
    );
}

#[tokio::test]
async fn users_are_scoped_to_their_org_and_bound_by_id() {
    let (_db, app) = boot().await;

    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "Acme").await;
    let org_b = create_org(&app, &bootstrap, "Globex").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!("Bearer {}", token_for(&org_b, "admin").await);

    let created = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Ada", "email": "ada@acme.test" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let user_a = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    assert!(
        user_names(&app, &token_b).await.is_empty(),
        "org B sees none of org A's users",
    );

    app.http()
        .get(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    app.http()
        .get("/users/018f0000-0000-7000-8000-0000000000ff")
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
    app.http()
        .get("/users/not-a-uuid")
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::BAD_REQUEST);

    assert_eq!(user_names(&app, &token_a).await, vec!["Ada".to_string()]);
    let got = app
        .http()
        .get(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .send()
        .await;
    got.assert_status_is_ok();
    assert_eq!(
        got.json().await.value().object().get("name").string(),
        "Ada"
    );
}

mod user_row {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "user")]
    pub struct Model {
        #[sea_orm(primary_key, auto_increment = false)]
        pub id: Uuid,
        pub org_id: Uuid,
        pub name: String,
        pub email: String,
        pub role: String,
        pub password_hash: Option<String>,
        pub created_at: DateTimeWithTimeZone,
        pub updated_at: DateTimeWithTimeZone,
        pub deleted_at: Option<DateTimeWithTimeZone>,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

#[tokio::test]
async fn writes_are_scoped_to_the_callers_ability() {
    let (db, app) = boot().await;

    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "Acme Writes").await;
    let org_b = create_org(&app, &bootstrap, "Globex Writes").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!("Bearer {}", token_for(&org_b, "admin").await);
    let org_b_id = Uuid::parse_str(&org_b).expect("valid org uuid");

    let created = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Ada", "email": "ada-writes@acme.test" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let user_a = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();
    let user_a_id = Uuid::parse_str(&user_a).expect("valid user uuid");

    let patched = app
        .http()
        .patch(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Ada L.", "email": "ada-writes@acme.test" }))
        .send()
        .await;
    patched.assert_status_is_ok();
    assert_eq!(
        patched.json().await.value().object().get("name").string(),
        "Ada L."
    );

    app.http()
        .patch(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_b)
        .body_json(&json!({ "name": "Hijacked", "email": "ada-writes@acme.test" }))
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    app.http()
        .delete(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_b)
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);

    let conn = db.connection();
    let blocked = Arc::new({
        let mut b = AbilityBuilder::new();
        b.can(Action::Manage, user_row::Entity)
            .when(move |p| p.eq(user_row::Column::OrgId, org_b_id));
        b.build().expect("valid test ability")
    });
    let (update, delete) = with_executor(
        Executor::Pool((*conn).clone()),
        with_ability(blocked, async move {
            let model = user_row::Entity::find_by_id(user_a_id)
                .one(&*conn)
                .await
                .expect("load user A directly")
                .expect("user A exists");
            let mut active = model.clone().into_active_model();
            active.name = Set("Hacked".to_owned());
            let update = Repo::<user_row::Entity>::update(active).await;
            let delete = Repo::<user_row::Entity>::delete(model).await;
            (update, delete)
        }),
    )
    .await;
    assert!(
        matches!(update, Err(sea_orm::DbErr::RecordNotUpdated)),
        "an out-of-scope update touches no row: {update:?}",
    );
    let delete = delete.expect("a delete query runs");
    assert_eq!(
        delete.rows_affected, 0,
        "an out-of-scope delete removes no row",
    );

    let survivor = user_row::Entity::find_by_id(user_a_id)
        .one(&*db.connection())
        .await
        .expect("re-read user A")
        .expect("user A still exists");
    assert_eq!(survivor.name, "Ada L.", "the row was never mutated");

    app.http()
        .delete(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .send()
        .await
        .assert_status(StatusCode::NO_CONTENT);
    app.http()
        .get(format!("/users/{user_a}"))
        .header(header::AUTHORIZATION, &token_a)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);

    let tombstone = user_row::Entity::find_by_id(user_a_id)
        .one(&*db.connection())
        .await
        .expect("re-read user A directly")
        .expect("soft-deleted user row remains in the database");
    assert!(
        tombstone.deleted_at.is_some(),
        "delete stamps deleted_at instead of removing the row",
    );
}

#[tokio::test]
async fn a_plain_user_get_by_id_masks_the_email() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &bootstrap, "Initech").await;

    app.http()
        .post("/users")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_for(&org, "admin").await),
        )
        .body_json(&json!({ "name": "Bob", "email": "bob@initech.test" }))
        .send()
        .await
        .assert_status_is_ok();

    let listed = app
        .http()
        .get("/users")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_for(&org, "admin").await),
        )
        .send()
        .await;
    listed.assert_status_is_ok();
    let user_id = listed
        .json()
        .await
        .value()
        .array()
        .iter()
        .next()
        .expect("one user")
        .object()
        .get("id")
        .string()
        .to_owned();

    let user = format!("Bearer {}", token_for(&org, "user").await);
    let got = app
        .http()
        .get(format!("/users/{user_id}"))
        .header(header::AUTHORIZATION, &user)
        .send()
        .await;
    got.assert_status_is_ok();
    let json = got.json().await;
    let body = json.value().object();
    assert_eq!(body.get("name").string(), "Bob");
    assert!(
        body.get_opt("email").is_none(),
        "a plain user's GET by id masks the email field",
    );
}

#[tokio::test]
async fn a_plain_user_listing_masks_the_email() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &bootstrap, "Initech").await;

    app.http()
        .post("/users")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token_for(&org, "admin").await),
        )
        .body_json(&json!({ "name": "Bob", "email": "bob@initech.test" }))
        .send()
        .await
        .assert_status_is_ok();

    let user = format!("Bearer {}", token_for(&org, "user").await);
    let listed = app
        .http()
        .get("/users")
        .header(header::AUTHORIZATION, &user)
        .send()
        .await;
    listed.assert_status_is_ok();
    let body = listed.json().await;
    let first = body
        .value()
        .array()
        .iter()
        .next()
        .expect("one user")
        .object();
    assert_eq!(first.get("name").string(), "Bob");
    assert!(
        first.get_opt("email").is_none(),
        "a plain user's listing masks the email field",
    );
}

#[tokio::test]
async fn a_failed_mutation_persists_nothing() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &bootstrap, "Hooli").await;
    let admin = format!("Bearer {}", token_for(&org, "admin").await);

    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "Ada", "email": "dup@hooli.test" }))
        .send()
        .await
        .assert_status_is_ok();

    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "Grace", "email": "dup@hooli.test" }))
        .send()
        .await
        .assert_status(StatusCode::CONFLICT);

    assert_eq!(user_names(&app, &admin).await, vec!["Ada".to_string()]);
}

#[tokio::test]
async fn orgs_admin_sees_all_but_a_plain_user_is_scoped_to_its_own() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_x = create_org(&app, &admin, "OrgX").await;
    let org_y = create_org(&app, &admin, "OrgY").await;

    let admin_list = app
        .http()
        .get("/orgs")
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await;
    admin_list.assert_status_is_ok();
    let admin_names: Vec<String> = admin_list
        .json()
        .await
        .value()
        .array()
        .iter()
        .map(|o| o.object().get("name").string().to_owned())
        .collect();
    assert!(
        admin_names.contains(&"OrgX".to_string()) && admin_names.contains(&"OrgY".to_string()),
        "the admin sees every org: {admin_names:?}",
    );

    let user_x = format!("Bearer {}", token_for(&org_x, "user").await);
    let user_list = app
        .http()
        .get("/orgs")
        .header(header::AUTHORIZATION, &user_x)
        .send()
        .await;
    user_list.assert_status_is_ok();
    let user_names: Vec<String> = user_list
        .json()
        .await
        .value()
        .array()
        .iter()
        .map(|o| o.object().get("name").string().to_owned())
        .collect();
    assert_eq!(user_names, vec!["OrgX".to_string()]);

    app.http()
        .get(format!("/orgs/{org_y}"))
        .header(header::AUTHORIZATION, &user_x)
        .send()
        .await
        .assert_status(StatusCode::FORBIDDEN);
    let got = app
        .http()
        .get(format!("/orgs/{org_x}"))
        .header(header::AUTHORIZATION, &user_x)
        .send()
        .await;
    got.assert_status_is_ok();
    assert_eq!(
        got.json().await.value().object().get("name").string(),
        "OrgX"
    );
}

#[tokio::test]
async fn graphql_requires_a_jwt_and_scopes_to_the_callers_org() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &admin, "GqlAcme").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!(
        "Bearer {}",
        token_for(&create_org(&app, &admin, "GqlGlobex").await, "admin").await
    );

    let created = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "name": "Gql Ada", "email": "gqlada@acme.test" }))
        .send()
        .await;
    created.assert_status_is_ok();
    let user_a = created
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    let query = json!({ "query": "{ users { name } }" });

    let anon = app.http().post("/graphql").body_json(&query).send().await;
    anon.assert_status_is_ok();
    assert!(
        anon.json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "an anonymous GraphQL query is rejected",
    );

    let b = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_b)
        .body_json(&query)
        .send()
        .await;
    b.assert_status_is_ok();
    let b_users = b.json().await;
    let b_names: Vec<String> = b_users
        .value()
        .object()
        .get("data")
        .object()
        .get("users")
        .array()
        .iter()
        .map(|u| u.object().get("name").string().to_owned())
        .collect();
    assert!(
        b_names.is_empty(),
        "org B sees no users in GraphQL: {b_names:?}"
    );

    let a = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&query)
        .send()
        .await;
    a.assert_status_is_ok();
    let a_users = a.json().await;
    let a_names: Vec<String> = a_users
        .value()
        .object()
        .get("data")
        .object()
        .get("users")
        .array()
        .iter()
        .map(|u| u.object().get("name").string().to_owned())
        .collect();
    assert_eq!(a_names, vec!["Gql Ada".to_string()]);

    let by_id = json!({ "query": format!("{{ user(id: \"{user_a}\") {{ name }} }}") });
    let a_one = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&by_id)
        .send()
        .await;
    a_one.assert_status_is_ok();
    assert_eq!(
        a_one
            .json()
            .await
            .value()
            .object()
            .get("data")
            .object()
            .get("user")
            .object()
            .get("name")
            .string(),
        "Gql Ada",
    );
    let b_one = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_b)
        .body_json(&by_id)
        .send()
        .await;
    b_one.assert_status_is_ok();
    assert!(
        b_one
            .json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "org B is forbidden org A's user by id",
    );
}

#[tokio::test]
async fn graphql_auto_resolved_relations_respect_ability_scope() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &admin, "RelA").await;
    let org_b = create_org(&app, &admin, "RelB").await;
    let token_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let token_b = format!("Bearer {}", token_for(&org_b, "admin").await);

    for (tok, email) in [
        (&token_a, "ada@rel.test"),
        (&token_a, "bea@rel.test"),
        (&token_b, "leak@rel.test"),
    ] {
        app.http()
            .post("/users")
            .header(header::AUTHORIZATION, tok)
            .body_json(&json!({ "name": "Twin", "email": email }))
            .send()
            .await
            .assert_status_is_ok();
    }

    let resp = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "query": "{ users { id org { id } } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "graphql response must not contain errors",
    );
    let users_a = body
        .value()
        .object()
        .get("data")
        .object()
        .get("users")
        .array();
    assert!(
        users_a.iter().count() >= 2,
        "org A must see its two seeded users (got {})",
        users_a.iter().count(),
    );
    for u in users_a.iter() {
        let org_id = u.object().get("org").object().get("id").string();
        assert_eq!(
            org_id, org_a,
            "auto-resolved org must be caller's: {org_id}"
        );
    }

    let resp = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token_a)
        .body_json(&json!({ "query": "{ orgs { id users { email } } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "graphql response must not contain errors",
    );
    let mut seen: Vec<String> = Vec::new();
    for org in body
        .value()
        .object()
        .get("data")
        .object()
        .get("orgs")
        .array()
        .iter()
    {
        for u in org.object().get("users").array().iter() {
            seen.push(u.object().get("email").string().to_owned());
        }
    }
    assert!(
        seen.iter().any(|e| e == "ada@rel.test"),
        "org A's own users must surface through the HasMany resolver: {seen:?}",
    );
    assert!(
        !seen.contains(&"leak@rel.test".to_string()),
        "org B's user must not leak through Org.users: {seen:?}",
    );
}

#[tokio::test]
async fn a_duplicate_email_create_is_a_conflict_not_a_500() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    // Auto-generated `#[crud]` create (orgs): a duplicate unique name is a 409,
    // not the blanket 500 the generated create used to return.
    create_org(&app, &admin, "SameName").await;
    app.http()
        .post("/orgs")
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "SameName" }))
        .send()
        .await
        .assert_status(StatusCode::CONFLICT);

    // Manual create handler (users) delegating to the service: the service maps
    // the unique-email violation to a `Conflict` rather than an opaque `Db` 500.
    let org = create_org(&app, &admin, "Conflict").await;
    let token = format!("Bearer {}", token_for(&org, "admin").await);
    let body = json!({ "name": "Dup", "email": "dup@conflict.test" });
    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &token)
        .body_json(&body)
        .send()
        .await
        .assert_status_is_ok();
    app.http()
        .post("/users")
        .header(header::AUTHORIZATION, &token)
        .body_json(&body)
        .send()
        .await
        .assert_status(StatusCode::CONFLICT);
}

#[tokio::test]
async fn has_many_relation_load_is_capped_at_relation_load_cap() {
    use sea_orm::ConnectionTrait;

    let (db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org = create_org(&app, &admin, "Fanout").await;
    let token = format!("Bearer {}", token_for(&org, "admin").await);

    // An author in that org to satisfy the post's author FK.
    let author_resp = app
        .http()
        .post("/users")
        .header(header::AUTHORIZATION, &token)
        .body_json(&json!({ "name": "Author", "email": "fanout-author@rel.test" }))
        .send()
        .await;
    author_resp.assert_status_is_ok();
    let author = author_resp
        .json()
        .await
        .value()
        .object()
        .get("id")
        .string()
        .to_owned();

    // Seed more children under one parent than the relation cap allows.
    let seeded = nest_rs_seaorm::RELATION_LOAD_CAP + 5;
    let rows: Vec<String> = (0..seeded)
        .map(|i| format!("('{}','{org}','{author}','t{i}','b{i}')", Uuid::now_v7()))
        .collect();
    db.connection()
        .execute_unprepared(&format!(
            "INSERT INTO post (id, org_id, author_id, title, body) VALUES {}",
            rows.join(", "),
        ))
        .await
        .expect("bulk insert posts");

    let resp = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &token)
        .body_json(&json!({ "query": "{ orgs { id posts { id } } }" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "graphql response must not contain errors",
    );
    let loaded = body
        .value()
        .object()
        .get("data")
        .object()
        .get("orgs")
        .array()
        .iter()
        .find(|o| o.object().get("id").string() == org.as_str())
        .expect("the seeded org is present in the response")
        .object()
        .get("posts")
        .array()
        .iter()
        .count() as u64;
    assert_eq!(
        loaded,
        nest_rs_seaorm::RELATION_LOAD_CAP,
        "an exposed has_many load is bounded at RELATION_LOAD_CAP, not the {seeded} seeded",
    );
}

#[tokio::test]
async fn crud_generated_update_and_delete_round_trip() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    let id = create_org(&app, &admin, "Before").await;

    let patched = app
        .http()
        .patch(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .body_json(&json!({ "name": "After" }))
        .send()
        .await;
    patched.assert_status_is_ok();
    assert_eq!(
        patched.json().await.value().object().get("name").string(),
        "After"
    );

    let got = app
        .http()
        .get(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await;
    got.assert_status_is_ok();
    assert_eq!(
        got.json().await.value().object().get("name").string(),
        "After"
    );

    app.http()
        .delete(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await
        .assert_status(StatusCode::NO_CONTENT);

    app.http()
        .get(format!("/orgs/{id}"))
        .header(header::AUTHORIZATION, &admin)
        .send()
        .await
        .assert_status(StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn crud_cursor_pagination_walks_the_collection_in_order() {
    let (_db, app) = boot().await;
    let admin = format!("Bearer {}", token_for(ORG_ID, "admin").await);

    let mut created = Vec::new();
    for i in 0..5 {
        created.push(create_org(&app, &admin, &format!("Page{i}")).await);
    }

    let mut seen: Vec<String> = Vec::new();
    let mut after: Option<String> = None;
    let mut first_page = true;
    loop {
        let path = match &after {
            Some(cursor) => format!("/orgs?first=2&after={cursor}"),
            None => "/orgs?first=2".to_string(),
        };
        let resp = app
            .http()
            .get(&path)
            .header(header::AUTHORIZATION, &admin)
            .send()
            .await;
        resp.assert_status_is_ok();
        if first_page {
            resp.assert_header_exist("x-next-cursor");
            first_page = false;
        }
        let body = resp.json().await;
        let page: Vec<String> = body
            .value()
            .array()
            .iter()
            .map(|o| o.object().get("id").string().to_owned())
            .collect();
        assert!(
            page.len() <= 2,
            "the page respects first=2: got {}",
            page.len()
        );
        if page.is_empty() {
            break;
        }
        after = page.last().cloned();
        seen.extend(page);
        if seen.len() >= created.len() {
            break;
        }
    }

    assert_eq!(seen.len(), 5, "all five orgs are paged through: {seen:?}");
    assert_eq!(
        seen, created,
        "keyset pages preserve ascending-id (chronological) order",
    );
}

#[tokio::test]
async fn audio_transcode_endpoint_enqueues_a_job_for_the_worker() {
    let (_db, app) = boot().await;

    app.http()
        .post("/audio/transcode")
        .body_json(&json!({ "file": "track-1.mp3" }))
        .send()
        .await
        .assert_status(StatusCode::UNAUTHORIZED);

    let bearer = format!("Bearer {}", login().await);
    let resp = app
        .http()
        .post("/audio/transcode")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "file": "track-1.mp3" }))
        .send()
        .await;
    resp.assert_status_is_ok();
    assert_eq!(
        resp.json().await.value().object().get("file").string(),
        "track-1.mp3",
    );
}

/// The posts GraphQL adapter: reads are row-level scoped to the caller's org,
/// and `publishPost` transitions a draft to published (the path that emits
/// `PostPublishedEvent` to the notifications listener).
#[tokio::test]
async fn posts_graphql_scopes_reads_and_publish_transitions() {
    let (_db, app) = boot().await;
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "PostAcme").await;
    let org_b = create_org(&app, &bootstrap, "PostGlobex").await;

    // An author user in org A, then a token whose `sub` is that user (a post
    // needs a human author — a subject-less machine token is refused).
    let admin_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let author_id =
        Uuid::parse_str(&create_user(&app, &admin_a, "Author", "author@postacme.test").await)
            .expect("valid user uuid");
    let author_a = format!(
        "Bearer {}",
        token_with_sub(&org_a, "admin", author_id).await
    );
    let admin_b = format!("Bearer {}", token_for(&org_b, "admin").await);

    // Create a post in org A over HTTP — it lands as a draft.
    let post_a = create_post(&app, &author_a, "Launch", "Big news").await;

    let list = json!({ "query": "{ posts { id status } }" });

    // Row-level scope over GraphQL: org B sees none of org A's posts.
    let b = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &admin_b)
        .body_json(&list)
        .send()
        .await;
    b.assert_status_is_ok();
    let b_body = b.json().await;
    assert!(
        b_body.value().object().get_opt("errors").is_none(),
        "org B list must not error",
    );
    assert!(
        b_body
            .value()
            .object()
            .get("data")
            .object()
            .get("posts")
            .array()
            .iter()
            .next()
            .is_none(),
        "org B sees no posts of org A",
    );

    // Org A sees its post, still a draft.
    let a = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&list)
        .send()
        .await;
    a.assert_status_is_ok();
    let a_body = a.json().await;
    let a_status = a_body
        .value()
        .object()
        .get("data")
        .object()
        .get("posts")
        .array()
        .iter()
        .find(|p| p.object().get("id").string() == post_a.as_str())
        .expect("org A sees its own post")
        .object()
        .get("status")
        .string()
        .to_owned();
    assert_eq!(a_status, "DRAFT", "a freshly created post is a draft");

    let publish = |id: &str| json!({ "query": format!("mutation {{ publishPost(id: \"{id}\") {{ id status }} }}") });

    // Org B cannot publish org A's post — forbidden (GraphQL error, no data).
    let denied = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &admin_b)
        .body_json(&publish(&post_a))
        .send()
        .await;
    denied.assert_status_is_ok();
    assert!(
        denied
            .json()
            .await
            .value()
            .object()
            .get_opt("errors")
            .is_some(),
        "org B is forbidden publishing org A's post",
    );

    // Org A publishes — the transition returns PUBLISHED and emits the event.
    let published = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&publish(&post_a))
        .send()
        .await;
    published.assert_status_is_ok();
    let pub_body = published.json().await;
    assert!(
        pub_body.value().object().get_opt("errors").is_none(),
        "publish must not error",
    );
    assert_eq!(
        pub_body
            .value()
            .object()
            .get("data")
            .object()
            .get("publishPost")
            .object()
            .get("status")
            .string(),
        "PUBLISHED",
    );

    // The transition persists.
    let by_id = json!({ "query": format!("{{ post(id: \"{post_a}\") {{ status }} }}") });
    let again = app
        .http()
        .post("/graphql")
        .header(header::AUTHORIZATION, &author_a)
        .body_json(&by_id)
        .send()
        .await;
    again.assert_status_is_ok();
    assert_eq!(
        again
            .json()
            .await
            .value()
            .object()
            .get("data")
            .object()
            .get("post")
            .object()
            .get("status")
            .string(),
        "PUBLISHED",
    );
}

/// The real worker path, booted in-process against the **same** ephemeral DB the
/// api uses: `DatabaseModule` auto-binds the `WorkerDbContext` that gives each
/// job a pool executor, and `NotificationsQueueModule` registers the processor
/// that drains the `notifications` queue. Mirrors the inline module the worker
/// e2e defines to exercise its own consumer.
#[module(
    imports = [
        DatabaseModule::for_root(None),
        QueueModule::for_root(None),
        QueueWorkerModule,
        NotificationsQueueModule,
    ],
)]
struct NotificationsWorkerHarness;

/// End-to-end proof of task A7's magic moment: publishing a post over GraphQL
/// emits `PostPublishedEvent`; the listener enqueues a `NotifyCommand` (no DB —
/// it has no request context); the worker consumes it and persists a
/// `Notification`; and `GET /notifications` returns it, row-level scoped so
/// org B never sees org A's notification.
#[tokio::test]
async fn publishing_a_post_notifies_the_org_through_the_worker() {
    let (db, app) = boot().await;

    // Boot the real worker on the same ephemeral DB + real Redis, and start its
    // queue transport so it drains `notifications`.
    let worker = TestApp::builder()
        .module::<NotificationsWorkerHarness>()
        .provide_arc(db.connection())
        .build_headless()
        .await
        .expect("the notifications worker boots against the ephemeral DB and Redis");
    let worker_queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the worker's QueueWorker drains the notifications queue");

    // Two orgs; an author (with a `sub`) in org A to satisfy the post author FK.
    let bootstrap = format!("Bearer {}", token_for(ORG_ID, "admin").await);
    let org_a = create_org(&app, &bootstrap, "NotifyAcme").await;
    let org_b = create_org(&app, &bootstrap, "NotifyGlobex").await;
    let admin_a = format!("Bearer {}", token_for(&org_a, "admin").await);
    let admin_b = format!("Bearer {}", token_for(&org_b, "admin").await);
    let author_id =
        Uuid::parse_str(&create_user(&app, &admin_a, "Author", "author@notify.test").await)
            .expect("valid user uuid");
    let author_a = format!(
        "Bearer {}",
        token_with_sub(&org_a, "admin", author_id).await
    );

    let post_a = create_post(&app, &author_a, "Launch", "Big news").await;

    // Count org A's notifications through the read resource.
    let notification_count = |bearer: String| {
        let app = &app;
        async move {
            let listed = app
                .http()
                .get("/notifications")
                .header(header::AUTHORIZATION, &bearer)
                .send()
                .await;
            listed.assert_status_is_ok();
            listed.json().await.value().array().iter().count()
        }
    };

    // Publish (re-publishing defensively: a stray competing worker on shared
    // Redis could steal a single message) and poll until the worker persists.
    let mut seen = false;
    'outer: for _ in 0..5 {
        let publish = json!({ "query": format!("mutation {{ publishPost(id: \"{post_a}\") {{ id status }} }}") });
        app.http()
            .post("/graphql")
            .header(header::AUTHORIZATION, &author_a)
            .body_json(&publish)
            .send()
            .await
            .assert_status_is_ok();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            if notification_count(admin_a.clone()).await >= 1 {
                seen = true;
                break 'outer;
            }
        }
    }
    assert!(
        seen,
        "the worker persisted a notification for org A that GET /notifications returns",
    );

    // Row-level scope: org B sees none of org A's notifications.
    assert_eq!(
        notification_count(admin_b).await,
        0,
        "org B must not see org A's notification",
    );

    // And the message is the one the listener produced from the publish event.
    let a_list = app
        .http()
        .get("/notifications")
        .header(header::AUTHORIZATION, &admin_a)
        .send()
        .await;
    a_list.assert_status_is_ok();
    let a_body = a_list.json().await;
    let messages: Vec<String> = a_body
        .value()
        .array()
        .iter()
        .map(|n| n.object().get("message").string().to_owned())
        .collect();
    assert!(
        messages.iter().any(|m| m.contains("Launch")),
        "the persisted notification names the published post: {messages:?}",
    );

    worker_queue
        .shutdown()
        .await
        .expect("the worker's QueueWorker stops cleanly");
}

/// A worker that drains the `audio` queue, booted in-process for the storage
/// round-trip below. `AudioQueueModule` imports the audio port, which owns the
/// `StorageModule` import, so the processor's `AudioService` gets the real
/// `Storage` client — no DB is involved (audio touches only object storage and
/// Redis).
#[module(
    imports = [
        QueueModule::for_root(None),
        QueueWorkerModule,
        AudioQueueModule,
    ],
)]
struct AudioWorkerHarness;

/// A `Storage` client mirroring the app's configuration, used only to ensure the
/// bucket exists before the round-trip. Honors the `NESTRS_STORAGE__*` overrides
/// the app reads (defaults target the dev-container RustFS).
fn storage_client() -> Storage {
    let mut config = StorageConfig::default();
    if let Ok(v) = std::env::var("NESTRS_STORAGE__ENDPOINT") {
        config.endpoint = v;
    }
    if let Ok(v) = std::env::var("NESTRS_STORAGE__ACCESS_KEY") {
        config.access_key = v;
    }
    if let Ok(v) = std::env::var("NESTRS_STORAGE__SECRET_KEY") {
        config.secret_key = v;
    }
    if let Ok(v) = std::env::var("NESTRS_STORAGE__BUCKET") {
        config.bucket = v;
    }
    Storage::new(Arc::new(config))
}

/// Best-effort bucket creation: a presigned PUT on the bucket root is an S3
/// `CreateBucket`; a 2xx means created, a 409 means it already exists. Both are
/// fine — the object round-trip below is the real assertion.
async fn ensure_bucket(http: &reqwest::Client) {
    if let Ok(url) = storage_client()
        .presign_put("", Duration::from_secs(60))
        .await
    {
        let _ = http.put(&url).send().await;
    }
}

/// End-to-end proof of task A4: the audio slice does real S3 object I/O.
/// `POST /audio/uploads` presigns a PUT; the client pushes bytes straight to
/// RustFS (the server never sees the payload); `POST /audio/transcode` enqueues
/// the object key; the worker reads the source object and writes a derived one;
/// and `GET /audio/results` presigns a GET the client fetches to read the
/// derived bytes back — asserted byte-for-byte, against live RustFS, no mocking.
#[tokio::test]
async fn audio_upload_transcode_and_result_round_trips_through_real_storage() {
    let (_db, app) = boot().await;
    let http = reqwest::Client::new();
    ensure_bucket(&http).await;

    // Boot a worker that drains the audio queue against real Redis + storage.
    let worker = TestApp::builder()
        .module::<AudioWorkerHarness>()
        .build_headless()
        .await
        .expect("the audio worker boots against Redis and storage");
    let worker_queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the worker's QueueWorker drains the audio queue");

    let bearer = format!("Bearer {}", login().await);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let filename = format!("e2e-{}-{}.mp3", std::process::id(), nonce);

    // 1. Presign a PUT and push a small payload straight to storage.
    let ticket = app
        .http()
        .post("/audio/uploads")
        .header(header::AUTHORIZATION, &bearer)
        .body_json(&json!({ "filename": filename }))
        .send()
        .await;
    ticket.assert_status_is_ok();
    let ticket = ticket.json().await;
    let key = ticket.value().object().get("key").string().to_owned();
    let put_url = ticket.value().object().get("url").string().to_owned();

    let payload = b"nestrs audio A4 e2e \xf0\x9f\x8e\xb5 payload".to_vec();
    let put = http
        .put(&put_url)
        .body(payload.clone())
        .send()
        .await
        .expect("PUT to the presigned upload URL");
    assert!(
        put.status().is_success(),
        "presigned PUT failed: {} — {}",
        put.status(),
        put.text().await.unwrap_or_default(),
    );

    // 2 & 3. Enqueue the transcode and poll the result endpoint until the worker
    // has produced the derived object. Re-enqueuing across outer iterations is
    // safe (the transform is deterministic) and defends against a stray worker on
    // shared Redis stealing a single message.
    let mut result_url: Option<String> = None;
    'outer: for _ in 0..5 {
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &bearer)
            .body_json(&json!({ "file": key }))
            .send()
            .await
            .assert_status_is_ok();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let resp = app
                .http()
                .get(format!("/audio/results?file={key}"))
                .header(header::AUTHORIZATION, &bearer)
                .send()
                .await;
            if resp.0.status() == StatusCode::OK {
                result_url = Some(
                    resp.json()
                        .await
                        .value()
                        .object()
                        .get("url")
                        .string()
                        .to_owned(),
                );
                break 'outer;
            }
        }
    }
    let result_url =
        result_url.expect("the worker produced the derived object and /audio/results served a URL");

    // 4. Fetch the derived object back through the presigned GET and assert the
    // bytes survived the upload → worker → download round-trip.
    let got = http
        .get(&result_url)
        .send()
        .await
        .expect("GET the presigned result URL");
    assert!(
        got.status().is_success(),
        "presigned GET failed: {}",
        got.status(),
    );
    let got_bytes = got.bytes().await.expect("result body").to_vec();
    assert_eq!(
        got_bytes, payload,
        "the derived object's bytes match the uploaded payload",
    );

    worker_queue
        .shutdown()
        .await
        .expect("the worker's QueueWorker stops cleanly");
}

/// The multipart + streaming counterpart of the presigned round-trip: the client
/// posts the file as `multipart/form-data` straight through the server (no
/// presign), the worker transcodes it, and `GET /audio/download` streams the
/// derived object back chunk by chunk — asserted byte-for-byte against live
/// RustFS, no mocking.
#[tokio::test]
async fn audio_multipart_upload_and_streamed_download_round_trip() {
    let (_db, app) = boot().await;
    let http = reqwest::Client::new();
    ensure_bucket(&http).await;

    let worker = TestApp::builder()
        .module::<AudioWorkerHarness>()
        .build_headless()
        .await
        .expect("the audio worker boots against Redis and storage");
    let worker_queue = worker
        .spawn_transport(QueueWorker::new())
        .await
        .expect("the worker's QueueWorker drains the audio queue");

    let bearer = format!("Bearer {}", login().await);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let filename = format!("e2e-multipart-{}-{}.mp3", std::process::id(), nonce);
    let payload = b"nestrs multipart + streaming payload \xf0\x9f\x8e\xa7".to_vec();

    // 1. Upload the file directly as multipart/form-data through the server.
    let form = TestForm::new().field(
        TestFormField::bytes(payload.clone())
            .name("file")
            .filename(&filename),
    );
    let up = app
        .http()
        .post("/audio/uploads/direct")
        .header(header::AUTHORIZATION, &bearer)
        .multipart(form)
        .send()
        .await;
    up.assert_status_is_ok();
    let key = up
        .json()
        .await
        .value()
        .object()
        .get("key")
        .string()
        .to_owned();

    // 2 & 3. Enqueue the transcode and stream the derived object back once ready.
    let mut downloaded: Option<Vec<u8>> = None;
    'outer: for _ in 0..5 {
        app.http()
            .post("/audio/transcode")
            .header(header::AUTHORIZATION, &bearer)
            .body_json(&json!({ "file": key }))
            .send()
            .await
            .assert_status_is_ok();

        for _ in 0..20 {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let resp = app
                .http()
                .get(format!("/audio/download?file={key}"))
                .header(header::AUTHORIZATION, &bearer)
                .send()
                .await;
            if resp.0.status() == StatusCode::OK {
                let bytes = resp.0.into_body().into_bytes().await.expect("stream body");
                downloaded = Some(bytes.to_vec());
                break 'outer;
            }
        }
    }
    let downloaded = downloaded.expect("the streamed download served the derived object");
    assert_eq!(
        downloaded, payload,
        "the streamed bytes match the multipart upload",
    );

    // 4. The SSE progress feed reports the transcode as ready and closes.
    let events = app
        .http()
        .get(format!("/audio/events?file={key}"))
        .header(header::AUTHORIZATION, &bearer)
        .send()
        .await;
    events.assert_status_is_ok();
    events.assert_content_type("text/event-stream");
    let body = events.0.into_body().into_bytes().await.expect("sse body");
    let body = String::from_utf8(body.to_vec()).expect("sse is utf-8");
    assert!(
        body.contains("event: transcode") && body.contains("\"state\":\"ready\""),
        "the SSE feed emits a ready transcode event: {body:?}",
    );

    worker_queue
        .shutdown()
        .await
        .expect("the worker's QueueWorker stops cleanly");
}
