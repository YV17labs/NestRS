//! `nestrs g auth` — the one auth adapter a workspace gets, and its two roots at
//! the composition site.

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
    assert!(ability.contains("impl AbilityFactory for AppAbility"));
    // Both branches are scaffolded empty: the authenticated policy and the
    // visitor one a `#[public]` route reads. Leaving `define_visitor` out would
    // hide the only place an anonymous read can be granted.
    assert!(
        ability.contains("fn define(") && ability.contains("fn define_visitor("),
        "the scaffolded policy carries both branches: {ability}"
    );
    let guard = fs::read_to_string(src.join("authz/http/guard.rs")).unwrap();
    assert!(guard.contains("AbilityGuard<AppAbility>"));

    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    assert!(lib.contains("pub mod authn;"));
    assert!(lib.contains("pub mod authz;"));
    assert!(lib.contains("pub use identity::{Claims, Role};"));

    // A workspace has exactly one auth adapter; a second run must not clobber
    // the policy the developer has been editing.
    let second = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "auth", "-p", dir.path().to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!second.status.success());
}

// Both auth roots have to reach the composition site from one run. They are two
// imports into the same `module.rs`, and a scaffold transaction resolves every
// edit against the file *on disk* — so queuing them as two edits would silently
// drop the first, leaving an app that lists only half of what it serves.
#[test]
fn generate_auth_wires_both_roots_into_the_app() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");

    run_ok(&app, &["g", "auth"]);

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    for (use_path, ident) in [
        ("features::authn::AuthnModule", "AuthnModule"),
        ("features::authz::AuthzHttpModule", "AuthzHttpModule"),
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
