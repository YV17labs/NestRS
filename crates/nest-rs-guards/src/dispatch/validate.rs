//! Boot-time guard-chain validation — declared [`GuardPhase`] ordering and
//! produced/expected [`PrincipalClaim`] cross-checks. Replaces the former
//! name-substring ordering heuristic: guards **declare** their phase and the
//! principal type they attach or expect, and a chain whose declarations
//! cannot line up fails boot with a named error instead of answering `500`
//! on every request.

use nest_rs_core::Container;
use nest_rs_core::layer_chain::{LayerSite, ResolvedLayer, compose_chain, dedup_bucket};

use crate::dispatch::scoped_spec::{ScopedGuardSpec, resolve_global_guards, resolve_specs};
use crate::{Guard, GuardPhase};

/// Validate one resolved guard chain (in execution order).
///
/// Fails on: an `Authentication`-phase guard listed after an
/// `Authorization`-phase one; a guard expecting a principal type produced
/// only *later* in the chain; a guard expecting a principal type while the
/// chain's producer(s) attach a **different** type. A consumer with no
/// producer anywhere in the chain only warns — a controller may serve
/// exclusively `#[public]` routes, where the authorization guard admits the
/// anonymous actor.
pub fn validate_guard_chain(label: &str, chain: &[ResolvedLayer<dyn Guard>]) -> Result<(), String> {
    let mut saw_authorization: Option<&'static str> = None;
    for entry in chain {
        match entry.layer.phase() {
            GuardPhase::Authorization => saw_authorization = Some(entry.name),
            GuardPhase::Authentication => {
                if let Some(authz) = saw_authorization {
                    return Err(format!(
                        "guard chain for {label} lists authentication guard `{}` after \
                         authorization guard `{authz}` — authentication must run first \
                         so the principal it attaches is available; reorder the \
                         `use_guards` declaration",
                        entry.name,
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
                return Err(format!(
                    "guard chain for {label}: `{}` expects principal `{}` which `{late_name}` \
                     only attaches later in the chain — list `{late_name}` before `{}`",
                    entry.name, expected.type_name, entry.name,
                ));
            }
            None if !producers.is_empty() => {
                let produced: Vec<&str> = producers.iter().map(|(_, _, c)| c.type_name).collect();
                return Err(format!(
                    "guard chain for {label}: `{}` expects principal `{}` but the chain's \
                     authentication guard(s) attach [{}] — the auth strategy's principal and \
                     the ability factory's `Actor` must be the same type",
                    entry.name,
                    expected.type_name,
                    produced.join(", "),
                ));
            }
            None => {
                tracing::warn!(
                    target: "nest_rs::layers",
                    route = label,
                    guard = entry.name,
                    expected = expected.type_name,
                    "guard expects a principal but no guard in the chain produces one — \
                     non-public routes will answer 500",
                );
            }
        }
    }
    Ok(())
}

/// Boot check for one controller: compose the global pool with the
/// controller-scope specs (same dedup as the route shaper) and validate the
/// result. Method-scope guards compose per route at mount; the canonical
/// authn/authz pairing is declared at controller or global scope, which this
/// covers.
pub fn boot_validate_guards(
    container: &Container,
    controller_specs: &[ScopedGuardSpec],
    label: &str,
) -> Result<(), String> {
    let global = resolve_global_guards(container);
    let controller = resolve_specs(container, controller_specs, LayerSite::Controller);
    let chain =
        compose_chain::<dyn Guard>(dedup_bucket(global), controller, Vec::new(), &[], label);
    validate_guard_chain(label, &chain)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use nest_rs_core::Layer;
    use nest_rs_core::layer_chain::LayerSite;

    use super::*;
    use crate::PrincipalClaim;

    #[derive(Clone)]
    struct ClaimsA;
    #[derive(Clone)]
    struct ClaimsB;

    struct Authn;
    impl Layer for Authn {}
    impl Guard for Authn {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authentication
        }
        fn produced_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<ClaimsA>())
        }
    }

    struct Authz;
    impl Layer for Authz {}
    impl Guard for Authz {
        fn phase(&self) -> GuardPhase {
            GuardPhase::Authorization
        }
        fn expected_principal(&self) -> Option<PrincipalClaim> {
            Some(PrincipalClaim::of::<ClaimsA>())
        }
    }

    struct AuthzWantsB;
    impl Layer for AuthzWantsB {}
    impl Guard for AuthzWantsB {
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
            source: LayerSite::Controller,
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
    fn principal_type_mismatch_fails_boot() {
        let chain = vec![entry(Authn, "Authn"), entry(AuthzWantsB, "AuthzWantsB")];
        let err = validate_guard_chain("test", &chain).expect_err("mismatch");
        assert!(err.contains("ClaimsB"), "{err}");
        assert!(err.contains("ClaimsA"), "{err}");
    }

    #[test]
    fn consumer_without_any_producer_only_warns() {
        let chain = vec![entry(Authz, "Authz")];
        validate_guard_chain("test", &chain).expect("no producer at all is a warn, not an error");
    }
}
