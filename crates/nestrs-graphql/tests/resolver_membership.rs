//! A `#[resolver]` is part of the GraphQL schema the moment it is linked — it
//! self-composes through the link-time registry, so unlike a provider (reached
//! only when injected) it is always live. The access contract therefore requires
//! it be a member of a module reachable from the root: listed in
//! `providers = [...]`, like a controller. An unlisted resolver fails the boot
//! with the named `ResolverMembershipError` (the resolver counterpart of the
//! controller `AccessGraphError`), so its `#[inject]` dependencies can never
//! escape the contract by sitting outside every module.
//!
//! One resolver per test binary: the membership check sees *every* linked
//! resolver, so a passing "listed" case cannot share a binary with an unlisted
//! one. The listed-and-boots-cleanly path is covered by the other GraphQL e2es
//! (`context`, `resolver_guard`, `authorize`), which now list their resolvers.

use nestrs_core::{module, App, ResolverMembershipError};
use nestrs_graphql::resolver;

#[resolver]
struct LooseResolver;

#[resolver]
impl LooseResolver {
    #[query]
    async fn loose(&self) -> String {
        "ok".into()
    }
}

// The breach: a module that lists no providers, so `LooseResolver` — though
// linked and thus in the schema — belongs to no module.
#[module]
struct LooseModule;

#[test]
fn an_unlisted_resolver_fails_the_boot() {
    match App::new::<LooseModule>() {
        Ok(_) => panic!("expected the boot to fail: the resolver is in no module's providers"),
        Err(err) => {
            let membership = err
                .downcast::<ResolverMembershipError>()
                .expect("the failure is the named resolver-membership error, not a panic");
            assert_eq!(membership.resolver, "LooseResolver");
            assert!(
                membership.to_string().contains("LooseResolver"),
                "{membership}"
            );
        }
    }
}
