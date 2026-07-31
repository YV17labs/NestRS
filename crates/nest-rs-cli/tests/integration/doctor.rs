//! `nestrs doctor` — the toolchain report.

use std::process::Command;

#[test]
fn doctor_passes_with_rust_toolchain() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("doctor")
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}
