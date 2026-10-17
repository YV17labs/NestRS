//! Attribute-referenced layers (`#[use_guards/filters/interceptors]`) and
//! every edge host's dependencies, optional ones included, satisfy the access
//! contract: one whose module isn't imported fails the boot.

use std::sync::Arc;

use nest_rs_core::{AccessGraphError, App, Layer, injectable, module};
use nest_rs_graphql::{async_graphql, operations, resolver};
use nest_rs_guards::{Denial, Guard, GuardFor};
use nest_rs_http::{async_trait, controller, routes};
// The name rmcp's `#[tool_router]` / `#[tool_handler]` expansions resolve.
use nest_rs_mcp::rmcp;
use nest_rs_mcp::{ServerHandler, mcp, tool_handler, tool_router};
use nest_rs_ws::{WsModule, gateway, messages};

#[injectable]
#[derive(Default)]
struct AuthzGuard;

impl Layer for AuthzGuard {}

#[async_trait]
impl Guard for AuthzGuard {
    async fn check_http(
        &self,
        _cx: &mut nest_rs_core::UnitContext<'_>,
    ) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("forbidden"))
    }
}

impl GuardFor<nest_rs_http::HttpView> for AuthzGuard {}

#[module(providers = [AuthzGuard])]
struct GuardModule;

#[controller(path = "/loose")]
struct LooseController;

#[routes]
impl LooseController {
    #[get("/")]
    #[use_guards(AuthzGuard)]
    async fn loose_list(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [LooseController])]
struct LooseModule;

#[controller(path = "/loose-ctrl")]
#[use_guards(AuthzGuard)]
struct LooseCtrlController;

#[routes]
impl LooseCtrlController {
    #[get("/")]
    async fn loose_ctrl_list(&self) -> &'static str {
        "ok"
    }
}

#[module(providers = [LooseCtrlController])]
struct LooseCtrlModule;

#[controller(path = "/tight")]
struct TightController;

#[routes]
impl TightController {
    #[get("/")]
    #[use_guards(AuthzGuard)]
    async fn tight_list(&self) -> &'static str {
        "ok"
    }
}

#[module(imports = [GuardModule], providers = [TightController])]
struct TightModule;

/// `App` is not `Debug`, so `expect_err` is unavailable.
fn boot_error<M: nest_rs_core::Module + 'static>(scenario: &str) -> AccessGraphError {
    match App::new::<M>() {
        Ok(_) => panic!("{scenario}: expected the boot to fail with an access violation"),
        Err(err) => err
            .downcast::<AccessGraphError>()
            .expect("the failure is the named access-graph error, not a mount-time panic"),
    }
}

#[test]
fn a_per_route_guard_in_an_unimported_module_fails_the_boot() {
    let access = boot_error::<LooseModule>("per-route guard across an unimported boundary");
    assert_eq!(access.consumer, "LooseController");
    assert_eq!(access.dependency, "AuthzGuard");
    assert_eq!(access.owner, "GuardModule");
}

#[test]
fn a_controller_level_guard_in_an_unimported_module_fails_the_boot() {
    let access =
        boot_error::<LooseCtrlModule>("controller-level guard across an unimported boundary");
    assert_eq!(access.consumer, "LooseCtrlController");
    assert_eq!(access.dependency, "AuthzGuard");
}

#[test]
fn a_guard_whose_module_is_imported_boots_cleanly() {
    App::new::<TightModule>()
        .expect("a controller that imports the guard's module satisfies the contract");
}

#[injectable]
#[derive(Default)]
struct SharedThing;

#[module(providers = [SharedThing])]
struct FirstProviderModule;

#[module(providers = [SharedThing])]
struct SecondProviderModule;

#[module(imports = [FirstProviderModule, SecondProviderModule])]
struct DuplicateRoot;

#[injectable]
#[derive(Default)]
struct UnprovidedDep;

#[injectable]
struct EagerConsumer {
    #[inject]
    _dep: std::sync::Arc<UnprovidedDep>,
}

#[module(providers = [EagerConsumer])]
struct EagerMissingModule;

#[test]
fn an_eager_provider_with_a_missing_dependency_is_a_named_boot_error_not_a_panic() {
    match App::new::<EagerMissingModule>() {
        Ok(_) => panic!("an unprovided eager dependency must fail the boot"),
        Err(err) => {
            let missing = err
                .downcast::<nest_rs_core::MissingDependencyError>()
                .expect("the failure is the named missing-dependency error, not a panic");
            assert_eq!(missing.consumer, "EagerConsumer");
            assert!(
                missing.dependency.contains("UnprovidedDep"),
                "names the missing dep: {}",
                missing.dependency
            );
        }
    }
}

