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
    fs::write(root.join(".env"), "NESTRS_SEAORM__URL=postgres://x\n").unwrap();
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
/// loads under `NESTRS_ENV=test`.
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

/// The one value of `var` that `file` assigns, read the way the cascade reads it.
pub(crate) fn assigned(file: &Path, var: &str) -> Option<String> {
    let body = fs::read_to_string(file).ok()?;
    let mut values = body
        .lines()
        .filter_map(|line| line.strip_prefix(var)?.strip_prefix('='))
        .map(str::to_owned);
    let value = values.next();
    assert!(
        values.next().is_none(),
        "{} assigns {var} twice",
        file.display()
    );
    value
}

/// RFC 7518 §3.2: an HS256 key carries at least the hash's 256 bits.
pub(crate) fn assert_256_bit_hex(secret: &str) {
    assert!(
        secret.len() == 64
            && secret
                .bytes()
                .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')),
        "a secret is 64 lowercase hex characters: {secret:?}",
    );
}

/// The bootstrap variable that renames every other, as the CLI spells it.
pub(crate) use nest_rs_cli::ENV_PREFIX_VAR;

/// The framework variable name the *generated* project will carry — the CLI's
/// own derivation, read from its library rather than mirrored here.
pub(crate) use nest_rs_cli::scaffolded_var;
