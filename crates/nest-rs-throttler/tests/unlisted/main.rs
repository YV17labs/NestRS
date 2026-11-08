//! A provider injecting the store when no module this binary links lists it:
//! the unmet dependency, in the port's words. Its own binary, since one module
//! listing the store anywhere in a binary makes the same app an access
//! violation (`tests/integration/module.rs` links one).

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, injectable, module};
use nest_rs_throttler::ThrottlerStore;

#[expect(
    dead_code,
    reason = "the store is declared for the container to resolve, never read"
)]
#[injectable]
struct ReadsTheStore {
    #[inject]
    store: Arc<dyn ThrottlerStore>,
}

#[module(providers = [ReadsTheStore])]
struct StoreWithoutThePort;

#[tokio::test]
async fn a_provider_injecting_the_store_no_module_lists_is_told_the_import() {
    let Err(refused) = App::builder().module::<StoreWithoutThePort>().build().await else {
        panic!("no module binds the store");
    };
    let missing = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency),
        ("ReadsTheStore", "dyn ThrottlerStore")
    );
    assert!(
        missing
            .remedy
            .is_some_and(|remedy| remedy.contains("`nest_rs::throttler::ThrottlerModule`")),
        "{missing}"
    );
}
