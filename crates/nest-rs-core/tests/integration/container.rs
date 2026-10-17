//! Covers `src/container.rs` — one type has one provider whatever its scope, so
//! a second registration fails the boot naming both; `Foo as dyn Trait` holds
//! one `Foo` under its own type and each trait; a replaced keyed binding warns
//! and continues.

use nest_rs_core::target;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use nest_rs_core::{
    App, Collecting, Container, ContainerBuilder, DuplicateProviderError, Module, ProviderScope,
    Registering, injectable, module,
};
use nest_rs_testing::LogCapture;

#[derive(Debug, PartialEq)]
struct Pool(&'static str);

#[test]
fn a_second_registration_under_one_key_names_the_provider_and_the_key() {
    let logs = LogCapture::install();
    let container = Container::builder()
        .provide_keyed("primary", Pool("first"))
        .provide_keyed("primary", Pool("second"))
        .build();

    let pool: Arc<Pool> = container
        .get_keyed("primary")
        .expect("the keyed provider resolves");
    assert_eq!(pool.0, "second");

    let event = logs.expect_one(target::CONTAINER, "keyed provider override");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("key").as_deref(), Some("primary"));
    assert!(
        event.field("provider").is_some_and(|p| p.contains("Pool")),
        "the event names the provider whose binding was replaced, got {:?}",
        event.fields,
    );
}

fn duplicate(boot: anyhow::Result<App>) -> DuplicateProviderError {
    let Err(refused) = boot else {
        panic!("one registration would be dropped for the other");
    };
    refused
        .downcast::<DuplicateProviderError>()
        .unwrap_or_else(|refused| panic!("not the duplicate: {refused:#}"))
}

/// The duplicate `boot` refused, checked to name `type_name` and both sites, in
/// its fields and in its sentence.
fn names_both(
    boot: anyhow::Result<App>,
    type_name: &str,
    first: &str,
    second: &str,
) -> DuplicateProviderError {
    let duplicate = duplicate(boot);
    let sentence = duplicate.to_string();
    assert!(duplicate.type_name.contains(type_name), "{sentence}");
    assert_eq!(duplicate.first, first, "{sentence}");
    assert_eq!(duplicate.second, second, "{sentence}");
    assert!(
        sentence.contains(&format!("registered by {first} and again by {second}")),
        "{sentence}"
    );
    duplicate
}

#[injectable(scope = request)]
#[derive(Default)]
struct PerRequest;

#[module(providers = [PerRequest])]
struct FirstRequestModule;

#[module(providers = [PerRequest])]
struct SecondRequestModule;

#[module(imports = [FirstRequestModule, SecondRequestModule])]
struct TwoRequestScopedRoot;

#[test]
fn a_request_scoped_provider_registered_twice_fails_the_boot_naming_both() {
    let duplicate = names_both(
        App::new::<TwoRequestScopedRoot>(),
        "PerRequest",
        "`FirstRequestModule` at `imports[0]` of `TwoRequestScopedRoot`",
        "`SecondRequestModule` at `imports[1]` of `TwoRequestScopedRoot`",
    );
    assert_eq!(
        duplicate.scopes, None,
        "one scope says no scopes: {duplicate}"
    );
}

#[injectable(scope = transient)]
#[derive(Default)]
struct Rebuilt;

#[module(providers = [Rebuilt])]
struct FirstTransientModule;

#[module(providers = [Rebuilt])]
struct SecondTransientModule;

#[module(imports = [FirstTransientModule, SecondTransientModule])]
struct TwoTransientRoot;

#[test]
fn a_transient_provider_registered_twice_fails_the_boot_naming_both() {
    let duplicate = names_both(
        App::new::<TwoTransientRoot>(),
        "Rebuilt",
        "`FirstTransientModule` at `imports[0]` of `TwoTransientRoot`",
        "`SecondTransientModule` at `imports[1]` of `TwoTransientRoot`",
    );
    assert_eq!(duplicate.scopes, None, "{duplicate}");
}

#[injectable]
#[derive(Default)]
struct Shelf;

#[module(providers = [Shelf])]
struct SingletonShelfModule;

/// Registers `Shelf` by hand under another scope than `#[injectable]` gave it.
struct TransientShelfModule;

