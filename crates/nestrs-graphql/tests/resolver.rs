//! A `#[resolver]` self-composes into the GraphQL schema through the link-time
//! registry, so — when a schema is actually composed — it is always live, unlike a
//! provider (reached only when injected). The access contract therefore requires
//! it be a member of a module reachable from the root: listed in
//! `providers = [...]`, like a controller. An unlisted resolver fails the boot with
//! the named `ResolverMembershipError`, so its `#[inject]` dependencies can never
//! escape the contract by sitting outside every module.
//!
//! The check is scoped to apps that actually compose the schema — signalled by
//! `ResolverSchemaActive`, which the schema-composing layer (`GraphqlModule`)
//! registers. An app that links resolvers transitively (e.g. through a shared
//! library) but composes no schema is not their home, and boots cleanly.
//!
//! One resolver per test binary: the membership check sees *every* linked
//! resolver, so the two cases below share one `LooseResolver` and differ only by
//! whether a schema is composed.

use nestrs_core::{module, App, ResolverMembershipError, ResolverSchemaActive};
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

// A module that lists no providers, so `LooseResolver` — though linked — belongs
// to no module.
#[module]
struct LooseModule;

#[tokio::test]
async fn an_unlisted_resolver_fails_the_boot_when_a_schema_is_composed() {
    // `ResolverSchemaActive` stands in for the schema-composing layer: with a
    // schema live, `LooseResolver` is in it yet declared in no module.
    let result = App::builder()
        .provide(ResolverSchemaActive)
        .module::<LooseModule>()
        .build()
        .await;
    match result {
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

#[tokio::test]
async fn an_unlisted_resolver_is_ignored_when_no_schema_is_composed() {
    // No `ResolverSchemaActive`: this app composes no schema, so a linked-but-
    // unlisted resolver (e.g. pulled in transitively) must not fail its boot.
    App::builder()
        .module::<LooseModule>()
        .build()
        .await
        .expect("an app that composes no schema ignores linked resolvers");
}
