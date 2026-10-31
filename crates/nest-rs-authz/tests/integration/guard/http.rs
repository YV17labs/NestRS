//! Covers `src/guard.rs` — the events `AbilityGuard` files when it denies,
//! run through the HTTP edge's guard stage as the unit a route declares.

use std::any::TypeId;
use std::sync::Arc;

use nest_rs_authz::__private::AmbientAbility;
use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::__private::{HandlerDeclaration, MetaLevels, declare_handler};
use nest_rs_core::{AmbientValue, Container, Discoverable, Handler, Posture, with_ambient};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::__private::{attached, check_request};
use nest_rs_http::Request;
use nest_rs_testing::LogCapture;

use crate::{child, parent};

#[derive(Clone)]
struct Actor;

impl AmbientValue for Actor {}

struct Routes;

/// A route with no posture of its own, gated by its guards alone.
static GATED: Handler = declare_handler(HandlerDeclaration {
    unit: nest_rs_http::unit::REQUEST,
    host: TypeId::of::<Routes>(),
    host_name: "Routes",
    method: "gated",
    address: "/",
    posture: Posture::Gated,
    meta: MetaLevels::default,
});

/// An in-band endpoint, as `/graphql` declares itself.
static IN_BAND: Handler = declare_handler(HandlerDeclaration {
    unit: nest_rs_http::unit::REQUEST,
    host: TypeId::of::<Routes>(),
    host_name: "Routes",
    method: "in_band",
    address: "/",
    posture: Posture::InBand,
    meta: MetaLevels::default,
});

/// Run `guard` on `req` as the HTTP edge runs it on the route `site`.
async fn check(guard: &dyn Guard, site: &'static Handler, req: &mut Request) -> Result<(), Denial> {
    check_request(site, [("AbilityGuard", guard)], req).await
}

/// Grants nothing and never fails: enough to reach the no-actor branch.
struct WellFormed;

impl AbilityFactory for WellFormed {
    type Actor = Actor;

    fn define(&self, _actor: &Self::Actor, ability: &mut AbilityBuilder) {
        ability.can(Action::Read, parent::Entity);
    }
}

/// Declares the related entity as `child` while the relation points at
/// `parent` — the mismatch `PredicateBuilder::related` answers with `Deny`,
/// which `build` reports rather than installing.
struct Malformed;

impl AbilityFactory for Malformed {
    type Actor = Actor;

    fn define(&self, _actor: &Self::Actor, ability: &mut AbilityBuilder) {
        ability.can(Action::Read, child::Entity).when(|p| {
            p.related::<child::Entity, _>(child::Relation::Parent, |c| c.eq(child::Column::Id, 1))
        });
    }
}

/// The guard takes its factory by injection, so a test builds it the way the
/// boot does — registered into a container holding that factory.
fn guard_over<F: AbilityFactory>(factory: F) -> Arc<AbilityGuard<F>> {
    let builder = Container::builder().provide_arc(Arc::new(factory));
    <AbilityGuard<F> as Discoverable>::register(builder)
        .build()
        .expect("the container builds")
        .get::<AbilityGuard<F>>()
        .expect("the guard registers over its factory")
}

#[tokio::test]
async fn a_route_reached_with_no_actor_files_that_no_authn_guard_ran() {
    let logs = LogCapture::install();
    let guard = guard_over(WellFormed);
    // No ambient `Actor`, on a route that is not public: the branch that means
    // the developer bound the ability guard without an authn guard before it.
    let mut req = Request::builder().finish();

    let denial = check(&*guard, &GATED, &mut req)
        .await
        .expect_err("no actor on a non-public route is refused");
    assert_eq!(denial.http_status(), 500, "{denial:?}");

    let event = logs.expect_one("nest_rs::authz", "guard denied the request");
    assert_eq!(
        event.level, "error",
        "a guard bound without its authn guard is a defect"
    );
    assert_eq!(event.field("reason").as_deref(), Some("missing_ambient"));
    assert!(
        event.field("error").is_some_and(|t| t.contains("Actor")),
        "the line names the actor type the missing authn guard would have attached, got {:?}",
        event.fields,
    );
}

/// An anonymous request to an in-band endpoint (`/graphql`) gets the visitor
/// ability, which each operation's own gate then reads.
#[tokio::test]
async fn an_in_band_request_with_no_actor_gets_the_visitor_ability() {
    let guard = guard_over(WellFormed);
    let mut req = Request::builder().finish();

    check(&*guard, &IN_BAND, &mut req)
        .await
        .expect("an anonymous in-band request is left to its operations' gates");
    let ability = attached::<AmbientAbility>(&req)
        .and_then(|held| held.ability())
        .expect("the visitor ability is attached");
    assert!(ability.is_visitor());
}

#[tokio::test]
async fn a_malformed_rule_is_reported_at_error_before_the_request_is_denied() {
    let logs = LogCapture::install();
    let guard = guard_over(Malformed);
    let mut req = Request::builder().finish();

    let denial = with_ambient(Arc::new(Actor), check(&*guard, &GATED, &mut req))
        .await
        .expect_err("a malformed rule denies rather than installing a partial ability");
    assert_eq!(denial.http_status(), 500, "{denial:?}");

    let event = logs.expect_one("nest_rs::authz", "guard denied the request");
    assert_eq!(event.level, "error");
    assert_eq!(
        event.field("reason").as_deref(),
        Some("rules_misconfigured")
    );
    // The client body is deliberately opaque, so the rule that broke is only
    // ever readable here.
    assert!(
        event.field("error").is_some_and(|e| e.contains("child")),
        "the event names the offending rule's subject, got {:?}",
        event.fields,
    );
}