impl Module for TransientShelfModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_transient(|_| Shelf)
    }
}

struct RequestShelfModule;

impl Module for RequestShelfModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder.provide_scoped(|_| Shelf)
    }
}

#[module(imports = [SingletonShelfModule, TransientShelfModule])]
struct SingletonThenTransientRoot;

#[module(imports = [TransientShelfModule, SingletonShelfModule])]
struct TransientThenSingletonRoot;

#[module(imports = [SingletonShelfModule, RequestShelfModule])]
struct SingletonThenRequestRoot;

#[test]
fn a_singleton_then_a_transient_of_one_type_fail_the_boot_naming_both_scopes() {
    let duplicate = names_both(
        App::new::<SingletonThenTransientRoot>(),
        "Shelf",
        "`SingletonShelfModule` at `imports[0]` of `SingletonThenTransientRoot`",
        "`TransientShelfModule` at `imports[1]` of `SingletonThenTransientRoot`",
    );
    assert_eq!(
        duplicate.scopes,
        Some((ProviderScope::Singleton, ProviderScope::Transient))
    );
    assert!(
        duplicate
            .to_string()
            .contains("(first as a singleton, then transient)"),
        "{duplicate}"
    );
}

#[test]
fn a_transient_then_a_singleton_of_one_type_fail_the_boot_naming_both_scopes() {
    let duplicate = names_both(
        App::new::<TransientThenSingletonRoot>(),
        "Shelf",
        "`TransientShelfModule` at `imports[0]` of `TransientThenSingletonRoot`",
        "`SingletonShelfModule` at `imports[1]` of `TransientThenSingletonRoot`",
    );
    assert_eq!(
        duplicate.scopes,
        Some((ProviderScope::Transient, ProviderScope::Singleton))
    );
    assert!(
        duplicate
            .to_string()
            .contains("(first transient, then as a singleton)"),
        "{duplicate}"
    );
}

#[test]
fn a_singleton_then_a_request_scoped_provider_of_one_type_fail_the_boot_naming_both_scopes() {
    let duplicate = names_both(
        App::new::<SingletonThenRequestRoot>(),
        "Shelf",
        "`SingletonShelfModule` at `imports[0]` of `SingletonThenRequestRoot`",
        "`RequestShelfModule` at `imports[1]` of `SingletonThenRequestRoot`",
    );
    assert_eq!(
        duplicate.scopes,
        Some((ProviderScope::Singleton, ProviderScope::Request))
    );
    assert!(
        duplicate
            .to_string()
            .contains("(first as a singleton, then request-scoped)"),
        "{duplicate}"
    );
}

/// Queues a factory of `Shelf`, which `SingletonShelfModule` also lists.
struct QueuesShelfModule;

impl Module for QueuesShelfModule {
    fn collect(builder: ContainerBuilder, _: Collecting<Self>) -> ContainerBuilder {
        builder.provide_factory(|_| async { Ok(Shelf) })
    }

    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        builder
    }
}

#[module(imports = [QueuesShelfModule, SingletonShelfModule])]
struct FactoryThenModuleRoot;

#[tokio::test]
async fn a_factory_output_a_module_also_lists_names_the_import_that_queued_the_factory() {
    names_both(
        App::builder()
            .module::<FactoryThenModuleRoot>()
            .build()
            .await,
        "Shelf",
        "a factory queued by `QueuesShelfModule` at `imports[0]` of `FactoryThenModuleRoot`",
        "`SingletonShelfModule` at `imports[1]` of `FactoryThenModuleRoot`",
    );
}

trait Greeting: Send + Sync {
    fn word(&self) -> &'static str;
}

#[injectable]
struct Hello;

impl Greeting for Hello {
    fn word(&self) -> &'static str {
        "hello"
    }
}

#[injectable]
struct Bonjour;

impl Greeting for Bonjour {
    fn word(&self) -> &'static str {
        "bonjour"
    }
}

#[module(providers = [Hello as dyn Greeting])]
struct HelloModule;

#[module(providers = [Bonjour as dyn Greeting])]
struct BonjourModule;

#[module(imports = [HelloModule, BonjourModule])]
struct BothGreetingsModule;

