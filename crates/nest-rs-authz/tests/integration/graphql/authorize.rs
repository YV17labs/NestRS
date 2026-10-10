//! The resolver gate end-to-end through the in-band path: `GraphqlAbilityBridge`
//! builds the ability per operation and `#[authorize(Read, …)]` admits or rejects
//! by role. `/graphql` is `EdgePosture::Exempt`, so the bridge is the only
//! execution site.

use std::sync::Arc;

use nest_rs_authz::graphql::GraphqlAbilityBridge;
use nest_rs_authz::{AbilityBuilder, Action, Read};
use nest_rs_core::{Layer, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlOperationGuard, operations, resolver};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::async_trait;
use nest_rs_http::poem::Request;
use nest_rs_testing::TestApp;

use super::query;

/// A throwaway SeaORM entity to act as the authorization `Subject`.
mod widget {
    use sea_orm::entity::prelude::*;
    use serde::{Deserialize, Serialize};

    #[derive(Clone, Debug, PartialEq, DeriveEntityModel, Serialize, Deserialize)]
    #[sea_orm(table_name = "widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        pub id: i32,
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// No-op stand-in for the bridge's authentication slot (`A` in
/// `GraphqlAbilityBridge<A, G>`) — this test only exercises the ability path.
#[injectable]
#[derive(Default)]
struct PassGuard;

impl Layer for PassGuard {}

#[async_trait]
impl Guard for PassGuard {}

/// Stands in for the `AbilityGuard` slot: reads the caller's role from a
/// header and builds the matching `Ability` onto the request. An admin gets a
/// Read grant on widgets; anyone else gets nothing.
///
/// No `x-role` header is the anonymous caller, taking the visitor branch.
#[injectable]
#[derive(Default)]
struct AbilityInjector;

impl Layer for AbilityInjector {}

#[async_trait]
impl Guard for AbilityInjector {
    async fn check_http(&self, req: &mut Request) -> Result<(), Denial> {
        let role = req
            .headers()
            .get("x-role")
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned);
        let mut b = AbilityBuilder::new();
        let ability = match role.as_deref() {
            None => {
                b.can(Action::Read, widget::Entity);
                b.build_visitor()
            }
            Some("admin") => {
                b.can(Action::Read, widget::Entity)
                    .when(|p| p.eq(widget::Column::Id, 1));
                b.build()
            }
            Some(_) => b.build(),
        };
        req.extensions_mut()
            .insert(Arc::new(ability.expect("valid test ability")));
        Ok(())
    }
}

impl HttpGuard for AbilityInjector {}

impl HttpGuard for PassGuard {}

impl nest_rs_authz::WireModelDefaults for widget::Entity {}

#[resolver]
struct WidgetResolver;

#[operations]
impl WidgetResolver {
    #[query]
    #[authorize(Read, widget::Entity)]
    async fn widget_name(&self) -> GqlResult<String> {
        Ok("ada".into())
    }

    /// The `#[public]` twin of `widget_name`: the surface a visitor grant is
    /// written for, and the one it must reach.
    #[query]
    #[public]
    async fn widget_motd(&self) -> GqlResult<String> {
        Ok("hello".into())
    }
}

/// The same shape `crates/features` wires for the real app:
/// `GraphqlAbilityBridge<AuthnGuard, AuthzGuard> as dyn GraphqlOperationGuard`.
type TestOpGuard = GraphqlAbilityBridge<PassGuard, AbilityInjector>;

#[module(
    imports = [GraphqlModule::for_root(None)],
    providers = [
        PassGuard,
        AbilityInjector,
        TestOpGuard as dyn GraphqlOperationGuard,
        WidgetResolver,
    ],
)]
struct AuthzGraphqlModule;

async fn boot() -> TestApp {
    TestApp::builder()
        .module::<AuthzGraphqlModule>()
        .build()
        .await
        .expect("the schema boots and mounts at /graphql")
}

#[tokio::test]
async fn admin_passes_the_resolver_gate() {
    let app = boot().await;
    let json = query(&app, "admin", "{ widgetName }").await;
    assert_eq!(json["data"]["widgetName"], "ada", "{json}");
}

/// A `define_visitor` grant opens the `#[public]` operations and nothing else.
#[tokio::test]
async fn a_visitor_grant_does_not_satisfy_a_guarded_operation() {
    let app = boot().await;
    let json = query(&app, "", "{ widgetName }").await;
    assert_eq!(
        json["data"],
        serde_json::Value::Null,
        "an anonymous caller must read no data from an `#[authorize]` operation: {json}",
    );
    assert_eq!(
        json["errors"][0]["extensions"]["code"], "UNAUTHENTICATED",
        "the anonymous caller is refused for want of a principal, not of a grant: {json}",
    );
}

/// The other half of the same contract: the visitor grant does reach the
/// operation it was written for.
#[tokio::test]
async fn a_visitor_grant_still_reaches_a_public_operation() {
    let app = boot().await;
    let json = query(&app, "", "{ widgetMotd }").await;
    assert_eq!(json["data"]["widgetMotd"], "hello", "{json}");
}

/// The refusal reaches the caller and the operator. Single-thread runtime:
/// `LogCapture` is thread-local.
#[tokio::test]
async fn non_admin_is_forbidden_by_the_resolver_gate() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = boot().await;
    // GraphQL reports authorization failures as a 200 response carrying an
    // `errors` array, not an HTTP status.
    let json = query(&app, "user", "{ widgetName }").await;
    assert_eq!(
        json["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{json}"
    );

    let event = logs
        .find("nest_rs::authz", "authorization denied")
        .into_iter()
        .next()
        .expect("a refused operation is reported to the operator, not only to the caller");
    assert_eq!(
        event.level, "warn",
        "a denial is a security event: `warn` or above, never `debug`",
    );
    assert_eq!(event.field("transport").as_deref(), Some("graphql"));
    assert!(
        event.field("subject").is_some_and(|s| s.contains("widget")),
        "the denial names the subject it refused, or an incident cannot tell \
         which entity was reached for: {event:?}",
    );
    assert_eq!(
        event.field("reason").as_deref(),
        Some("no_class_grant"),
        "and why, which is what separates a missing grant from a missing \
         principal when the two render the same to the caller: {event:?}",
    );
}
