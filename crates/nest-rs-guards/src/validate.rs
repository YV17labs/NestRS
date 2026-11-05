//! Boot-time guard-chain validation — declared [`GuardPhase`] ordering and
//! produced/expected [`PrincipalClaim`](crate::PrincipalClaim) cross-checks: a
//! chain whose declarations cannot line up fails boot with a named error
//! instead of answering `500` on every request.

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, resolve_scoped, shorten,
};
use nest_rs_core::Container;
use nest_rs_core::LayerSite;

use crate::registry::{GuardSpec, resolve_global_guards};
use crate::{Guard, GuardPhase};

/// Validate one resolved guard chain (in execution order).
///
/// Fails on: an `Authentication`-phase guard listed after an
/// `Authorization`-phase one; a guard expecting a principal type produced
/// only *later* in the chain; a guard expecting a principal type while the
/// chain's producer(s) attach a **different** type; a guard requiring a
/// principal nothing in the chain attaches. One doing without it
/// ([`PrincipalClaim::or_visitor`](crate::PrincipalClaim::or_visitor)) only
/// warns then — a controller may serve exclusively `#[public]` routes, where
/// the authorization guard admits the anonymous actor.
///
/// A guard is checked where it first runs: a `#[force_guards]` replay later in
/// the chain is no misorder, since every guard between the two runs already
/// read what the first one attached.
pub fn validate_guard_chain(label: &str, chain: &[ResolvedLayer<dyn Guard>]) -> Result<(), String> {
    validate(label, chain, |_| true)
}

/// [`validate_guard_chain`] for a route whose method scope declares guards,
/// once its controller's chain was validated ([`boot_validate_guards`]): a
/// consumer the controller's check already said has no producer is not said
/// again, so only the method's own guards warn.
pub fn validate_route_chain(label: &str, chain: &[ResolvedLayer<dyn Guard>]) -> Result<(), String> {
    validate(label, chain, |entry| entry.source == LayerSite::Method)
}

