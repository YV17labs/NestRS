//! A provider injecting the unit-of-work port, or the outbox store, when no
//! module this binary links lists it: the unmet dependency, in the port's
//! words. Its own binary, since one module listing either anywhere in a binary
//! makes the same app an access violation (`tests/integration` links one of
//! each).

use std::sync::Arc;

use nest_rs_core::{App, MissingDependencyError, injectable, module};
use nest_rs_database::{BACKEND_REMEDY, OutboxStore, UnitOfWork};

#[expect(
    dead_code,
    reason = "the binding is declared for the container to resolve, never read"
)]
#[injectable]
struct InjectsTheBinding {
    #[inject]
    units: Arc<dyn UnitOfWork>,
}

#[module(providers = [InjectsTheBinding])]
struct NoBindingModule;

#[tokio::test]
async fn a_provider_injecting_the_port_no_module_lists_is_told_the_first_party_binding() {
    let Err(refused) = App::builder().module::<NoBindingModule>().build().await else {
        panic!("nothing binds the port");
    };
    let missing = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (
            missing.consumer,
            missing.dependency,
            missing.remedy.as_deref()
        ),
        ("InjectsTheBinding", "dyn UnitOfWork", Some(BACKEND_REMEDY)),
    );
}

#[expect(
    dead_code,
    reason = "the store is declared for the container to resolve, never read"
)]
#[injectable]
struct ReadsTheOutbox {
    #[inject]
    store: Arc<dyn OutboxStore>,
}

#[module(providers = [ReadsTheOutbox])]
struct OutboxWithNoBinding;

#[tokio::test]
async fn a_provider_injecting_the_outbox_store_no_module_lists_is_told_the_binding() {
    let Err(refused) = App::builder().module::<OutboxWithNoBinding>().build().await else {
        panic!("no module binds the store");
    };
    let missing = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("not the unmet dependency: {refused:#}"));
    assert_eq!(
        (missing.consumer, missing.dependency),
        ("ReadsTheOutbox", "dyn OutboxStore")
    );
    assert!(
        missing
            .remedy
            .as_deref()
            .is_some_and(|remedy| remedy.contains("`nest_rs::seaorm::SeaOrmDatabaseModule`")),
        "{missing}"
    );
}
