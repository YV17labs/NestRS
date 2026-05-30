//! The boot-time access contract now governs **attribute-referenced layers**, not
//! just `#[inject]` fields: a `#[use_guards]` / `#[use_filters]` /
//! `#[use_interceptors]` reference is resolved from the container at mount, so a
//! layer registered in a module the controller does *not* import must fail the
//! boot with the named `AccessGraphError` — never be resolved silently through the
//! flat container (an encapsulation breach) or panic at mount. Both scopes
//! (controller-level on the struct, per-route beside the verb) fold into the
//! controller's `Discoverable::injected`, so the existing graph check covers them.

use nestrs_core::{injectable, module, AccessGraphError, App};
use nestrs_http::{async_trait, controller, routes, Guard};
use poem::http::StatusCode;
use poem::{Request, Response};

/// A trivial guard, registered only by `GuardModule`. A controller that binds it
/// without importing that module is the breach the contract must catch.
#[injectable]
#[derive(Default)]
struct AuthzGuard;

#[async_trait]
impl Guard for AuthzGuard {
    async fn check(&self, _req: &mut Request) -> std::result::Result<(), Response> {
        Err(Response::builder().status(StatusCode::FORBIDDEN).finish())
    }
}

#[module(providers = [AuthzGuard])]
struct GuardModule;

// --- The breach: a per-route guard whose module is not imported. ---

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

// --- The breach via a controller-level guard (on the struct). ---

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

// --- Correctly wired: the controller's module imports the guard's module. ---

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

/// `App::new` does not implement `Debug`, so `expect_err` is unavailable — pull
/// the error out by hand, asserting boot failed rather than succeeded.
fn boot_error<M: nestrs_core::Module + 'static>(scenario: &str) -> AccessGraphError {
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
