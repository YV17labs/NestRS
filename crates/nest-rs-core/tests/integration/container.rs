//! Covers `src/container.rs` — a duplicate unkeyed provider fails the boot; a
//! replaced keyed binding or a scope change under a type warns and continues.

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
