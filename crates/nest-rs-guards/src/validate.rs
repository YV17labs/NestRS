//! Boot-time guard-chain validation — declared [`GuardPhase`] ordering and
//! produced/expected [`PrincipalClaim`](crate::PrincipalClaim) cross-checks: a
//! chain whose declarations cannot line up fails boot with a named error
//! instead of answering `500` on every request.

use std::any::TypeId;

use nest_rs_core::__private::{
    ResolvedLayer, compose_chain, dedup_bucket, resolve_scoped, shorten,
};
use nest_rs_core::Container;
use nest_rs_core::{LayerSite, Posture};

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

/// Each guard of `chain` where it first runs: a `#[force_guards]` replay is no
/// second run of what it replays, nor a pooled guard at a unit of what its request
/// ran.
fn first_runs<'a>(
    chain: impl IntoIterator<Item = &'a ResolvedLayer<dyn Guard>>,
) -> Vec<&'a ResolvedLayer<dyn Guard>> {
    let mut runs: Vec<&ResolvedLayer<dyn Guard>> = Vec::new();
    for entry in chain {
        if !runs.iter().any(|run| run.type_id == entry.type_id) {
            runs.push(entry);
        }
    }
    runs
}

/// Refuse an `Authentication`-phase guard running after an `Authorization`-phase
/// one in `runs`: what the second read was not attached yet.
fn validate_phase_order(label: &str, runs: &[&ResolvedLayer<dyn Guard>]) -> Result<(), String> {
    let mut saw_authorization: Option<&'static str> = None;
    for entry in runs {
        match entry.layer.phase() {
            GuardPhase::Authorization => saw_authorization = Some(entry.name),
            GuardPhase::Authentication => {
                if let Some(authz) = saw_authorization {
                    return Err(format!(
                        "guard chain for {label} lists authentication guard `{}` after \
                         authorization guard `{}` — authentication must run first \
                         so the principal it attaches is available, and a request's guards \
                         run ahead of the units it carries; reorder the `use_guards` declaration",
                        shorten(entry.name),
                        shorten(authz),
                    ));
                }
            }
            GuardPhase::Other => {}
        }
    }
    Ok(())
}

fn validate(
    label: &str,
    chain: &[ResolvedLayer<dyn Guard>],
    warns: impl Fn(&ResolvedLayer<dyn Guard>) -> bool,
) -> Result<(), String> {
    let runs = first_runs(chain);
    let chain = runs.as_slice();
    validate_phase_order(label, chain)?;

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
                    site = label,
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

/// Validate a unit an in-band edge carries, declaring `posture`, whose `request` runs
/// ahead of its `unit` chain: the principals its guards require
/// (`validate_carried_chain`), then the ability what reads one finds attached
/// (`validate_carried_ability_reached`), `class_gated` when its posture declares a
/// class gate, `opened` when its endpoint was opened, its request running no guard.
pub fn validate_carried_unit(
    label: &str,
    posture: Posture,
    class_gated: bool,
    opened: bool,
    unit: &[ResolvedLayer<dyn Guard>],
    request: impl Fn() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
) -> Result<(), String> {
    validate_carried_chain(label, unit, &request)?;
    let site = CarriedUnit {
        label,
        posture,
        class_gated,
        opened,
    };
    validate_carried_ability_reached(&site, unit, request)
}

/// [`validate_guard_chain`] on a unit an in-band edge carries, whose `request`
/// runs ahead of its `unit` chain: no authentication guard follows an authorization
/// guard across the two, and each guard of `unit` requiring a principal finds a
/// guard attaching it in either. The closure composes the request only for a chain
/// holding such a guard, and a guard doing without its principal reads what it
/// finds.
fn validate_carried_chain(
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
    let authenticates = unit
        .iter()
        .any(|entry| entry.layer.phase() == GuardPhase::Authentication);
    if required.is_empty() && !authenticates {
        return Ok(());
    }
    let request = request()?;
    validate_phase_order(label, &first_runs(request.iter().chain(unit)))?;
    for (at, guard, expected) in required {
        let attached = request
            .iter()
            .chain(&unit[..at])
            .any(|entry| produces(entry, expected.type_id));
        if !attached {
            return Err(unattached(label, guard, expected));
        }
    }
    Ok(())
}

/// Refuse a class gate on a route no guard of its `chain` attaches the ability for:
/// every call would answer `500`.
pub fn validate_gate_reached(
    label: &str,
    chain: &[ResolvedLayer<dyn Guard>],
) -> Result<(), String> {
    if chain.iter().any(|entry| entry.layer.__attaches_ability()) {
        return Ok(());
    }
    Err(unreached(label))
}

/// A unit an in-band edge carries, as a refusal names it: its `label`, its `posture`,
/// whether that posture is `class_gated`, and whether its endpoint is `opened`, its
/// request running no guard.
struct CarriedUnit<'a> {
    label: &'a str,
    posture: Posture,
    class_gated: bool,
    opened: bool,
}

