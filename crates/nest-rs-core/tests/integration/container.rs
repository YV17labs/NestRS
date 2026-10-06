//! Covers `src/container.rs` — the registrations the container accepts while
//! saying it did something surprising.
//!
//! Two shapes, and the difference is deliberate. A duplicate *unkeyed* provider
//! is a boot error: nothing can distinguish the two, so one of them was going to
//! be silently unreachable. A duplicate *keyed* one, and a scope that changes
//! under a type, are legal — an app may genuinely want to replace a keyed
//! binding — so they warn and continue.
//!
//! Which makes the event the whole safety net: after it, resolution answers
//! normally and the losing registration is simply gone. Nothing read these, so
//! a module quietly shadowing another module's provider was invisible.

use nest_rs_core::target;
use std::sync::Arc;

use nest_rs_core::Container;
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

    // The last registration wins, silently as far as any caller can tell.
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

#[test]
fn a_singleton_shadowed_by_a_transient_of_the_same_type_is_reported() {
    let logs = LogCapture::install();
    // Registering both is what makes the singleton unreachable: resolution
    // answers with a fresh transient build every time, so whatever state the
    // singleton held is never read again.
    let _container = Container::builder()
        .provide(Pool("singleton"))
        .provide_transient(|_| Pool("transient"))
        .build();

    let event = logs.expect_one(target::CONTAINER, "provider scope conflict");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("existing_kind").as_deref(), Some("singleton"));
    assert_eq!(event.field("new_kind").as_deref(), Some("transient"));
}

#[test]
fn the_conflict_is_reported_from_either_direction() {
    let logs = LogCapture::install();
    // The mirror registration order: the warning has to fire whichever module
    // happened to be imported first, or the diagnostic depends on `imports`
    // ordering — which is the class of bug it exists to surface.
    let _container = Container::builder()
        .provide_transient(|_| Pool("transient"))
        .provide(Pool("singleton"))
        .build();

    let event = logs.expect_one(target::CONTAINER, "provider scope conflict");
    assert_eq!(event.field("existing_kind").as_deref(), Some("transient"));
    assert_eq!(event.field("new_kind").as_deref(), Some("singleton"));
}

#[test]
fn a_request_scoped_factory_replaced_by_another_says_which_kind_it_was() {
    let logs = LogCapture::install();
    // The third survivable conflict, and the one whose kind field matters: a
    // request-scoped binding replaced by another is legal, so the only way to
    // notice that two modules both claim the type is this line.
    let _container = Container::builder()
        .provide_scoped(|_| Pool("first"))
        .provide_scoped(|_| Pool("second"))
        .build();

    let event = logs.expect_one(target::CONTAINER, "provider override");
    assert_eq!(event.level, "warn");
    assert_eq!(event.field("kind").as_deref(), Some("request_scoped"));
    assert!(
        event.field("provider").is_some_and(|p| p.contains("Pool")),
        "{:?}",
        event.fields,
    );
}

trait Greeting: Send + Sync {
    fn word(&self) -> &'static str;
}

#[nest_rs_core::injectable]
struct Hello;

impl Greeting for Hello {
    fn word(&self) -> &'static str {
        "hello"
    }
}

#[nest_rs_core::injectable]
struct Bonjour;

impl Greeting for Bonjour {
    fn word(&self) -> &'static str {
        "bonjour"
    }
}

#[nest_rs_core::module(providers = [Hello as dyn Greeting])]
struct HelloModule;

#[nest_rs_core::module(providers = [Bonjour as dyn Greeting])]
struct BonjourModule;

#[nest_rs_core::module(imports = [HelloModule, BonjourModule])]
struct BothGreetingsModule;

/// The boot's refusal, as the duplicate it must be.
fn duplicate(boot: anyhow::Result<nest_rs_core::App>) -> nest_rs_core::DuplicateProviderError {
    let Err(refused) = boot else {
        panic!("one binding would be dropped for the other");
    };
    refused
        .downcast::<nest_rs_core::DuplicateProviderError>()
        .unwrap_or_else(|refused| panic!("not the duplicate: {refused:#}"))
}

#[test]
fn two_modules_binding_one_trait_object_fail_the_boot_naming_it() {
    let duplicate = duplicate(nest_rs_core::App::new::<BothGreetingsModule>());
    assert_eq!(
        duplicate.type_name,
        std::any::type_name::<Arc<dyn Greeting>>()
    );
}

#[tokio::test]
async fn a_seeded_trait_object_a_module_also_binds_fails_the_boot() {
    let duplicate = duplicate(
        nest_rs_core::App::builder()
            .provide_dyn::<dyn Greeting>(Arc::new(Bonjour))
            .module::<HelloModule>()
            .build()
            .await,
    );
    assert_eq!(
        duplicate.type_name,
        std::any::type_name::<Arc<dyn Greeting>>()
    );
}

#[tokio::test]
async fn an_override_replaces_the_trait_object_a_module_binds() {
    let app = nest_rs_core::App::builder()
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
