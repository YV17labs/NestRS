//! A provider injecting the occurrence lock when no module this binary links
//! lists it: the unmet dependency, in the port's words. Its own binary, since
//! one module listing the lock anywhere in a binary makes the same app an
//! access violation (`tests/integration/occurrence.rs` links one).

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, injectable, module};
use nest_rs_schedule::{BACKEND_REMEDY, OccurrenceLock};

#[expect(
    dead_code,
    reason = "the lock is declared for the container to resolve, never read"
)]
#[injectable]
struct ReadsTheLock {
    #[inject]
    lock: Arc<dyn OccurrenceLock>,
}

#[module(providers = [ReadsTheLock])]
struct LockWithNoBinding;

#[tokio::test]
async fn a_provider_injecting_the_lock_no_module_lists_is_told_the_binding() {
    let Err(refused) = App::builder().module::<LockWithNoBinding>().build().await else {
        panic!("no module binds the lock");
    };
    let missing = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency, missing.remedy),
        ("ReadsTheLock", "dyn OccurrenceLock", Some(BACKEND_REMEDY)),
    );
    assert!(missing.to_string().ends_with(BACKEND_REMEDY), "{missing}");
}
