//! Covers `src/app_identity.rs` — the binary's name, version and description
//! `#[nest_rs::main]` reads off the manifest of the crate it expands in.

use nest_rs_core::AppIdentity;

#[nest_rs_core::main]
async fn the_identity_main_captured() -> Option<AppIdentity> {
    AppIdentity::current().cloned()
}

#[test]
fn main_captures_the_manifest_of_the_crate_it_expanded_in_before_its_body() {
    assert_eq!(AppIdentity::current(), None, "before any `main`, none");

    let captured = the_identity_main_captured().expect("captured before the body ran");
    assert_eq!(captured.name, env!("CARGO_PKG_NAME"));
    assert_eq!(captured.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        captured.description.as_deref(),
        Some(env!("CARGO_PKG_DESCRIPTION")),
    );
    assert_eq!(AppIdentity::current(), Some(&captured), "and kept after it");
}
