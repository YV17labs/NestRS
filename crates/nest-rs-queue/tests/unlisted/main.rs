//! A provider injecting a queue backend's contract when no module this binary
//! links lists it: the unmet dependency, pointed at the port's handle. Its own
//! binary, since one module listing the contract anywhere in a binary makes
//! the same app an access violation (`tests/integration/backend.rs` links one).

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, injectable, module};
use nest_rs_queue::JobProducer;

#[expect(
    dead_code,
    reason = "the contract is declared for the container to resolve, never read"
)]
#[injectable]
struct ContractPusher {
    #[inject]
    queue: Arc<dyn JobProducer>,
}

#[module(providers = [ContractPusher])]
struct ContractModule;

#[tokio::test]
async fn a_provider_injecting_the_contract_no_module_lists_is_pointed_at_the_handle() {
    let Err(refused) = App::builder().module::<ContractModule>().build().await else {
        panic!("no binding registers a backend's contract");
    };
    let missing = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency),
        ("ContractPusher", "dyn JobProducer")
    );
    assert!(
        missing
            .remedy
            .is_some_and(|remedy| remedy.contains("Arc<nest_rs::queue::Jobs>")),
        "the refusal names the handle to inject: {missing}"
    );
}
