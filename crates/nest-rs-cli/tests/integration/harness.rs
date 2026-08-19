//! Shared fixtures: a fake workspace on disk, a fake app crate inside it, and
//! the `nestrs` invocation every test drives the binary through.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) fn write_fake_workspace(root: &Path) {
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
    fs::write(
        root.join("crates/features/Cargo.toml"),
        "[package]\nname = \"features\"\n\n[dependencies]\nnest-rs-core.workspace = true\n",
    )
    .unwrap();
    fs::write(root.join(".env"), "NESTRS_DATABASE__URL=postgres://x\n").unwrap();
}

/// An app crate inside the fake workspace, carrying the
/// `#[module(imports = [ … ])]` shape the auto-wiring anchors on.
pub(crate) fn write_fake_app(root: &Path, name: &str) -> PathBuf {
    let app = root.join("apps").join(name);
    fs::create_dir_all(app.join("src")).unwrap();
    fs::write(
        app.join("Cargo.toml"),
        format!("[package]\nname = \"{name}\"\n\n[dependencies]\nnest-rs-core.workspace = true\n"),
    )
    .unwrap();
    fs::write(
        app.join("src/module.rs"),
        "use nest_rs_core::module;\nuse nest_rs_http::{HttpConfig, HttpModule};\n\n\
         #[module(\n    imports = [\n        \
         HttpModule::for_root(HttpConfig { port: 3000, ..Default::default() }),\n    ],\n)]\n\
         pub struct AppModule;\n",
    )
    .unwrap();
    app
}

/// Run `nestrs <args...>` with cwd at `dir`, asserting success.
pub(crate) fn run_ok(dir: &Path, args: &[&str]) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "args {args:?} stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// The scaffolded `.env.example` sends a test override to a file the cascade
/// actually loads under `NESTRS_ENV=test` (see `templates::shared::ENV_EXAMPLE`
/// for why the wrong answer fails silently).
pub(crate) fn assert_env_example_points_test_overrides_somewhere_loaded(root: &Path) {
    let example = fs::read_to_string(root.join(".env.example")).unwrap();
    assert!(
        example.contains(".env.test.local"),
        "`.env.example` must name the file a test override actually loads from: {example}"
    );
    assert!(
        example.contains("skips `.env.local`"),
        "…and say why `.env.local` is not it: {example}"
    );
}

pub(crate) fn write_fake_migrations_crate(root: &Path) {
    let dir = root.join("crates/migrations/src");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("lib.rs"),
        "mod m20260101_000000_init;\nmod migrator;\n\npub use migrator::Migrator;\n",
    )
    .unwrap();
    fs::write(
        dir.join("migrator.rs"),
        "use sea_orm_migration::prelude::*;\n\nuse super::{\n    m20260101_000000_init,\n};\n\npub struct Migrator;\n",
    )
    .unwrap();
    fs::write(dir.join("m20260101_000000_init.rs"), "// init\n").unwrap();
}

/// The framework variable name the *generated* project will carry, built the
/// way the CLI builds it.
///
/// **The CLI is a binary crate**, so its suite cannot call
/// `context::var_name` / `context::env_prefix` the way every other crate here
/// calls the thing it tests — there is no `[lib]` to import. So the mirror is
/// mirrored once more, here, rather than at each assertion: a literal
/// `"NESTRS_AUTHN__SECRET"` in a test asserts the *default* prefix and fails
/// the moment the suite runs under `NESTRS_ENV_PREFIX=ACME`, which is the run
/// that proves a rename reaches everything. Third copy of one join, and it is
/// reported as such — `context::ENV_PREFIX_VAR` is spelled here because it is
/// the one name no prefix renames, which `CLAUDE.md` sanctions per crate.
pub fn scaffolded_var(namespace: &str, key: &str) -> String {
    let prefix = std::env::var("NESTRS_ENV_PREFIX")
        .ok()
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "NESTRS".to_owned());
    format!("{prefix}_{}__{key}", namespace.to_uppercase())
}
