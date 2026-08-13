//! What an app has to write to rate-limit a route — the boot half of the
//! contract, as opposed to the counting behaviour the rest of this suite covers.
//!
//! The class this closes: the documented wiring is two steps — import
//! `ThrottlerModule::for_root(None)`, bind `#[use_guards(ThrottlerGuard)]` —
//! and those two steps used to fail the boot. `#[use_guards]` puts the guard
//! under the access contract, so the *controller's* module owed a provider for
//! it; no step supplied one, and a **dynamic** import (`for_root`) contributes
//! only global infrastructure, so no amount of importing could. The module now
//! registers the guard alongside the store it reads.

use std::sync::Arc;

use nest_rs_core::{App, Layer, MissingDependencyError, injectable, module};
use nest_rs_guards::{Denial, Guard, HttpGuard};
use nest_rs_http::{HttpConfig, HttpModule, async_trait, controller, routes};
use nest_rs_throttler::{ThrottlerGuard, ThrottlerModule, ThrottlerStore};

#[controller(path = "/limited")]
#[use_guards(ThrottlerGuard)]
struct LimitedController;

#[routes]
impl LimitedController {
    #[get("/")]
    async fn ping(&self) -> String {
        "pong".into()
    }
}

#[module(providers = [LimitedController])]
struct LimitedModule;

/// The composition site the [Rate limiting] page describes: the throttler goes
/// in the app's imports, the guard on the controller. Nothing in `providers`.
///
/// [Rate limiting]: https://nestrs.dev/rate-limiting/
#[module(
    imports = [
        HttpModule::for_root(HttpConfig { port: 0, ..Default::default() }),
        ThrottlerModule::for_root(None),
        LimitedModule,
    ],
)]
struct AppModule;

#[tokio::test]
async fn the_documented_two_steps_boot() {
    let app = App::builder()
        .module::<AppModule>()
        .build()
        .await
        .expect("importing ThrottlerModule is the whole wiring");

    // The guard really is resolvable — the access graph passing is necessary
    // but not sufficient (a factory could register the wrong key).
    assert!(
        app.container().get::<ThrottlerGuard>().is_some(),
        "ThrottlerModule registers the guard, not only its store",
    );
    assert!(
        app.container().get_dyn::<dyn ThrottlerStore>().is_some(),
        "…alongside the store it reads",
    );
}

// A guard nothing provides, injecting a trait object — the shape every guard,
// store and bridge takes. Its dependency has no name the graph can render, so
// the boot error used to say `<unnamed dependency>`, twice, including in the
// fix it suggested.
trait Nowhere: Send + Sync {}

#[injectable]
struct UnprovidedGuard {
    #[inject]
    _dep: Arc<dyn Nowhere>,
}

impl Layer for UnprovidedGuard {}

#[async_trait]
impl Guard for UnprovidedGuard {
    async fn check_http(&self, _req: &mut poem::Request) -> Result<(), Denial> {
        Ok(())
    }
}

impl HttpGuard for UnprovidedGuard {}

#[controller(path = "/unwired")]
#[use_guards(UnprovidedGuard)]
struct UnwiredController;

#[routes]
impl UnwiredController {
    #[get("/")]
    async fn ping(&self) -> String {
        "pong".into()
    }
}

#[module(providers = [UnwiredController])]
struct UnwiredModule;

#[module(
    imports = [
        HttpModule::for_root(HttpConfig { port: 0, ..Default::default() }),
        UnwiredModule,
    ],
)]
struct UnwiredAppModule;

/// An attribute-bound layer that no module provides must be **named**. The
/// access-graph message is the framework's best wiring diagnostic; a layer is
/// reached by `Container::get::<P>` rather than by an `#[inject]` field, and
/// the names list used to cover only the fields — so every guard, filter and
/// interceptor fell off the end of it and printed as a placeholder.
#[tokio::test]
async fn a_layer_no_module_provides_is_named_in_the_boot_error() {
    let Err(err) = App::builder().module::<UnwiredAppModule>().build().await else {
        panic!("UnprovidedGuard is provided by no module — the boot must fail");
    };

    let missing = err
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("expected an unmet-dependency boot error, got: {err:#}"));
    assert_eq!(missing.consumer, "UnwiredController");
    assert_eq!(
        missing.dependency, "UnprovidedGuard",
        "the layer is named, not `<unnamed dependency>`: {err:#}",
    );
    assert!(
        !format!("{err:#}").contains("<unnamed dependency>"),
        "…including in the suggested fix: {err:#}",
    );
}