/// Refuse the unit `site` names, where what reads the ability would find none
/// attached, class gate or not: a guard of its `unit` chain reading the one its
/// request attached, which it builds for another posture alone (an ability guard at a
/// gated unit), with no guard ahead of it or at its `request` attaching one, for every
/// call would answer `401`; or a class gate no guard attaches it for, at the unit or
/// its request, for every call would answer `500`. The closure composes the request
/// only where the unit's chain answers neither.
fn validate_carried_ability_reached(
    site: &CarriedUnit<'_>,
    unit: &[ResolvedLayer<dyn Guard>],
    request: impl FnOnce() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
) -> Result<(), String> {
    let attaches =
        |entry: &ResolvedLayer<dyn Guard>| entry.layer.__attaches_ability_to(site.posture);
    let reader = unit
        .iter()
        .position(|entry| entry.layer.__attaches_ability() && !attaches(entry));
    let answered = match reader {
        Some(at) => unit[..at].iter().any(attaches),
        None => !site.class_gated || unit.iter().any(attaches),
    };
    if answered
        || request()?
            .iter()
            .any(|entry| entry.layer.__attaches_ability())
    {
        return Ok(());
    }
    Err(match reader {
        Some(at) => site.read_unattached(unit[at].name),
        None => site.unreached(),
    })
}

impl CarriedUnit<'_> {
    /// The refusal of `guard` at the unit reading the ability its request attached,
    /// where nothing attaches one ahead of it: every call would answer `401`.
    fn read_unattached(&self, guard: &str) -> String {
        let posture = if self.class_gated {
            "declares `#[authorize]`"
        } else {
            "is gated by its guards alone"
        };
        let remedy = if self.opened {
            let public = if self.class_gated {
                "declare it `#[public(Action, Subject)]`, which keeps its gate"
            } else {
                "declare it `#[public]`, or `#[public(Action, Subject)]` to gate it on a subject"
            };
            format!(
                "its endpoint is opened, so no guard runs at its request, and every call would \
                 answer `401` — {public}, or stop opening the endpoint and bind `AbilityGuard` \
                 over your `AbilityFactory` at its request, in `GlobalGuards<(...)>` or \
                 `EndpointGuards<View, (...)>`"
            )
        } else {
            let or_public = if self.class_gated {
                ", or declare the unit `#[public(Action, Subject)]`"
            } else {
                ""
            };
            format!(
                "no guard ahead of it or at its request attaches it, so every call would answer \
                 `401` — bind `AbilityGuard` over your `AbilityFactory` at its request, in \
                 `EndpointGuards<View, (...)>`, `GlobalGuards<(...)>` or a gateway's \
                 `#[use_guards]`{or_public}"
            )
        };
        format!(
            "{} {posture}, and `{}` at the unit reads the ability its request attached, \
             building one on a `#[public]` unit alone: {remedy}",
            self.label,
            shorten(guard),
        )
    }

    /// The refusal of its class gate, no guard reaching the unit attaching the ability
    /// the gate reads: every call would answer `500`.
    fn unreached(&self) -> String {
        if !self.opened {
            return unreached(self.label);
        }
        let public = if self.posture == Posture::Public {
            ""
        } else {
            ", and declare the unit `#[public(Action, Subject)]`"
        };
        format!(
            "{} declares a class gate (`#[authorize]` or `#[public(Action, Subject)]`), and no \
             guard in its chain attaches the ability the gate reads, its endpoint opened so that \
             no guard runs at its request: every call would answer `500` — bind `AbilityGuard` \
             over your `AbilityFactory` at the unit, with `#[use_guards]` or \
             `GlobalGuards<(...)>`{public}, or stop opening the endpoint and bind it at its \
             request, in `GlobalGuards<(...)>` or `EndpointGuards<View, (...)>`",
            self.label,
        )
    }
}

/// The refusal of a class gate no guard reaching its unit attaches the ability for.
fn unreached(label: &str) -> String {
    format!(
        "{label} declares a class gate (`#[authorize]` or `#[public(Action, Subject)]`), and no \
         guard in its chain or at its request attaches the ability the gate reads, so every call \
         would answer `500`: bind `AbilityGuard` over your `AbilityFactory` with `#[use_guards]`, \
         `GlobalGuards<(...)>` or, on an in-band endpoint, `EndpointGuards<View, (...)>`"
    )
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
    chain.iter().find(|entry| identifies(entry))
}

/// Whether `entry` declares that it establishes or reads who the caller is.
pub fn identifies(entry: &ResolvedLayer<dyn Guard>) -> bool {
    matches!(
        entry.layer.phase(),
        GuardPhase::Authentication | GuardPhase::Authorization
    )
}

