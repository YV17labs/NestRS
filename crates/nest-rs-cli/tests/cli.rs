use std::fs;
use std::path::Path;
use std::process::Command;

fn write_fake_workspace(root: &Path) {
    fs::create_dir_all(root.join("crates/features/src")).unwrap();
    fs::create_dir_all(root.join("apps")).unwrap();
    fs::write(
        root.join("Cargo.toml"),
        r#"
[workspace]
members = ["crates/*", "apps/*"]

[workspace.package]
version = "0.1.0"
"#,
    )
    .unwrap();
    fs::write(
        root.join("crates/features/src/lib.rs"),
        "pub mod users;\n\npub use users::UsersModule;\n",
    )
    .unwrap();
}

#[test]
fn new_standalone_hello_template() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "demo-api", "--template", "hello"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = dir.path().join("demo-api");
    assert!(app.join("src/main.rs").is_file());
    assert!(app.join("src/controller.rs").is_file());
    assert!(app.join("Cargo.toml").is_file());
}

#[test]
fn new_workspace_empty_template() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args([
            "new",
            "tutorial-api",
            "--in-workspace",
            "--template",
            "empty",
            "-o",
        ])
        .arg(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = dir.path().join("apps/tutorial-api");
    assert!(app.join("src/module.rs").is_file());
    assert!(!app.join("src/controller.rs").exists());
}

#[test]
fn generate_feature_with_http() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "feature", "posts", "--http", "-p"])
        .arg(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let feature = dir.path().join("crates/features/src/posts");
    assert!(feature.join("http/controller.rs").is_file());

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod posts;"));
}

#[test]
fn version_prints_single_line() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("version")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout.trim();
    assert!(line.starts_with("NestRS "));
    assert!(!line.contains('\n'));
}

#[test]
fn about_prints_metadata_block() {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .arg("about")
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Version:"));
    assert!(stdout.contains("Tagline:"));
    assert!(stdout.contains("Yoann Vanitou"));
}

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
