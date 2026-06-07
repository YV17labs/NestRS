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
