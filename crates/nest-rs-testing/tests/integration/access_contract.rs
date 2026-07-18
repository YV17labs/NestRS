//! Attribute-referenced layers (`#[use_guards/filters/interceptors]`) must
//! satisfy the access contract: binding a layer whose module isn't imported
//! fails the boot with `AccessGraphError`, never silently resolves or panics
//! at mount.

use nest_rs_core::{AccessGraphError, App, Layer, injectable, module};
use nest_rs_guards::{Denial, Guard};
use nest_rs_http::{async_trait, controller, routes};
use poem::Request;

#[injectable]
#[derive(Default)]
struct AuthzGuard;

impl Layer for AuthzGuard {}

#[async_trait]
impl Guard for AuthzGuard {
    async fn check_http(&self, _req: &mut Request) -> std::result::Result<(), Denial> {
        Err(Denial::forbidden("forbidden"))
    }
}

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

// `EagerConsumer` injects `UnprovidedDep`, which no module provides.
#[module(providers = [EagerConsumer])]
struct EagerMissingModule;

#[test]
fn an_eager_provider_with_a_missing_dependency_is_a_named_boot_error_not_a_panic() {
    // The register phase used to panic on this before the access-graph check
    // could run; it now defers to the graph, so the failure is the same named
    // boot error as every other wiring mistake.
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
    // A concrete type registered by two modules used to silently last-write-wins;
    // it now fails the boot, uniform with every other wiring error.
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
