use nest_rs::core::module;
use nest_rs::http::header;
use nest_rs::redis::RedisQueueModule;
use nest_rs::seaorm::SeaOrmDatabaseModule;
use nest_rs::testing::TestApp;
use serde_json::json;

use features::authn::Role;
use features::orgs::OrgsGraphqlModule;
use features::testing::{claims, connection, seed_org, user};

#[module(
    imports = [
        SeaOrmDatabaseModule,
        RedisQueueModule,
        OrgsGraphqlModule,
    ],
)]
struct OrgsGraphqlTestModule;

#[tokio::test]
async fn the_orgs_adapter_serves_every_relation_of_an_org_without_another_adapter() {
    let app = TestApp::builder()
        .module::<OrgsGraphqlTestModule>()
        .ephemeral_database::<migrations::Migrator>()
        .ephemeral_signing_key()
        .build()
        .await
        .expect("OrgsGraphqlModule boots with no posts adapter beside it");
    let org = seed_org(&connection(app.container()), "Acme").await;

    let resp = app
        .http()
        .post("/graphql")
        .header(
            header::AUTHORIZATION,
            app.bearer(&claims(org, Role::Admin, user())),
        )
        .body_json(&json!({ "query": "{ orgs { id users { edges { node { id } } } posts { edges { node { id } } } } }" }))
        .send()
        .await;

    resp.assert_status_is_ok();
    let body = resp.json().await;
    assert!(
        body.value().object().get_opt("errors").is_none(),
        "every relation of `Org` resolves",
    );
    assert_eq!(
        body.value()
            .object()
            .get("data")
            .object()
            .get("orgs")
            .array()
            .iter()
            .count(),
        1,
        "the seeded row is the parent each relation resolved on",
    );
}
