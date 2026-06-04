//! Lives in its own binary: `inventory::submit!` is link-time global, so the
//! throwaway resolver would leak into other tests in the same binary.

use std::any::TypeId;

use nestrs_core::{
    App, ResolverDescriptor, ResolverSchemaActive, UnreachableResolversError, inventory, module,
};

struct StrayResolver;

inventory::submit! {
    ResolverDescriptor {
        resolver: || TypeId::of::<StrayResolver>(),
        name: "StrayResolver",
    }
}

#[module]
struct AppModule;

#[tokio::test]
async fn strict_mode_fails_when_a_resolver_lives_in_no_reachable_module() {
    // `ResolverSchemaActive` (which `GraphqlModule` normally provides) gates
    // the check; `App` is not `Debug`, so `expect_err` is unavailable.
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
    App::builder()
        .provide(ResolverSchemaActive)
        .module::<AppModule>()
        .build()
        .await
        .expect("default mode tolerates an unreachable resolver");
}
