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
    let ability = fs::read_to_string(src.join("authz/ability.rs")).unwrap();
    assert!(ability.contains("impl AbilityFactory for AuthzAbility"));
    // Both branches are scaffolded empty: `define_visitor` is the only place an
    // anonymous read can be granted.
    assert!(
        ability.contains("fn define(") && ability.contains("fn define_visitor("),
        "the scaffolded policy carries both branches: {ability}"
    );
    let guard = fs::read_to_string(src.join("authz/guard.rs")).unwrap();
    assert!(guard.contains("AbilityGuard<AuthzAbility>"));

    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    assert!(lib.contains("pub mod authn;"));
    assert!(lib.contains("pub mod authz;"));
    assert!(lib.contains("pub use authn::{Claims, Role};"));

    // The dev token route is `#[public]` by necessity; the boot refusal keeps it
    // out of production.
    let controller = fs::read_to_string(src.join("authn/http/controller.rs")).unwrap();
    assert!(
        controller.contains("#[post(\"/dev-token\")]") && controller.contains("#[public]"),
        "the development token route is what a scaffolded app calls its guarded routes with: \
         {controller}"
    );
    // On its own provider: a `#[controller]` registers no instance to run hooks on.
    let audit = fs::read_to_string(src.join("authn/http/audit.rs")).unwrap();
    assert!(
        audit.contains("#[on_module_init]") && audit.contains("if is_development()"),
        "the module refuses the boot when this is not a development run: {audit}"
    );
    assert!(
        !controller.contains("#[hooks]"),
        "the refusal must not sit on the controller, where the hook is skipped: {controller}"
    );

    // **Absence must answer `false`**: `<PREFIX>_ENV` is unset in most scaffolds
    // and CI, so the scaffold asks `Environment::declared`, which answers `None`
    // for unset and misspelled alike.
    assert!(
        audit.contains("Environment::declared()")
            && !audit.contains("Environment::from_env()")
            && !audit.contains(r#"Ok("development")"#),
        "the predicate asks the framework's classifier positively, so an unset or hand-parsed \
         variable is not a development run: {audit}"
    );
    // A guard asking the same question covers registration from another module.
    let guard = fs::read_to_string(src.join("authn/http/guard.rs")).unwrap();
    assert!(
        guard.contains("impl Guard for DevOnlyGuard") && guard.contains("if is_development()"),
        "the route's own refusal is a guard: {guard}"
    );
    assert!(
        controller.contains("#[use_guards(DevOnlyGuard)]"),
        "the controller binds it, so the refusal travels with the route: {controller}"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "auth", "-p", dir.path().to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!second.status.success());
}

// Every auth root reaches the composition site from one run, in one edit:
// separate edits would drop all but the last.
#[test]
fn generate_auth_wires_every_root_into_the_app() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");

    run_ok(&app, &["g", "auth"]);

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    for (use_path, ident) in [
        ("features::authn::AuthnModule", "AuthnModule"),
        ("features::authn::AuthnHttpModule", "AuthnHttpModule"),
        ("features::authz::AuthzModule", "AuthzModule"),
    ] {
        assert!(module.contains(&format!("use {use_path};")), "{module}");
        assert!(
            module.contains(&format!("{ident},")),
            "the imports array must list {ident}: {module}"
        );
    }
    assert!(module.contains("HttpModule::for_root"), "{module}");
}
