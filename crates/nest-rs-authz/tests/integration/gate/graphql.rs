//! The class gate on a resolver, end to end: the guards at the `/graphql`
//! request build the ability and `#[authorize(Read, …)]` admits or rejects by
//! role. `/graphql` is an `InBand` mount, so an anonymous request reaches the
//! gate, which refuses it.

use nest_rs_authz::{AbilityBuilder, Action, Read, Update};
use nest_rs_core::{Layer, async_trait, injectable, module};
use nest_rs_graphql::async_graphql::Result as GqlResult;
use nest_rs_graphql::{GraphqlModule, GraphqlView, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::EndpointGuards;
use nest_rs_testing::TestApp;

use crate::graphql::query;

/// A throwaway SeaORM entity to act as the authorization `Subject`.
mod widget {
    use nest_rs_seaorm::expose;
    use sea_orm::entity::prelude::*;

    #[expose(name = "Widget")]
    #[derive(Clone, Debug, PartialEq, DeriveEntityModel)]
    #[sea_orm(table_name = "widgets")]
    pub(super) struct Model {
        #[sea_orm(primary_key)]
        #[expose]
        pub id: i32,
        #[expose]
        pub name: String,
    }

    #[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
    pub(super) enum Relation {}

    impl ActiveModelBehavior for ActiveModel {}
}

/// No-op stand-in for the authentication guard ahead of the ability guard at
/// the request — this test only exercises the ability path.
#[injectable]
#[derive(Default)]
struct PassGuard;

impl Layer for PassGuard {}

#[async_trait]
impl Guard for PassGuard {
    async fn check(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Ok(())
    }
}

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
    async fn check_http(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let role = nest_rs_http::HttpView::of(cx)?
            .request()
            .header("x-role")
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
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            ability.expect("valid test ability"),
        ));
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for AbilityInjector {}

impl GuardFor<nest_rs_http::HttpView> for PassGuard {}

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

#[module(
    imports = [GraphqlModule],
    providers = [
        PassGuard,
        AbilityInjector,
        EndpointGuards<GraphqlView, (PassGuard, AbilityInjector)>,
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
    assert_eq!(event.field("kind").as_deref(), Some("query"));
    assert_eq!(event.field("field").as_deref(), Some("widgetName"));
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

/// A unit's own guard, with nothing at the `/graphql` request: it builds the
/// ability the posture reads, a Read grant on widgets and nothing else.
#[injectable]
#[derive(Default)]
struct UnitAbility;

impl Layer for UnitAbility {}

#[async_trait]
impl Guard for UnitAbility {
    async fn check(&self, cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        let mut b = AbilityBuilder::new();
        b.can(Action::Read, widget::Entity);
        cx.attach(nest_rs_authz::__private::AmbientAbility::new(
            b.build().expect("valid test ability"),
        ));
        Ok(())
    }
}

impl GuardFor<GraphqlView> for UnitAbility {}

#[resolver]
#[use_guards(UnitAbility)]
struct ScopedResolver;

#[operations]
impl ScopedResolver {
    #[query]
    #[authorize(Read, widget::Entity)]
    async fn scoped_name(&self) -> GqlResult<String> {
        Ok("ada".into())
    }

    #[query]
    #[authorize(Update, widget::Entity)]
    async fn scoped_write(&self) -> GqlResult<String> {
        Ok("written".into())
    }
}

#[module(imports = [GraphqlModule], providers = [UnitAbility, ScopedResolver])]
struct ScopedModule;

/// No guard at the request is no `#[public]`-only surface: a gated operation
/// whose resolver binds its guards runs, and refuses what they do not grant, so
/// the boot's line says each unit decides alone.
#[tokio::test]
async fn where_no_guard_runs_at_the_request_each_unit_decides_alone() {
    let logs = nest_rs_testing::LogCapture::install();
    let app = TestApp::builder()
        .module::<ScopedModule>()
        .build()
        .await
        .expect("a resolver binding its guards boots with nothing at the request");

    let read = query(&app, "", "{ scopedName }").await;
    assert_eq!(read["data"]["scopedName"], "ada", "{read}");
    let write = query(&app, "", "{ scopedWrite }").await;
    assert_eq!(
        write["errors"][0]["extensions"]["code"], "FORBIDDEN",
        "{write}"
    );

    let line = logs.expect_one("nest_rs::layers", "no guard at the request");
    assert_eq!(line.field("surface").as_deref(), Some("graphql"));
    assert_eq!(
        line.field("then").as_deref(),
        Some("each unit decides alone")
    );
}