#[test]
fn two_modules_providing_the_same_concrete_type_fail_the_boot() {
    match App::new::<DuplicateRoot>() {
        Ok(_) => panic!("a duplicate concrete provider must fail the boot"),
        Err(err) => {
            let dup = err
                .downcast::<nest_rs_core::DuplicateProviderError>()
                .expect("the failure is the named duplicate-provider error");
            assert!(
                dup.type_name.contains("SharedThing"),
                "the error names the duplicated type: {}",
                dup.type_name
            );
        }
    }
}

#[injectable]
#[derive(Default)]
struct Directory;

#[module(providers = [Directory])]
struct DirectoryModule;

#[controller(path = "/maybe-directory")]
struct MaybeDirectoryController {
    #[inject]
    _directory: Option<Arc<Directory>>,
}

#[routes]
impl MaybeDirectoryController {
    #[get("/")]
    async fn maybe_directory(&self) -> &'static str {
        "ok"
    }
}

#[gateway(path = "/maybe-directory")]
struct MaybeDirectoryGateway {
    #[inject]
    _directory: Option<Arc<Directory>>,
}

#[messages]
impl MaybeDirectoryGateway {
    #[subscribe_message("ping")]
    #[public]
    async fn ping(&self) -> &'static str {
        "pong"
    }
}

#[resolver]
struct MaybeDirectoryResolver {
    #[inject]
    _directory: Option<Arc<Directory>>,
}

#[operations]
impl MaybeDirectoryResolver {
    #[query]
    #[public]
    async fn maybe_directory(&self) -> async_graphql::Result<String> {
        Ok("ok".into())
    }
}

#[mcp]
#[derive(Clone)]
struct MaybeDirectoryTools {
    #[inject]
    _directory: Option<Arc<Directory>>,
}

#[tool_router(allow_empty)]
impl MaybeDirectoryTools {}

#[tool_handler]
impl ServerHandler for MaybeDirectoryTools {}

#[module(providers = [MaybeDirectoryController])]
struct MaybeDirectoryHttpModule;

#[module(imports = [WsModule], providers = [MaybeDirectoryGateway])]
struct MaybeDirectoryWsModule;

#[module(providers = [MaybeDirectoryResolver])]
struct MaybeDirectoryGraphqlModule;

#[module(providers = [MaybeDirectoryTools])]
struct MaybeDirectoryMcpModule;

#[module(imports = [DirectoryModule, MaybeDirectoryHttpModule])]
struct MaybeDirectoryHttpRoot;

#[module(imports = [DirectoryModule, MaybeDirectoryWsModule])]
struct MaybeDirectoryWsRoot;

#[module(imports = [DirectoryModule, MaybeDirectoryGraphqlModule])]
struct MaybeDirectoryGraphqlRoot;

#[module(imports = [DirectoryModule, MaybeDirectoryMcpModule])]
struct MaybeDirectoryMcpRoot;

/// The access violation booting `M` fails with, on the path that drains the
/// factories a `WsModule` import queues.
async fn built_boot_error<M: nest_rs_core::Module>(scenario: &str) -> AccessGraphError {
    match App::builder().module::<M>().build().await {
        Ok(_) => panic!("{scenario}: expected the boot to fail with an access violation"),
        Err(err) => err
            .downcast::<AccessGraphError>()
            .unwrap_or_else(|err| panic!("{scenario}: not the access violation: {err:#}")),
    }
}

#[tokio::test]
async fn an_edge_hosts_optional_dependency_another_reached_module_provides_needs_its_import() {
    for (host, error) in [
        (
            "MaybeDirectoryController",
            built_boot_error::<MaybeDirectoryHttpRoot>("a controller's optional dependency").await,
        ),
        (
            "MaybeDirectoryGateway",
            built_boot_error::<MaybeDirectoryWsRoot>("a gateway's optional dependency").await,
        ),
        (
            "MaybeDirectoryResolver",
            built_boot_error::<MaybeDirectoryGraphqlRoot>("a resolver's optional dependency").await,
        ),
        (
            "MaybeDirectoryTools",
            built_boot_error::<MaybeDirectoryMcpRoot>("an MCP host's optional dependency").await,
        ),
    ] {
        assert_eq!(error.consumer, host);
        assert_eq!(error.owner, "DirectoryModule", "{host}");
    }
}
