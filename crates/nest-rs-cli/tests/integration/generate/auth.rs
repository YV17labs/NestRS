//! `nestrs g auth` — the one auth adapter a workspace gets, and its three roots
//! at the composition site.

use crate::harness::{run_ok, write_fake_app, write_fake_workspace};
use std::fs;
use std::process::Command;

#[test]
fn generate_auth_scaffolds_the_adapter_once() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &["g", "auth", "-p", dir.path().to_str().unwrap()],
    );

    let src = dir.path().join("crates/features/src");
    let ability = fs::read_to_string(src.join("app_authz/ability.rs")).unwrap();
    assert!(ability.contains("impl AbilityFactory for AppAbility"));
    // Both branches are scaffolded empty: the authenticated policy and the
    // visitor one a `#[public]` route reads. Leaving `define_visitor` out would
    // hide the only place an anonymous read can be granted.
    assert!(
        ability.contains("fn define(") && ability.contains("fn define_visitor("),
        "the scaffolded policy carries both branches: {ability}"
    );
    let guard = fs::read_to_string(src.join("app_authz/http/guard.rs")).unwrap();
    assert!(guard.contains("AbilityGuard<AppAbility>"));

    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    assert!(lib.contains("pub mod app_authn;"));
    assert!(lib.contains("pub mod app_authz;"));
    assert!(lib.contains("pub use app_authn::{Claims, Role};"));

    // Every guarded route the adapter arms needs a bearer token, and until the
    // app writes its real login nothing mints one. The route that fills the gap
    // is `#[public]` by necessity — a caller with no token is exactly who asks
    // for one — so what keeps it out of production is the boot refusal, not the
    // posture. Assert both halves: without the refusal this is an open token
    // minter, and without the route the tutorial is back to hand-signing HS256.
    let controller = fs::read_to_string(src.join("app_authn/http/controller.rs")).unwrap();
    assert!(
        controller.contains("#[post(\"/dev-token\")]") && controller.contains("#[public]"),
        "the development token route is what a scaffolded app calls its guarded routes with: \
         {controller}"
    );
    // The refusal lives on its own provider, not on the controller: a
    // `#[controller]` registers metadata, never an instance, so a `#[hooks]`
    // block on it could only be skipped at boot — a composition the framework
    // now refuses at compile time (`hooks_on_a_controller` trybuild snapshot).
    let audit = fs::read_to_string(src.join("app_authn/http/audit.rs")).unwrap();
    assert!(
        audit.contains("#[on_module_init]") && audit.contains("if is_development()"),
        "the module refuses the boot when this is not a development run: {audit}"
    );
    assert!(
        !controller.contains("#[hooks]"),
        "the refusal must not sit on the controller, where the hook is skipped: {controller}"
    );

    // **Absence must answer `false`.** `<PREFIX>_ENV` is unset in a fresh
    // scaffold, in every container the CLI writes and in most CI, so a predicate
    // written as "refuse when it says production" leaves the minter serving
    // wherever nobody set the variable — including a misspelled `producton`.
    // `Environment::from_env` maps both to `Development`, which is right for
    // picking a `.env` cascade and wrong for arming a security affordance;
    // `Environment::declared` exists for this question and answers `None` for
    // both, so the scaffold asks it instead of re-deriving the classification.
    assert!(
        audit.contains("Environment::declared()")
            && !audit.contains("Environment::from_env()")
            && !audit.contains(r#"Ok("development")"#),
        "the predicate asks the framework's classifier positively, so an unset or hand-parsed \
         variable is not a development run: {audit}"
    );
    // And the route carries a guard asking the same question, so registering the
    // controller from another module — which the audit cannot see — buys nothing.
    // A guard rather than an `if` in the body: an access decision belongs at a
    // greppable `#[use_guards]` site, renders through the same denial path as
    // every other refusal, and logs. A body `if` does none of the three.
    let guard = fs::read_to_string(src.join("app_authn/http/guard.rs")).unwrap();
    assert!(
        guard.contains("impl Guard for DevOnlyGuard") && guard.contains("if is_development()"),
        "the route's own refusal is a guard: {guard}"
    );
    assert!(
        controller.contains("#[use_guards(DevOnlyGuard)]"),
        "the controller binds it, so the refusal travels with the route: {controller}"
    );

    // A workspace has exactly one auth adapter; a second run must not clobber
    // the policy the developer has been editing.
    let second = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "auth", "-p", dir.path().to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!second.status.success());
}

// Every auth root has to reach the composition site from one run. They are
// imports into the same `module.rs`, and a scaffold transaction resolves every
// edit against the file *on disk* — so queuing them as separate edits would
// silently drop all but the last, leaving an app that lists part of what it
// serves. `AppAuthnHttpModule` is the one that would go unnoticed: the app still
// boots without it, and the only symptom is a `POST /auth/dev-token` answering
// 404 to a reader following the tutorial.
#[test]
fn generate_auth_wires_every_root_into_the_app() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");

    run_ok(&app, &["g", "auth"]);

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    for (use_path, ident) in [
        ("features::app_authn::AppAuthnModule", "AppAuthnModule"),
        (
            "features::app_authn::AppAuthnHttpModule",
            "AppAuthnHttpModule",
        ),
        (
            "features::app_authz::AppAuthzHttpModule",
            "AppAuthzHttpModule",
        ),
    ] {
        assert!(module.contains(&format!("use {use_path};")), "{module}");
        assert!(
            module.contains(&format!("{ident},")),
            "the imports array must list {ident}: {module}"
        );
    }
    // The pre-existing import is untouched.
    assert!(module.contains("HttpModule::for_root"), "{module}");
}
