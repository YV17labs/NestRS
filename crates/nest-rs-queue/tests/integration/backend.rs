//! Covers `src/backend.rs`: a provider pushing on a queue port no backend binds
//! is refused at boot in the port's own words, as a second backend is, and one
//! injecting a backend's contract is pointed at the port's handle — unless a
//! module of the app's own registers it, which is then named.

use std::sync::Arc;

use nest_rs_core::{
    AccessGraphError, App, ContainerBuilder, MissingDependencyError, Module, Registering,
    UnbuiltCause, UnbuiltProviderError, async_trait, injectable, module,
};
use nest_rs_queue::{
    BACKEND_REMEDY, Envelope, JobProducer, Jobs, PushOptions, QueueBackend, QueueError, QueueName,
};

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

/// `AppProducerModule` below lists the contract in this binary, a module this
/// app does not reach, so the access graph says it: the port's line all the
/// same. With no module listing it, `tests/unlisted` holds the unmet
/// dependency's.
#[tokio::test]
async fn a_provider_injecting_the_backend_contract_is_pointed_at_the_handle() {
    let Err(refused) = App::builder().module::<ContractModule>().build().await else {
        panic!("no binding registers a backend's contract");
    };
    let violation = refused
        .downcast::<AccessGraphError>()
        .unwrap_or_else(|refused| panic!("not an access violation: {refused:#}"));
    assert_eq!(
        (violation.consumer, violation.dependency),
        ("ContractPusher", "dyn JobProducer")
    );
    let remedy = violation.remedy.unwrap_or_default();
    assert!(
        remedy.contains("Arc<nest_rs::queue::Jobs>"),
        "the refusal names the handle to inject: {remedy}"
    );
}

/// A producer the app binds itself.
#[injectable]
#[derive(Default)]
struct AppProducer;

static APP_BACKEND: QueueBackend = QueueBackend::new("app", nest_rs_queue::Capabilities::NONE);

#[async_trait]
impl JobProducer for AppProducer {
    fn backend(&self) -> &'static QueueBackend {
        &APP_BACKEND
    }

    async fn enqueue(
        &self,
        _: &QueueName,
        _: Vec<Envelope>,
        _: &PushOptions,
    ) -> Result<(), QueueError> {
        Ok(())
    }
}

#[module(providers = [AppProducer as dyn JobProducer])]
struct AppProducerModule;

#[module(imports = [AppProducerModule, ContractModule])]
struct ContractListedBesideItsReader;

/// The module listing the contract is the fix, whatever the port names.
#[tokio::test]
async fn a_reader_beside_the_module_listing_the_contract_is_told_that_module() {
    let Err(refused) = App::builder()
        .module::<ContractListedBesideItsReader>()
        .build()
        .await
    else {
        panic!("the reader imports nothing");
    };
    let violation = refused
        .downcast::<AccessGraphError>()
        .unwrap_or_else(|refused| panic!("not an access violation: {refused:#}"));
    assert_eq!(
        (violation.consumer, violation.owner),
        ("ContractPusher", "AppProducerModule")
    );
    let said = violation.to_string();
    assert!(said.contains("— add `AppProducerModule` to"), "{said}");
    assert!(!said.contains("Arc<nest_rs::queue::Jobs>"), "{said}");
}

#[module(imports = [AppProducerModule], providers = [ContractPusher])]
struct ReaderOverItsContract;

#[tokio::test]
async fn following_that_sentence_boots_on_the_apps_producer() {
    let app = App::builder()
        .module::<ReaderOverItsContract>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the import is the fix: {refused:#}"));
    assert!(app.container().get::<ContractPusher>().is_some());
}

/// The port's handle, which no `#[module]` can list, registered by a module the
/// app writes by hand.
struct AppJobsModule;

impl Module for AppJobsModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide(Jobs::new(Arc::new(AppProducer)))
    }
}

#[module(imports = [UnboundModule, AppJobsModule])]
struct JobsRegisteredAfterThePusher;

/// The module registering the handle is the fix, never the port's binding,
/// which would register it a second time.
#[tokio::test]
async fn a_pusher_built_before_a_hand_written_handle_is_told_that_module() {
    let Err(refused) = App::builder()
        .module::<JobsRegisteredAfterThePusher>()
        .build()
        .await
    else {
        panic!("the handle registers after its pusher");
    };
    let unbuilt = refused
        .downcast::<UnbuiltProviderError>()
        .unwrap_or_else(|refused| panic!("not the unbuilt provider: {refused:#}"));
    assert_eq!(
        unbuilt.cause,
        UnbuiltCause::RegisteredLater(vec!["AppJobsModule"])
    );
    let said = unbuilt.to_string();
    assert!(
        said.contains("`AppJobsModule` registers it later"),
        "{said}"
    );
    assert!(!said.contains(BACKEND_REMEDY), "{said}");
}

#[module(imports = [AppJobsModule], providers = [Pusher])]
struct PusherOverItsJobs;

#[tokio::test]
async fn following_that_sentence_boots_on_the_apps_handle() {
    let app = App::builder()
        .module::<PusherOverItsJobs>()
        .build()
        .await
        .unwrap_or_else(|refused| panic!("the import is the fix: {refused:#}"));
    assert!(app.container().get::<Pusher>().is_some());
}