fn validate(
    label: &str,
    chain: &[ResolvedLayer<dyn Guard>],
    warns: impl Fn(&ResolvedLayer<dyn Guard>) -> bool,
) -> Result<(), String> {
    let first_runs: Vec<&ResolvedLayer<dyn Guard>> = chain
        .iter()
        .enumerate()
        .filter(|(i, entry)| !chain[..*i].iter().any(|e| e.type_id == entry.type_id))
        .map(|(_, entry)| entry)
        .collect();
    let chain = first_runs.as_slice();

    let mut saw_authorization: Option<&'static str> = None;
    for entry in chain {
        match entry.layer.phase() {
            GuardPhase::Authorization => saw_authorization = Some(entry.name),
            GuardPhase::Authentication => {
                if let Some(authz) = saw_authorization {
                    return Err(format!(
                        "guard chain for {label} lists authentication guard `{}` after \
                         authorization guard `{}` — authentication must run first \
                         so the principal it attaches is available; reorder the \
                         `use_guards` declaration",
                        shorten(entry.name),
                        shorten(authz),
                    ));
                }
            }
            GuardPhase::Other => {}
        }
    }

    let producers: Vec<(usize, &'static str, crate::PrincipalClaim)> = chain
        .iter()
        .enumerate()
        .filter_map(|(i, e)| e.layer.produced_principal().map(|c| (i, e.name, c)))
        .collect();
    for (i, entry) in chain.iter().enumerate() {
        let Some(expected) = entry.layer.expected_principal() else {
            continue;
        };
        // `producers` is in chain order, so the first type match is the
        // earliest — one scan decides all four outcomes.
        match producers
            .iter()
            .find(|(_, _, c)| c.type_id == expected.type_id)
        {
            Some((j, _, _)) if *j < i => {}
            Some((_, late_name, _)) => {
                let (guard, late_name) = (shorten(entry.name), shorten(late_name));
                return Err(format!(
                    "guard chain for {label}: `{guard}` expects principal `{}` which `{late_name}` \
                     only attaches later in the chain — list `{late_name}` before `{guard}`",
                    shorten(expected.type_name),
                ));
            }
            None if !producers.is_empty() => {
                let produced: Vec<String> = producers
                    .iter()
                    .map(|(_, _, c)| shorten(c.type_name))
                    .collect();
                return Err(format!(
                    "guard chain for {label}: `{}` expects principal `{}` but the chain's \
                     authentication guard(s) attach [{}] — the auth strategy's principal and \
                     the ability factory's `Actor` must be the same type",
                    shorten(entry.name),
                    shorten(expected.type_name),
                    produced.join(", "),
                ));
            }
            None if !expected.or_visitor => return Err(unattached(label, entry.name, expected)),
            None if !warns(entry) => {}
            None => {
                tracing::warn!(
                    target: nest_rs_core::target::LAYERS,
                    route = label,
                    guard = shorten(entry.name),
                    expected = shorten(expected.type_name),
                    "guard expects a principal but no guard in the chain produces one — \
                     non-public routes will answer 500",
                );
            }
        }
    }
    Ok(())
}

/// The refusal of a guard requiring a principal nothing attaches: every request
/// it runs on would answer `500`.
fn unattached(label: &str, guard: &str, expected: crate::PrincipalClaim) -> String {
    let guard = shorten(guard);
    format!(
        "guard chain for {label}: `{guard}` requires principal `{}` and no guard in the chain \
         attaches it — bind the guard attaching it ahead of `{guard}`, and have that guard \
         declare it (`produced_principal`)",
        shorten(expected.type_name),
    )
}

/// The principal half of [`validate_guard_chain`] on a unit an in-band edge
/// carries: each guard of its `unit` chain requiring a principal finds a guard
/// attaching it there or in the chain its `request` ran, which the closure
/// composes only for a chain that has such a guard. Ordering stays unchecked in
/// band, and a guard doing without its principal reads what it finds.
pub fn validate_carried_chain(
    label: &str,
    unit: &[ResolvedLayer<dyn Guard>],
    request: impl FnOnce() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
) -> Result<(), String> {
    let required: Vec<_> = unit
        .iter()
        .enumerate()
        .filter_map(|(at, entry)| {
            let expected = entry.layer.expected_principal()?;
            (!expected.or_visitor).then_some((at, entry.name, expected))
        })
        .collect();
    if required.is_empty() {
        return Ok(());
    }
    let request = request()?;
    for (at, guard, expected) in required {
        let attached = request.iter().chain(&unit[..at]).any(|entry| {
            entry
                .layer
                .produced_principal()
                .is_some_and(|produced| produced.type_id == expected.type_id)
        });
        if !attached {
            return Err(unattached(label, guard, expected));
        }
    }
    Ok(())
}

/// Whether a unit an in-band edge carries needs no guard stage: its `unit` chain
/// is empty, and a posture that `withheld` its reads has nothing to withhold, with
/// no guard at its `request` either to have attached anything.
pub fn carried_stage_idle(
    unit: &[ResolvedLayer<dyn Guard>],
    withheld: bool,
    request: impl FnOnce() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
) -> Result<bool, String> {
    let withholds = withheld && !request()?.is_empty();
    Ok(unit.is_empty() && !withholds)
}

/// Boot check for one controller or gateway upgrade: compose the global pool
/// with the host-scope specs (the dedup a route's guard stage composes) and validate the
/// result. A host-scope spec no imported module provides fails it, naming the
/// guard and `label`. A route whose method scope declares guards is validated
/// again, whole, as it mounts.
pub fn boot_validate_guards(
    container: &Container,
    controller_specs: &[GuardSpec],
    label: &str,
) -> Result<(), String> {
    validate_guard_chain(label, &host_chain(container, controller_specs, label)?)
}

/// The global pool, then the host-scope specs, as a request running them composes
/// them; a spec no imported module provides fails it, naming the guard and
/// `label`.
pub fn host_chain(
    container: &Container,
    host_specs: &[GuardSpec],
    label: &str,
) -> Result<Vec<ResolvedLayer<dyn Guard>>, String> {
    let global = resolve_global_guards(container);
    let host = resolve_scoped(container, host_specs, LayerSite::Host, label)
        .map_err(|unresolved| unresolved.to_string())?;
    Ok(compose_chain::<dyn Guard>(
        dedup_bucket(global),
        host,
        Vec::new(),
        &[],
        label,
    ))
}

/// The guard of a `chain` that decides who reaches what it covers: one declaring
/// that it establishes or reads who the caller is ([`GuardPhase`]). A `#[public]`
/// unit behind it at a request can open nothing; a chain of
/// [`Other`](GuardPhase::Other) guards — a rate limit — identifies no one, so it
/// covers nothing and still admits a visitor.
pub fn identifying_guard(chain: &[ResolvedLayer<dyn Guard>]) -> Option<&ResolvedLayer<dyn Guard>> {
    chain.iter().find(|entry| {
        matches!(
            entry.layer.phase(),
            GuardPhase::Authentication | GuardPhase::Authorization
        )
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use nest_rs_core::LayerSite;
    use nest_rs_core::{Layer, UnitContext};

    use super::*;
    use crate::{Denial, PrincipalClaim};

    #[derive(Clone)]
    struct ClaimsA;
    #[derive(Clone)]
    struct ClaimsB;

    struct Authn;
    impl Layer for Authn {}
    #[async_trait]
    impl Guard for Authn {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }
        fn produced_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<ClaimsA>())
        }
    }

    struct Authz;
    impl Layer for Authz {}
    #[async_trait]
    impl Guard for Authz {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authorization
        }
        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<ClaimsA>().or_visitor())
        }
    }

    /// Reads what it requires: its decision has no visitor branch.
    struct Requires;
    impl Layer for Requires {}
    #[async_trait]
    impl Guard for Requires {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<ClaimsA>())
        }
    }

    struct AuthzWantsB;
    impl Layer for AuthzWantsB {}
    #[async_trait]
    impl Guard for AuthzWantsB {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authorization
        }
        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<ClaimsB>())
        }
    }

    fn entry<G: Guard + 'static>(guard: G, name: &'static str) -> ResolvedLayer<dyn Guard> {
        ResolvedLayer {
            type_id: std::any::TypeId::of::<G>(),
            name,
            source: LayerSite::Host,
            layer: Arc::new(guard) as Arc<dyn Guard>,
        }
    }

    #[test]
    fn authn_before_authz_with_matching_principal_passes() {
        let chain = vec![entry(Authn, "Authn"), entry(Authz, "Authz")];
        validate_guard_chain("test", &chain).expect("valid chain");
    }

    #[test]
    fn reversed_order_fails_boot() {
        let chain = vec![entry(Authz, "Authz"), entry(Authn, "Authn")];
        let err = validate_guard_chain("test", &chain).expect_err("reversed order");
        assert!(err.contains("after authorization guard"), "{err}");
    }

    #[test]
    fn a_forced_authentication_replay_after_authorization_passes() {
        let mut replay = entry(Authn, "Authn");
        replay.source = LayerSite::Method;
        let chain = vec![entry(Authn, "Authn"), entry(Authz, "Authz"), replay];
        validate_guard_chain("test", &chain)
            .expect("the authorization guard read what the first authentication run attached");
    }

    #[test]
    fn principal_type_mismatch_fails_boot() {
        let chain = vec![entry(Authn, "Authn"), entry(AuthzWantsB, "AuthzWantsB")];
        let err = validate_guard_chain("test", &chain).expect_err("mismatch");
        assert!(err.contains("ClaimsB"), "{err}");
        assert!(err.contains("ClaimsA"), "{err}");
    }

    #[test]
    fn a_guard_requiring_a_principal_nothing_attaches_refuses_the_chain() {
        let err = validate_guard_chain("test", &[entry(Requires, "Requires")])
            .expect_err("every request would answer 500");
        assert!(err.contains("Requires") && err.contains("ClaimsA"), "{err}");
        validate_guard_chain(
            "test",
            &[entry(Authn, "Authn"), entry(Requires, "Requires")],
        )
        .expect("the chain attaches it");
    }

    #[test]
    fn a_unit_finds_what_it_requires_in_its_requests_chain_or_ahead_of_it() {
        let requires = [entry(Requires, "Requires")];
        let refused = validate_carried_chain("field", &requires, || Ok(Vec::new()))
            .expect_err("no guard attaches it");
        assert!(
            refused.contains("field") && refused.contains("Requires"),
            "{refused}"
        );
        validate_carried_chain("field", &requires, || Ok(vec![entry(Authn, "Authn")]))
            .expect("the request's chain attaches it");
        validate_carried_chain(
            "field",
            &[entry(Authn, "Authn"), entry(Requires, "Requires")],
            || Ok(Vec::new()),
        )
        .expect("a guard ahead of it attaches it");
    }

    #[test]
    fn a_unit_with_nothing_to_require_never_composes_its_requests_chain() {
        validate_carried_chain("field", &[entry(Authz, "Authz")], || {
            panic!("an expectation with a visitor branch reads nothing at boot")
        })
        .expect("it does without");
    }

    #[test]
    fn a_carried_unit_keeps_a_stage_for_its_guards_or_for_what_it_withholds() {
        let idle = |unit: &[ResolvedLayer<dyn Guard>], withheld, request_guarded| {
            carried_stage_idle(unit, withheld, || {
                Ok(if request_guarded {
                    vec![entry(Authn, "Authn")]
                } else {
                    Vec::new()
                })
            })
            .expect("the request composes")
        };
        assert!(idle(&[], false, false));
        assert!(idle(&[], false, true), "nothing to withhold");
        assert!(idle(&[], true, false), "nothing was attached to withhold");
        assert!(!idle(&[], true, true), "what the request attached");
        assert!(
            !idle(&[entry(Authn, "Authn")], false, false),
            "its own guard"
        );
    }

    #[test]
    fn consumer_without_any_producer_only_warns() {
        let logs = nest_rs_testing::LogCapture::install();
        let chain = vec![entry(Authz, "acme::guards::Authz")];
        validate_guard_chain("test", &chain).expect("no producer at all is a warn, not an error");

        // A warn, not a boot error: a `#[public]` route on the same chain is
        // legitimate, so this is all that is said before other routes answer 500.
        let event = logs.expect_one(
            "nest_rs::layers",
            "guard expects a principal but no guard in the chain produces one — \
             non-public routes will answer 500",
        );
        assert_eq!(event.level, "warn");
        assert_eq!(event.field("route").as_deref(), Some("test"));
        assert_eq!(event.field("guard").as_deref(), Some("Authz"));
        assert_eq!(
            event.field("expected").as_deref(),
            Some("ClaimsA"),
            "the event names the principal type nobody attaches, as an app writes it",
        );
    }
}
