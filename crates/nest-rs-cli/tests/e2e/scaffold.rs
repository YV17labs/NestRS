//! Covers `src/templates/` and `src/commands/generate/` — by compiling what
//! they wrote.

use std::path::{Path, PathBuf};
use std::process::Command;

/// The repository root, from this crate's manifest directory.
fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repo root resolves")
}

/// Run `nestrs <args…>` with cwd at `dir`, asserting success.
fn nestrs(dir: &Path, args: &[&str]) {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("the nestrs binary runs");
    assert!(
        output.status.success(),
        "`nestrs {}` failed:\n{}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr),
    );
}

/// Point the generated `nest-rs` requirement at this working tree.
///
/// The scaffold pins the *published* umbrella — which is the right thing for a
/// user and the wrong thing for this test twice over: the version under
/// development is not on crates.io yet, and even once it is, checking against
/// the registry would prove that yesterday's release compiles rather than
/// today's templates. Patching the umbrella alone is enough: its own siblings
/// are declared `{ workspace = true }` and resolve through the repo's paths.
fn patch_to_working_tree(workspace: &Path) {
    let manifest = workspace.join("Cargo.toml");
    let mut raw = std::fs::read_to_string(&manifest).expect("the generated manifest is readable");
    raw.push_str(&format!(
        "\n[patch.crates-io]\nnest-rs = {{ path = \"{}\" }}\n",
        repo().join("crates/nest-rs").display(),
    ));
    std::fs::write(&manifest, raw).expect("the manifest is writable");
}

/// `cargo check --workspace` over the generated tree.
///
/// The target directory is shared with the repo's own so the framework's
/// artifacts are reused rather than rebuilt from scratch per run — the
/// difference between ~45 seconds and several minutes. It is a sibling of
/// `target/`, so it is already ignored by git.
fn cargo_check(workspace: &Path) -> Result<(), String> {
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args(["check", "--workspace"])
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", repo().join("target/scaffold-check"))
        .output()
        .map_err(|err| format!("cargo did not run: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).into_owned())
}

/// Scaffold `acme`, run `generate` inside it, and compile the result.
///
/// The fragile part is the arrange — the `[patch.crates-io]` repoint and the
/// shared target directory — so it lives here once: a fix to it must not have to
/// be made per test.
fn scaffold_and_check(generate: &[&[&str]], what: &str) {
    scaffold_write_and_check(generate, &[], what);
}

/// The same, with source files written into the generated tree before the
/// check — for the half of the contract the generators do not emit: what the
/// docs tell the reader to *write* into a scaffolded feature.
fn scaffold_write_and_check(generate: &[&[&str]], write: &[(&str, &str)], what: &str) {
    let dir = tempfile::tempdir().expect("a temp dir");
    nestrs(dir.path(), &["new", "acme"]);
    let workspace = dir.path().join("acme");
    for args in generate {
        nestrs(&workspace, args);
    }
    for (path, body) in write {
        std::fs::write(workspace.join(path), body).expect("the generated tree is writable");
    }

    patch_to_working_tree(&workspace);
    if let Err(stderr) = cargo_check(&workspace) {
        panic!("{what} does not compile:\n{stderr}");
    }
}

#[test]
fn a_greenfield_workspace_compiles() {
    // The first thing anyone does with the CLI. If this breaks, `nestrs new`
    // hands a new user a repository that does not build.
    scaffold_and_check(&[], "the scaffolded workspace");
}

#[test]
fn a_generated_crud_resource_compiles() {
    // This is the claim `.claude/rules/framework.md` makes and this suite
    // exists to honour: `#[crud]` and `#[expose]` are deliberately absent from
    // `nest-rs-macro-hygiene` because they need a real entity and a real
    // service, so their contract is proved *here* — on generated code, with the
    // derives the decorators emit and the auth adapter the guards require.
    scaffold_and_check(&[&["g", "resource", "post"]], "a generated CRUD resource");
}

/// The `/fundamentals/lifecycle/` snippet, verbatim but for the feature name —
/// the first thing a reader pastes into a freshly generated port. It reaches for
/// both crates the docs never tell anyone to add: the `tracing` façade for an
/// application log, and `anyhow` for the fallible hook's return type.
///
/// A transcription, so it can drift from the page. It is the narrowest form of
/// the general answer — compiling the docs' ~450 rust fences — which is an owner
/// call, not something to half-build here.
const LIFECYCLE_PAGE_SERVICE: &str = r#"use nest_rs::core::{hooks, injectable};

#[injectable]
#[derive(Default)]
pub struct BlogService;

#[hooks]
impl BlogService {
    #[on_application_bootstrap]
    async fn warm(&self) -> anyhow::Result<()> {
        tracing::info!(target: "features::blog", entries = 0, "cache warmed");
        Ok(())
    }

    #[on_application_shutdown]
    async fn flush(&self) {
        tracing::info!(target: "features::blog", pending = 0, "buffers flushed");
    }
}
"#;

#[test]
fn a_feature_can_log_and_return_a_fallible_hook() {
    // R12 L-1: the scaffolded features crate declared neither `tracing` nor
    // `anyhow`, so this paste failed with `E0433: cannot find module or crate
    // tracing` — then, once that was added by hand, with the same error on
    // `anyhow`. Both are the developer's own source naming its own crate, so the
    // scaffold declares them; nothing else in the generated tree uses them, and
    // a manifest entry no test exercises is one a later cleanup deletes.
    scaffold_write_and_check(
        &[&["g", "feature", "blog"]],
        &[(
            "crates/features/src/blog/service.rs",
            LIFECYCLE_PAGE_SERVICE,
        )],
        "a feature service that logs and returns a fallible hook",
    );
}

#[test]
fn the_generated_ws_and_mcp_authz_bridges_compile() {
    // F4: `g ws` and `g mcp` named `AuthzWsModule` / `features::authz::mcp` in
    // their own output while writing neither. They write both now — and a
    // bridge module is exactly the shape the `integration` suite cannot judge:
    // it asserts on the *text* a generator produced, so a `#[module]` naming a
    // provider behind a feature the manifest never enabled reads as correct
    // there and fails on the user's first `cargo check`.
    //
    // One workspace, both adapters: the `authz/` tree is shared, so this also
    // pins that two bridges land side by side without clobbering each other's
    // index lines.
    scaffold_and_check(
        &[
            &["g", "resource", "post"],
            &["g", "ws", "post"],
            &["g", "mcp", "post"],
        ],
        "the generated WS and MCP authz bridges",
    );
}
