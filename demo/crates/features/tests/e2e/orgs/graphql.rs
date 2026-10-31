use nest_rs::authn::AuthnConfig;
use nest_rs::core::module;
use nest_rs::http::header;
use nest_rs::redis::{RedisModule, RedisQueueModule};
use nest_rs::seaorm::{SeaOrmDatabaseModule, SeaOrmModule};
use nest_rs::testing::{EphemeralDatabase, TestApp};
use sea_orm::{ActiveModelTrait, Set};
use serde_json::json;
use uuid::Uuid;

use features::authn::{AuthnModule, Role};
use features::orgs::ActiveModel as OrgActive;
use features::orgs::OrgsGraphqlModule;
use features::testing::{API, DEV_PUBLIC_KEY, token, user};

#[module(
    imports = [
        SeaOrmModule,
        SeaOrmDatabaseModule,
        RedisModule,
        RedisQueueModule,
        AuthnModule,
        OrgsGraphqlModule,
    ],
)]
struct OrgsGraphqlTestModule;

#[tokio::test]
async fn the_orgs_adapter_serves_every_relation_of_an_org_without_another_adapter() {
    let db = EphemeralDatabase::create::<migrations::Migrator>()
        .await
        .expect("create + migrate a throwaway database");
    let org_id = Uuid::now_v7();
    OrgActive {
        id: Set(org_id),
        name: Set("Acme".into()),
        ..Default::default()
    }
    .insert(db.connection().as_ref())
    .await
    .expect("seed org");
    let app = TestApp::builder()
        .module::<OrgsGraphqlTestModule>()
        .provide_arc(db.connection())
        .provide(AuthnConfig {
            public_key: Some(DEV_PUBLIC_KEY.into()),
            audience: Some(API.into()),
            ..Default::default()
        })
        .build()
        .await
        .expect("OrgsGraphqlModule boots with no posts adapter beside it");

    let resp = app
        .http()
        .post("/graphql")
        .header(
            header::AUTHORIZATION,
            format!("Bearer {}", token(API, org_id, vec![Role::Admin], user())),
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
