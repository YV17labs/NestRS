//! The machinery every generator shares: `--dry-run` writes nothing, and says so
//! without claiming the work happened.

use crate::harness::{run_ok, write_fake_workspace};
use std::fs;

#[test]
fn generate_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &[
            "g",
            "feature",
            "posts",
            "--dry-run",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(!dir.path().join("crates/features/src/posts").exists());
    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(!lib.contains("pub mod posts;"));
}

/// B10: `--dry-run` printed "Created feature `orders`" directly above "no files
/// written" — the two lines contradicted each other.
#[test]
fn generate_dry_run_does_not_claim_the_work_happened() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let stdout = run_ok(
        dir.path(),
        &[
            "g",
            "feature",
            "orders",
            "--dry-run",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    assert!(stdout.contains("Dry run — no files written."), "{stdout}");
    assert!(
        !stdout.contains("Wrote feature"),
        "a dry run must not report a completed action: {stdout}",
    );
    assert!(stdout.contains("Would write feature `orders`"), "{stdout}");
}
