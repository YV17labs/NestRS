//! Keyed / multi-instance providers (`provide_keyed`) through the real
//! `#[module]` / `#[injectable]` macros and the `App` boot path.
//!
//! Two facets are covered end to end:
//! * a `#[inject(key = "…")]` dependency with no keyed provider fails the boot
//!   with a `KeyedDependencyError` naming **both** the type and the key —
//!   validated by the access graph like a bare dependency, but with a clean
//!   boot error instead of a `get_keyed(...).expect(...)` panic;
//! * a keyed provider seeded at the composition root satisfies the boot and
//!   resolves, letting two instances of one concrete type coexist by key.
//!
//! The link-time registry is shared across a test binary, so the graphs below
//! use disjoint roots and types.

use std::sync::Arc;

use nest_rs_core::{
    App, ContainerBuilder, Discoverable, KeyedDependency, KeyedDependencyError, ProviderKey,
    injectable, module,
};

/// The keyed provider type — registered imperatively via `provide_keyed`,
/// never listed in a module's `providers = [...]`.
struct StubClient(&'static str);

/// A lazily-built consumer (controller shape): `register` is a no-op, so the
/// keyed dependency is validated by the access graph rather than resolved
/// eagerly at register time.
struct KeyedConsumer;

impl Discoverable for KeyedConsumer {
    fn injected_keyed() -> Vec<KeyedDependency> {
        vec![KeyedDependency {
            key: ProviderKey::named::<StubClient>("github"),
            type_name: "StubClient",
        }]
    }
    fn register(builder: ContainerBuilder) -> ContainerBuilder {
        builder
    }
}

#[module(providers = [KeyedConsumer])]
struct KeyedConsumerModule;

#[test]
fn a_keyed_dependency_with_no_keyed_provider_fails_the_boot_naming_type_and_key() {
    let err = match App::new::<KeyedConsumerModule>() {
        Ok(_) => panic!("a keyed dependency with no keyed provider must fail the boot"),
        Err(err) => err
            .downcast::<KeyedDependencyError>()
            .expect("the failure is the named keyed-dependency error"),
    };
    assert_eq!(err.consumer, "KeyedConsumer");
    assert_eq!(err.type_name, "StubClient");
    assert_eq!(err.key, "github");
    let msg = err.to_string();
    assert!(msg.contains("StubClient"), "names the type: {msg}");
    assert!(msg.contains("github"), "names the key: {msg}");
}

/// An eagerly-built consumer resolving a keyed singleton by key.
#[injectable]
struct EagerKeyedService {
    #[inject(key = "github")]
    client: Arc<StubClient>,
}

impl EagerKeyedService {
    fn label(&self) -> &'static str {
        self.client.0
    }
}

#[module(providers = [EagerKeyedService])]
struct EagerKeyedModule;

#[tokio::test]
async fn a_seeded_keyed_provider_satisfies_the_boot_and_resolves() {
    let app = App::builder()
        .provide_keyed("github", StubClient("gh"))
        .module::<EagerKeyedModule>()
        .build()
        .await
        .expect("a keyed provider seeded at the composition root satisfies the keyed dependency");
    let svc = app
        .container()
        .get::<EagerKeyedService>()
        .expect("the eager consumer is built");
    assert_eq!(svc.label(), "gh", "the keyed client resolves to the seeded instance");
}
