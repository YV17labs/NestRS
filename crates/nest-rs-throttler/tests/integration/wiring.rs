//! What an app has to write to rate-limit a route — the boot half of the
//! contract: `ThrottlerModule` plus `#[use_guards(ThrottlerGuard)]`, nothing in
//! `providers`, and the route's `#[public]`, since a rate limit identifies no one.

use std::sync::Arc;

use nest_rs_core::{App, Layer, MissingDependencyError, async_trait, injectable, module};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{HttpConfig, HttpModule, controller, routes};
use nest_rs_throttler::{ThrottlerGuard, ThrottlerModule, ThrottlerStore};

#[controller(path = "/limited")]
#[use_guards(ThrottlerGuard)]
struct LimitedController;

#[routes]
impl LimitedController {
    #[get("/")]
    #[public]
    async fn ping(&self) -> String {
        "pong".into()
    }
}

#[module(providers = [LimitedController])]
struct LimitedModule;

#[module(
    imports = [
        HttpModule::for_root(HttpConfig { port: 0, ..Default::default() }),
        ThrottlerModule,
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

#[controller(path = "/silent")]
#[use_guards(ThrottlerGuard)]
struct SilentController;

#[routes]
impl SilentController {
    #[get("/")]
    async fn silent(&self) -> String {
        "reached by any caller, with no line saying so".into()
    }
}

#[module(imports = [ThrottlerModule], providers = [SilentController])]
struct SilentModule;

#[tokio::test]
async fn a_route_under_the_throttler_alone_refuses_the_boot_naming_it() {
    let Err(refused) = nest_rs_testing::TestApp::for_module::<SilentModule>().await else {
        panic!("a rate limit identifies no one, so it opens the route with no line");
    };
    let said = format!("{refused:#}");
    assert!(said.contains("GET /silent (silent)"), "{said}");
    assert!(said.contains("#[public]"), "{said}");
}

/// The pool the rate-limiting page binds, over a route, a message and an operation
/// each stating no posture, and a route saying what it opens.
mod under_the_pool {
    use nest_rs_core::module;
    use nest_rs_guards::GlobalGuards;
    use nest_rs_http::{HttpModule, controller, routes};
    use nest_rs_mcp::{McpError, mcp, tools};
    use nest_rs_testing::TestApp;
    use nest_rs_throttler::{ThrottlerGuard, ThrottlerModule};
    use nest_rs_ws::{WsModule, gateway, messages};

    #[controller(path = "/pooled")]
    struct PooledController;

    #[routes]
    impl PooledController {
        #[get("/silent")]
        async fn silent(&self) -> &'static str {
            "reached by any caller"
        }

        #[get("/said")]
        #[public]
        async fn said(&self) -> &'static str {
            "the opening it says"
        }
    }

    #[gateway(path = "/pooled/ws")]
    struct PooledGateway;

    #[messages]
    impl PooledGateway {
        #[subscribe_message("silent")]
        async fn silent(&self) -> &'static str {
            "reached by any caller"
        }
    }

    #[mcp(path = "/pooled/mcp")]
    #[derive(Clone, Default)]
    struct PooledTool;

    #[tools]
    impl PooledTool {
        /// Silent.
        #[tool]
        async fn silent(&self) -> Result<String, McpError> {
            Ok("reached by any caller".to_owned())
        }
    }

    async fn refusal<M: nest_rs_core::Module + 'static>() -> String {
        match TestApp::for_module::<M>().await {
            Ok(_) => panic!("a rate limit identifies no one, so it opens no unit"),
            Err(refused) => format!("{refused:#}"),
        }
    }

    #[module(imports = [HttpModule, ThrottlerModule], providers = [PooledController])]
    struct RouteModule;

    #[module(imports = [RouteModule], providers = [GlobalGuards<(ThrottlerGuard,)>])]
    struct RouteApp;

    #[tokio::test]
    async fn a_route_refuses_the_boot_naming_it_and_not_the_one_saying_public() {
        let said = refusal::<RouteApp>().await;
        assert!(said.contains("GET /pooled/silent (silent)"), "{said}");
        assert!(!said.contains("/pooled/said"), "{said}");
    }

    #[module(imports = [WsModule, ThrottlerModule], providers = [PooledGateway])]
    struct MessageModule;

    #[module(imports = [MessageModule], providers = [GlobalGuards<(ThrottlerGuard,)>])]
    struct MessageApp;

    #[tokio::test]
    async fn a_message_refuses_the_boot_naming_it() {
        let said = refusal::<MessageApp>().await;
        assert!(
            said.contains("/pooled/ws") && said.contains("silent"),
            "{said}"
        );
    }

    #[module(imports = [ThrottlerModule], providers = [PooledTool])]
    struct OperationModule;

    #[module(imports = [OperationModule], providers = [GlobalGuards<(ThrottlerGuard,)>])]
    struct OperationApp;

    #[tokio::test]
    async fn an_operation_refuses_the_boot_naming_it() {
        let said = refusal::<OperationApp>().await;
        assert!(said.contains("PooledTool::silent"), "{said}");
    }
}

// A guard nothing provides, injecting a trait object, whose dependency has no
// name the graph can render.
trait Nowhere: Send + Sync {}

#[injectable]
#[derive(Layer)]
struct UnprovidedGuard {
    #[inject]
    _dep: Arc<dyn Nowhere>,
}

#[async_trait]
impl Guard for UnprovidedGuard {
    async fn check_http(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }

    async fn check(&self, _cx: &mut nest_rs_core::UnitContext<'_>) -> Result<(), Denial> {
        Ok(())
    }
}

impl GuardFor<nest_rs_http::HttpView> for UnprovidedGuard {}

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
