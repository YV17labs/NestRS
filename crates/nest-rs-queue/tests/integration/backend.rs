//! Covers `src/backend.rs`: a provider pushing on a queue port no backend binds
//! is refused at boot in the port's own words, as a second backend is, and one
//! injecting a backend's contract is pointed at the port's handle.

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, injectable, module};
use nest_rs_queue::{BACKEND_REMEDY, JobProducer, Jobs};

#[expect(
    dead_code,
    reason = "the handle is declared for the container to resolve, never read"
)]
#[injectable]
struct Pusher {
    #[inject]
    jobs: Arc<Jobs>,
}

#[module(providers = [Pusher])]
struct UnboundModule;

#[tokio::test]
async fn a_producer_no_backend_binds_is_refused_naming_the_first_party_binding() {
    let Err(refused) = App::builder().module::<UnboundModule>().build().await else {
        panic!("no backend binds the queue port");
    };
    let missing = refused
        .downcast::<MissingDependencyError>()
        .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency, missing.remedy),
        ("Pusher", "Jobs", Some(BACKEND_REMEDY)),
    );
}

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
async fn a_provider_injecting_the_backend_contract_is_pointed_at_the_handle() {
    let Err(refused) = App::builder().module::<ContractModule>().build().await else {
        panic!("no binding registers a backend's contract");
    };
    let missing = refused
        .downcast::<MissingDependencyError>()
        .unwrap_or_else(|refused| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency),
        ("ContractPusher", "dyn JobProducer")
    );
    let remedy = missing.remedy.unwrap_or_default();
    assert!(
        remedy.contains("Arc<nest_rs::queue::Jobs>"),
        "the refusal names the handle to inject: {remedy}"
    );
}
