//! Covers `src/guard.rs` — what `AbilityGuard` *says* when it denies.
//!
//! The refusals themselves are asserted elsewhere; what nothing read was the
//! event. Both are `warn`+ on `nest_rs::authz`, which `CLAUDE.md` calls the
//! events queried under incident: an operator answering "why did this 500" has
//! only these lines, and a denial that fails closed while logging nothing is
//! indistinguishable from a bug in the handler.

use std::sync::Arc;

use nest_rs_authz::{AbilityBuilder, AbilityFactory, AbilityGuard, Action};
use nest_rs_core::Container;
use nest_rs_guards::{Denial, Guard};
use nest_rs_testing::LogCapture;
use poem::Request;

use crate::{child, parent};

#[derive(Clone)]
struct Actor;

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
        // The subject is `child`, whose `Parent` relation points at `parent` —
        // declaring `child` as the related entity is the mismatch.
        ability.can(Action::Read, child::Entity).when(|p| {
            p.related::<child::Entity, _>(child::Relation::Parent, |c| c.eq(child::Column::Id, 1))
        });
    }
}

/// The guard takes its factory by injection, so a test builds it the way the
/// boot does — through a container holding that factory.
fn guard_over<F: AbilityFactory>(factory: F) -> AbilityGuard<F> {
    let container = Container::builder().provide_arc(Arc::new(factory)).build();
    AbilityGuard::from_container(&container)
}

#[tokio::test]
async fn a_route_reached_with_no_actor_warns_that_no_authn_guard_ran() {
    let logs = LogCapture::install();
    let guard = guard_over(WellFormed);
    // No `Actor` in extensions, and no `Public` marker: the branch that means
    // the developer bound the ability guard without an authn guard before it.
    let mut req = Request::builder().finish();

    let denial = guard
        .check_http(&mut req)
        .await
        .expect_err("no actor on a non-public route is refused");
    assert!(matches!(denial, Denial::Internal(_)), "{denial:?}");

    let event = logs.expect_one(
        "nest_rs::authz",
        "ability guard denied: no authenticated actor and route is not public",
    );
    assert_eq!(event.level, "warn");
    // The field is what makes the line actionable: it names the actor type the
    // factory expected, which is what the missing authn guard would have
    // inserted.
    assert!(
        event
            .field("actor_type")
            .is_some_and(|t| t.contains("Actor")),
        "the event names the expected actor type, got {:?}",
        event.fields,
    );
}

#[tokio::test]
async fn a_malformed_rule_is_reported_at_error_before_the_request_is_denied() {
    let logs = LogCapture::install();
    let guard = guard_over(Malformed);
    let mut req = Request::builder().finish();
    req.extensions_mut().insert(Actor);

    let denial = guard
        .check_http(&mut req)
        .await
        .expect_err("a malformed rule denies rather than installing a partial ability");
    assert!(matches!(denial, Denial::Internal(_)), "{denial:?}");

    let event = logs.expect_one(
        "nest_rs::authz",
        "ability construction failed — denying the request",
    );
    assert_eq!(event.level, "error");
    // The client body is deliberately opaque, so the rule that broke is only
    // ever readable here.
    assert!(
        event.field("error").is_some_and(|e| e.contains("child")),
        "the event names the offending rule's subject, got {:?}",
        event.fields,
    );
}
