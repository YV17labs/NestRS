//! `nestrs g feature` — the port with no transport.

use crate::harness::{run_ok, write_fake_workspace};
use std::fs;

#[test]
fn generate_feature_creates_port_and_wires_lib() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &["g", "feature", "posts", "-p", dir.path().to_str().unwrap()],
    );

    let feature = dir.path().join("crates/features/src/posts");
    assert!(feature.join("mod.rs").is_file());
    assert!(feature.join("module.rs").is_file());
    assert!(feature.join("service.rs").is_file());
    // no transport yet
    assert!(!feature.join("http").exists());

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod posts;"));
}
