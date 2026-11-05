use nest_rs::core::module;
use nest_rs::http::header;
use nest_rs::redis::RedisQueueModule;
use nest_rs::seaorm::SeaOrmDatabaseModule;
use nest_rs::testing::TestApp;
use serde_json::json;

use features::authn::{AuthnModule, Role};
use features::testing::{claims, connection, seed_org, seed_user, user};
use features::users::{UserRole, UsersGraphqlModule};

#[module(
    imports = [
        SeaOrmDatabaseModule,
        RedisQueueModule,
        AuthnModule,
        UsersGraphqlModule,
    ],
)]
struct UsersGraphqlTestModule;

#[tokio::test]
async fn the_users_adapter_serves_every_relation_of_a_user_without_another_adapter() {
    let app = TestApp::builder()
        .module::<UsersGraphqlTestModule>()
        .ephemeral_database::<migrations::Migrator>()
        .ephemeral_signing_key()
        .build()
        .await
        .expect("UsersGraphqlModule boots with no posts adapter beside it");
    let conn = connection(app.container());
    let org = seed_org(&conn, "Acme").await;
    seed_user(&conn, org, "Ada", "ada@acme.test", UserRole::User).await;

    let resp = app
        .http()
        .post("/graphql")
        .header(
            header::AUTHORIZATION,
            app.bearer(&claims(org, Role::Admin, user())),
        )
        .body_json(
            &json!({ "query": "{ users { id org { id } posts { edges { node { id } } } } }" }),
        )
        .send()
        .await;

    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "every relation of `User` resolves",
    );
    assert_eq!(
        body.value()
            .object()
            .get("data")
            .object()
            .get("users")
            .array()
            .iter()
            .count(),
        1,
        "the seeded row is the parent each relation resolved on",
    );
}
