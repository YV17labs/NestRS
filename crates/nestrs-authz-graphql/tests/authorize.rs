//! The resolver gate end-to-end: an HTTP guard builds the actor's `Ability`, the
//! `ContextSeed` forwards it into the GraphQL context, and `authorize` admits or
//! rejects the query by the caller's role — driven through the in-process harness.

use std::sync::Arc;

use async_graphql::{Context, Result as GqlResult};
use nestrs_authz::{AbilityBuilder, Action, Read};
use nestrs_authz_graphql::authorize;
use nestrs_core::module;
use nestrs_graphql::{resolver, GraphqlModule};
use nestrs_http::poem::{Request, Response};
use nestrs_http::{async_trait, Guard, HttpTransport};
use nestrs_testing::TestApp;

/// A throwaway SeaORM entity to act as the authorization `Subject`.
mod widget {
    use sea_orm::entity::prelude::*;

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "widgets")]
    pub struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// Stands in for the `AuthGuard` + `AbilityGuard` chain: reads the caller's role
/// from a header and builds the matching `Ability` onto the request. An admin
/// gets a Read grant on widgets; anyone else gets nothing.
struct AbilityInjector;

#[async_trait]
impl Guard for AbilityInjector {
    async fn check(&self, req: &mut Request) -> Result<(), Response> {
        let admin = req
            .headers()
            .get("x-role")
            .and_then(|v| v.to_str().ok())
            .map(|role| role == "admin")
            .unwrap_or(false);
        let mut b = AbilityBuilder::new();
        if admin {
            b.can(Action::Read, widget::Entity)
                .when(|p| p.eq(widget::Column::Id, 1));
        }
        req.extensions_mut().insert(Arc::new(b.build()));
        Ok(())
    }
}

#[resolver]
struct WidgetResolver;

#[resolver]
impl WidgetResolver {
    #[query]
    async fn widget_name(&self, ctx: &Context<'_>) -> GqlResult<String> {
        authorize::<Read, widget::Entity>(ctx)?;
        Ok("ada".into())
    }
}

#[module(imports = [GraphqlModule])]
struct AuthzGraphqlModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AuthzGraphqlModule>()
        .http(HttpTransport::new().guard(AbilityInjector))
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

#[tokio::test]
async fn admin_passes_the_resolver_gate() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .header("x-role", "admin")
        .body_json(&serde_json::json!({ "query": "{ widgetName }" }))
        .send()
        .await;
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let name = json
        .value()
        .object()
        .get("data")
        .object()
        .get("widgetName")
        .string();
    assert_eq!(name, "ada");
}

#[tokio::test]
async fn non_admin_is_forbidden_by_the_resolver_gate() {
    let app = boot().await;
    let resp = app
        .http()
        .post("/graphql")
        .header("x-role", "user")
        .body_json(&serde_json::json!({ "query": "{ widgetName }" }))
        .send()
        .await;
    // GraphQL reports authorization failures as a 200 response carrying an
    // `errors` array, not an HTTP status.
    resp.assert_status_is_ok();

    let json = resp.json().await;
    let errors = json.value().object().get("errors").array();
    assert!(
        !errors.is_empty(),
        "a forbidden query must carry a GraphQL error"
    );
}