#[test]
fn two_modules_binding_one_trait_object_fail_the_boot_naming_it() {
    names_both(
        App::new::<BothGreetingsModule>(),
        std::any::type_name::<Arc<dyn Greeting>>(),
        "`HelloModule` at `imports[0]` of `BothGreetingsModule`",
        "`BonjourModule` at `imports[1]` of `BothGreetingsModule`",
    );
}

#[tokio::test]
async fn a_seeded_trait_object_a_module_also_binds_fails_the_boot_naming_the_seed() {
    let duplicate = duplicate(
        App::builder()
            .provide_dyn::<dyn Greeting>(Arc::new(Bonjour))
            .module::<HelloModule>()
            .build()
            .await,
    );
    assert_eq!(
        duplicate.type_name,
        std::any::type_name::<Arc<dyn Greeting>>()
    );
    assert_eq!(duplicate.first, "a seed (`App::builder().provide…`)");
    assert!(
        duplicate
            .second
            .contains("HelloModule`, a root the app is built from"),
        "{duplicate}"
    );
}

#[tokio::test]
async fn an_override_replaces_the_trait_object_a_module_binds() {
    let app = App::builder()
        .module::<HelloModule>()
        .override_dyn::<dyn Greeting>(Arc::new(Bonjour))
        .build()
        .await
        .expect("an override is the one replacement a binding takes");
    assert_eq!(
        app.container().get_dyn::<dyn Greeting>().map(|g| g.word()),
        Some("bonjour")
    );
}

trait Left: Send + Sync {
    fn serial(&self) -> usize;
}

trait Right: Send + Sync {
    fn serial(&self) -> usize;
}

static BOUND_BUILDS: AtomicUsize = AtomicUsize::new(0);

#[injectable]
struct Bound {
    serial: usize,
}

impl Default for Bound {
    fn default() -> Self {
        Self {
            serial: BOUND_BUILDS.fetch_add(1, Ordering::SeqCst),
        }
    }
}

impl Left for Bound {
    fn serial(&self) -> usize {
        self.serial
    }
}

impl Right for Bound {
    fn serial(&self) -> usize {
        self.serial
    }
}

#[module(providers = [Bound as dyn Left, Bound as dyn Right])]
struct BoundModule;

/// The address an `Arc` points at, whatever its pointee's metadata.
fn address<T: ?Sized>(arc: &Arc<T>) -> *const () {
    Arc::as_ptr(arc).cast::<()>()
}

#[test]
fn a_provider_bound_as_two_traits_is_one_instance_under_its_own_type_and_each_trait() {
    let app = App::new::<BoundModule>().expect("the module boots");
    let container = app.container();
    let own = container.get::<Bound>().expect("held under its own type");
    let left = container.get_dyn::<dyn Left>().expect("held as `dyn Left`");
    let right = container
        .get_dyn::<dyn Right>()
        .expect("held as `dyn Right`");

    assert_eq!(
        address(&own),
        address(&left),
        "one allocation under both keys"
    );
    assert_eq!(
        address(&own),
        address(&right),
        "one allocation under every key"
    );
    assert_eq!(left.serial(), right.serial());
    assert_eq!(
        BOUND_BUILDS.load(Ordering::SeqCst),
        1,
        "the constructor runs once for every key the provider is listed with"
    );
}

#[module(providers = [Hello])]
struct ConcreteHelloModule;

#[module(imports = [ConcreteHelloModule, HelloModule])]
struct HelloBothWaysRoot;

#[test]
fn a_provider_in_one_module_and_bound_as_dyn_in_another_is_a_duplicate_of_its_type() {
    let duplicate = names_both(
        App::new::<HelloBothWaysRoot>(),
        std::any::type_name::<Hello>(),
        "`ConcreteHelloModule` at `imports[0]` of `HelloBothWaysRoot`",
        "`HelloModule` at `imports[1]` of `HelloBothWaysRoot`",
    );
    assert_eq!(duplicate.scopes, None, "{duplicate}");
}

#[module(providers = [Hello, crate::container::Hello as dyn Greeting])]
struct TwoSpellingsModule;

#[test]
fn two_spellings_of_one_provider_in_one_list_are_a_duplicate_of_its_type() {
    let duplicate = duplicate(App::new::<TwoSpellingsModule>());
    assert_eq!(duplicate.type_name, std::any::type_name::<Hello>());
}
