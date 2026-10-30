//! Covers `src/backend.rs`: a provider pushing on a queue port no backend binds
//! is refused at boot in the port's own words, as a second backend is.

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, injectable, module};
use nest_rs_queue::{BACKEND_REMEDY, JobProducer};

#[expect(
    dead_code,
    reason = "the producer is declared for the container to resolve, never read"
)]
#[injectable]
struct Pusher {
    #[inject]
    queue: Arc<dyn JobProducer>,
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
        ("Pusher", "dyn JobProducer", Some(BACKEND_REMEDY)),
    );
}
