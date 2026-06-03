//! `AppBuilder::strict_resolver_membership` turns the default unreachable-
//! resolver `warn` into a boot [`UnreachableResolversError`] — every linked
//! `#[resolver]` must live in a reachable module, or the boot fails.
//!
//! Lives in its own binary because `inventory::submit!` is link-time global:
//! the throwaway resolver below would leak into every other test in the same
//! binary and pollute their boots.

use std::any::TypeId;

use nestrs_core::{
    inventory, module, App, ResolverDescriptor, ResolverSchemaActive, UnreachableResolversError,
};

/// A throwaway `#[resolver]` stand-in: a struct + a manual `ResolverDescriptor`
/// submission. The `#[resolver]` macro would emit the same submission, but the
/// macro lives in `nestrs-graphql-macros` and we want this test free of any
/// GraphQL surface.
struct StrayResolver;

inventory::submit! {
    ResolverDescriptor {
        resolver: || TypeId::of::<StrayResolver>(),
        name: "StrayResolver",
    }
}

/// An app that **does not** list `StrayResolver` in any reachable module. The
/// stray resolver is therefore unreachable from `AppModule`.
#[module]
struct AppModule;

#[tokio::test]
async fn strict_mode_fails_when_a_resolver_lives_in_no_reachable_module() {
    // `ResolverSchemaActive` is what tells the boot a schema is composed (the
    // real `GraphqlModule` provides it). Without it the check is skipped.
    // `App` is not `Debug`, so `expect_err` is unavailable — match the result
    // by hand.
    let err = match App::builder()
        .provide(ResolverSchemaActive)
        .module::<AppModule>()
        .strict_resolver_membership()
        .build()
        .await
    {
        Ok(_) => panic!("strict mode must surface the unreachable resolver"),
        Err(e) => e,
    };
    let unreachable = err
        .downcast::<UnreachableResolversError>()
        .expect("the named error, not a generic boot panic");
    assert!(
        unreachable.0.contains(&"StrayResolver"),
        "expected `StrayResolver` in {:?}",
        unreachable.0,
    );
}

#[tokio::test]
async fn default_mode_boots_with_only_a_warn() {
    // The same shape without `strict_resolver_membership()` boots; the warning
    // surfaces in logs but does not abort.
    App::builder()
        .provide(ResolverSchemaActive)
        .module::<AppModule>()
        .build()
        .await
        .expect("default mode tolerates an unreachable resolver");
}
