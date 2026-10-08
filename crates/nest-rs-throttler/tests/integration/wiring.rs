//! What an app has to write to rate-limit a route — the boot half of the
//! contract: `ThrottlerModule::for_root(None)` plus `#[use_guards(ThrottlerGuard)]`,
//! nothing in `providers`.

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

    // The access graph passing is not enough: a factory could register the wrong
    // key.
    assert!(
        app.container().get::<ThrottlerGuard>().is_some(),
        "ThrottlerModule registers the guard, not only its store",
    );
    assert!(
        app.container().get_dyn::<dyn ThrottlerStore>().is_some(),
        "…alongside the store it reads",
    );
}

// A guard nothing provides, injecting a trait object, whose dependency has no
// name the graph can render.
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

/// A layer is reached by `Container::get::<P>` rather than an `#[inject]`
/// field, so it must be named apart from the fields.
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
