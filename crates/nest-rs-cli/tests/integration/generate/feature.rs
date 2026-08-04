//! `nestrs g feature` — the port with no transport.

use crate::harness::{run_ok, write_fake_workspace};
use std::fs;

#[test]
fn generate_feature_creates_port_and_wires_lib() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let stdout = run_ok(
        dir.path(),
        &["g", "feature", "posts", "-p", dir.path().to_str().unwrap()],
    );
    assert_reference_resolves_where_the_reader_is(&stdout);

    let feature = dir.path().join("crates/features/src/posts");
    assert!(feature.join("mod.rs").is_file());
    assert!(feature.join("module.rs").is_file());
    assert!(feature.join("service.rs").is_file());
    // no transport yet
    assert!(!feature.join("http").exists());

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod posts;"));
}

/// R12 C-1: the printed next steps sent the reader to
/// `crates/features/src/users/` — a path relative to *their* workspace, where it
/// does not exist. It names the `users/` exemplar in the framework's `demo/`, so
/// it has to be the URL the scaffolded README already cites, not a bare path a
/// reader will `ls` and not find.
fn assert_reference_resolves_where_the_reader_is(stdout: &str) {
    let line = stdout
        .lines()
        .find(|l| l.trim_start().starts_with("Reference:"))
        .unwrap_or_else(|| panic!("`g feature` prints a Reference line: {stdout}"));
    assert!(
        line.contains("https://"),
        "the exemplar lives in the framework repo, so the reference is a URL: {line}"
    );
    assert!(
        line.contains("demo/crates/features/src/users"),
        "…and it points at the `users/` exemplar: {line}"
    );
}