/// Whether `entry` declares that it attaches the principal of the type
/// `principal` names.
pub fn produces(entry: &ResolvedLayer<dyn Guard>, principal: TypeId) -> bool {
    entry
        .layer
        .produced_principal()
        .is_some_and(|produced| produced.type_id == principal)
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

    /// Attaches what a class gate reads.
    struct Ability;
    impl Layer for Ability {}
    #[async_trait]
    impl Guard for Ability {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
        fn __attaches_ability(&self) -> bool {
            true
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
    fn a_carried_chain_listing_authentication_after_authorization_is_refused() {
        let refused = validate_carried_chain(
            "field",
            &[entry(Authz, "Authz"), entry(Authn, "Authn")],
            || Ok(Vec::new()),
        )
        .expect_err("what authorization read was not attached yet");
        assert!(refused.contains("field"), "{refused}");
        assert!(refused.contains("after authorization guard"), "{refused}");
    }

    #[test]
    fn a_carried_chain_authenticating_after_its_requests_authorization_is_refused() {
        let refused = validate_carried_chain("field", &[entry(Authn, "Authn")], || {
            Ok(vec![entry(Authz, "Authz")])
        })
        .expect_err("the ability its request built read no principal");
        assert!(refused.contains("after authorization guard"), "{refused}");
    }

    #[test]
    fn a_carried_chain_replaying_what_its_request_ran_keeps_its_first_run() {
        let request = || Ok(vec![entry(Authn, "Authn"), entry(Authz, "Authz")]);
        validate_carried_chain(
            "field",
            &[entry(Authn, "Authn"), entry(Authz, "Authz")],
            request,
        )
        .expect("the pool at a unit is what its request already ran");
        let mut replay = entry(Authn, "Authn");
        replay.source = LayerSite::Method;
        validate_carried_chain(
            "field",
            &[entry(Authn, "Authn"), entry(Authz, "Authz"), replay],
            || Ok(Vec::new()),
        )
        .expect("a forced replay read what the first run attached");
    }

    #[test]
    fn a_unit_with_nothing_to_require_never_composes_its_requests_chain() {
        validate_carried_chain("field", &[entry(Authz, "Authz")], || {
            panic!("an expectation with a visitor branch reads nothing at boot")
        })
        .expect("it does without");
    }

    /// Attaches it at the request, and at a carried unit only when it is `#[public]`,
    /// as `AbilityGuard` does.
    struct PublicAbility;
    impl Layer for PublicAbility {}
    #[async_trait]
    impl Guard for PublicAbility {
        async fn check(&self, _cx: &mut UnitContext<'_>) -> Result<(), Denial> {
            Ok(())
        }
        fn __attaches_ability(&self) -> bool {
            true
        }
        fn __attaches_ability_to(&self, posture: Posture) -> bool {
            posture == Posture::Public
        }
    }

    /// [`validate_carried_ability_reached`] on a unit `field` its endpoint's request
    /// guards, or runs no guard for when `opened`.
    fn reached_at(
        posture: Posture,
        class_gated: bool,
        opened: bool,
        unit: &[ResolvedLayer<dyn Guard>],
        request: impl FnOnce() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
    ) -> Result<(), String> {
        let site = CarriedUnit {
            label: "field",
            posture,
            class_gated,
            opened,
        };
        validate_carried_ability_reached(&site, unit, request)
    }

    /// [`reached_at`] a guarded endpoint.
    fn ability_reached(
        posture: Posture,
        class_gated: bool,
        unit: &[ResolvedLayer<dyn Guard>],
        request: impl FnOnce() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
    ) -> Result<(), String> {
        reached_at(posture, class_gated, false, unit, request)
    }

    #[test]
    fn a_routes_class_gate_finds_its_ability_guard_in_its_chain() {
        let refused = validate_gate_reached("GET /", &[entry(Authn, "Authn")])
            .expect_err("nothing attaches the ability");
        assert!(
            refused.contains("GET /") && refused.contains("`500`"),
            "{refused}"
        );
        validate_gate_reached(
            "GET /",
            &[entry(Authn, "Authn"), entry(PublicAbility, "Ability")],
        )
        .expect("its chain attaches it");
    }

    #[test]
    fn a_class_gate_finds_its_ability_guard_in_its_chain_or_at_its_request() {
        fn reached(
            unit: &[ResolvedLayer<dyn Guard>],
            request: impl FnOnce() -> Result<Vec<ResolvedLayer<dyn Guard>>, String>,
        ) -> Result<(), String> {
            ability_reached(Posture::Gated, true, unit, request)
        }
        let refused = reached(&[entry(Authn, "Authn")], || Ok(vec![entry(Authz, "Authz")]))
            .expect_err("neither attaches the ability");
        assert!(
            refused.contains("field") && refused.contains("AbilityGuard"),
            "{refused}"
        );
        assert!(refused.contains("`500`"), "{refused}");
        reached(&[entry(Ability, "Ability")], || {
            panic!("the unit's own chain already answers")
        })
        .expect("its chain attaches it");
        reached(&[], || Ok(vec![entry(Ability, "Ability")])).expect("its request attaches it");
        reached(&[entry(Arc::new(Ability), "Ability")], || Ok(Vec::new()))
            .expect("a shared guard answers as the guard it holds");
    }

    #[test]
    fn a_guard_attaching_at_a_public_unit_alone_reaches_no_gated_one() {
        let at_the_unit = || [entry(Arc::new(PublicAbility), "PublicAbility")];
        for class_gated in [true, false] {
            ability_reached(Posture::Public, class_gated, &at_the_unit(), || {
                panic!("the unit's own chain already answers")
            })
            .expect("a public unit's guard builds the ability");
            let refused = ability_reached(Posture::Gated, class_gated, &at_the_unit(), || {
                Ok(vec![entry(Authn, "Authn")])
            })
            .expect_err("a gated unit reads what its request attached");
            assert!(
                refused.contains("`PublicAbility`") && refused.contains("`401`"),
                "names the guard and what every call would answer: {refused}",
            );
            assert_eq!(
                refused.contains("#[authorize]"),
                class_gated,
                "names the posture it reads: {refused}",
            );
            ability_reached(Posture::Gated, class_gated, &at_the_unit(), || {
                Ok(vec![entry(PublicAbility, "PublicAbility")])
            })
            .expect("its request attaches it");
        }
    }

    /// An opened endpoint's request runs no guard, so binding one there lifts nothing:
    /// the refusal names the posture that does, or not opening the endpoint.
    #[test]
    fn a_unit_of_an_opened_endpoint_is_told_the_posture_that_lifts_its_refusal() {
        let opened = |posture, class_gated, unit: &[ResolvedLayer<dyn Guard>]| {
            reached_at(posture, class_gated, true, unit, || Ok(Vec::new()))
                .expect_err("nothing attaches the ability")
        };
        let reading = [entry(PublicAbility, "PublicAbility")];
        for (class_gated, public) in [
            (
                false,
                "declare it `#[public]`, or `#[public(Action, Subject)]`",
            ),
            (
                true,
                "declare it `#[public(Action, Subject)]`, which keeps its gate",
            ),
        ] {
            let refused = opened(Posture::Gated, class_gated, &reading);
            assert!(
                refused.contains("`401`")
                    && refused.contains("its endpoint is opened, so no guard runs at its request")
                    && refused.contains(public)
                    && refused.contains("stop opening the endpoint"),
                "{refused}",
            );
            assert!(!refused.contains("a gateway's"), "{refused}");
        }
        for (posture, public) in [(Posture::Gated, true), (Posture::Public, false)] {
            let refused = opened(posture, true, &[entry(Authn, "Authn")]);
            assert!(
                refused.contains("`500`")
                    && refused.contains("its endpoint opened")
                    && refused
                        .contains("at the unit, with `#[use_guards]` or `GlobalGuards<(...)>`")
                    && refused.contains("stop opening the endpoint"),
                "{refused}",
            );
            assert_eq!(
                refused.contains("declare the unit `#[public(Action, Subject)]`"),
                public,
                "an `#[authorize]` unit is told to say `#[public(Action, Subject)]`: {refused}",
            );
        }
    }

    #[test]
    fn a_guard_reading_the_ability_at_the_unit_finds_it_attached_ahead_of_it() {
        let reached = |unit: &[ResolvedLayer<dyn Guard>]| {
            ability_reached(Posture::Gated, false, unit, || Ok(Vec::new()))
        };
        reached(&[
            entry(Ability, "Ability"),
            entry(PublicAbility, "PublicAbility"),
        ])
        .expect("the guard ahead of it attaches it");
        let refused = reached(&[
            entry(PublicAbility, "PublicAbility"),
            entry(Ability, "Ability"),
        ])
        .expect_err("it runs before the guard attaching it");
        assert!(refused.contains("`PublicAbility`"), "{refused}");
    }

    #[test]
    fn a_unit_no_class_gate_and_no_reading_guard_holds_never_composes_its_request() {
        for unit in [vec![], vec![entry(Authn, "Authn"), entry(Authz, "Authz")]] {
            ability_reached(Posture::Gated, false, &unit, || {
                panic!("nothing at the unit reads an ability")
            })
            .expect("nothing reads one");
        }
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
        assert_eq!(event.field("site").as_deref(), Some("test"));
        assert_eq!(event.field("guard").as_deref(), Some("Authz"));
        assert_eq!(
            event.field("expected").as_deref(),
            Some("ClaimsA"),
            "the event names the principal type nobody attaches, as an app writes it",
        );
    }
}
