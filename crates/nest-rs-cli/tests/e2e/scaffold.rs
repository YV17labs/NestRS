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

/// Run `nestrs <args…>` with cwd at `dir` and `env` on the process, asserting
/// success.
///
/// The environment is explicit because the CLI reads the project's env prefix
/// from its own: a generator writing variable names behaves differently in a
/// shell that names one and a shell that does not. Passing it here is what a
/// developer's `direnv`, devcontainer or `nestrs run` does.
fn nestrs(dir: &Path, args: &[&str], env: &[(&str, &str)]) {
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(args)
        .current_dir(dir)
        .envs(env.iter().copied())
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

/// `cargo clippy --workspace --all-targets -- -D warnings` over the generated
/// tree — **the gate the generated project sets for itself**.
///
/// It used to be a bare `cargo check`, and that gap shipped two defects: a
/// template importing a name no rendered body used, and a borrow the lint
/// rejects. Both compiled, so both reached a user on their first `just lint` and
/// nowhere earlier. A generator that emits code failing the lint it also emits
/// is a generator defect, so the e2e holds it to the same bar rather than a
/// lower one.
///
/// The target directory is shared with the repo's own so the framework's
/// artifacts are reused rather than rebuilt from scratch per run — the
/// difference between ~45 seconds and several minutes. It is a sibling of
/// `target/`, so it is already ignored by git.
fn cargo_check(workspace: &Path) -> Result<(), String> {
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--",
            "-D",
            "warnings",
        ])
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
    scaffold_write_and_check_in(&["new", "acme"], generate, &[], write, what, |_| {});
}

/// The same, with the `nestrs new` invocation, the environment every `nestrs`
/// runs under, and an inspection hook over the generated tree — for a flag whose
/// effect is spread across the Justfile, the Dockerfile and the `.env` cascade.
fn scaffold_write_and_check_in(
    new: &[&str],
    generate: &[&[&str]],
    env: &[(&str, &str)],
    write: &[(&str, &str)],
    what: &str,
    inspect: impl FnOnce(&Path),
) {
    let dir = tempfile::tempdir().expect("a temp dir");
    nestrs(dir.path(), new, env);
    let workspace = dir.path().join("acme");
    for args in generate {
        nestrs(&workspace, args, env);
    }
    for (path, body) in write {
        std::fs::write(workspace.join(path), body).expect("the generated tree is writable");
    }

    inspect(&workspace);

    patch_to_working_tree(&workspace);
    if let Err(stderr) = cargo_check(&workspace) {
        panic!("{what} does not compile:\n{stderr}");
    }
}

fn read(workspace: &Path, path: &str) -> String {
    std::fs::read_to_string(workspace.join(path))
        .unwrap_or_else(|e| panic!("the generated tree has {path}: {e}"))
}

#[test]
fn a_greenfield_workspace_compiles() {
    // The first thing anyone does with the CLI. If this breaks, `nestrs new`
    // hands a new user a repository that does not build.
    scaffold_write_and_check_in(
        &["new", "acme"],
        &[],
        &[],
        &[],
        "the scaffolded workspace",
        |workspace| {
            // A prefix placeholder is empty on the default, and `cargo check`
            // would never notice one left unrendered in a non-Rust file — the
            // Justfile is read by `just`, not by the compiler.
            let justfile = read(workspace, "Justfile");
            assert!(
                !justfile.contains("{{env_prefix"),
                "the Justfile carries an unrendered placeholder:\n{justfile}",
            );
        },
    );
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

#[test]
fn a_generated_entity_compiles() {
    // `g entity` emits an `#[expose]` entity that names **no** service, and the
    // absence is the part only a compiler can judge: `#[expose(service = …)]`
    // requires a `CrudService`, a plain `g feature` port's service is not one,
    // and naming it anyway fails inside the macro expansion — where the
    // `integration` suite, which reads the text back, sees nothing wrong.
    //
    // The port is that plain port on purpose: it is the case that would break
    // first, and the one `g resource` never exercises.
    scaffold_and_check(
        &[&["g", "feature", "blog"], &["g", "entity", "blog/article"]],
        "a generated entity",
    );
}

/// `crates/features/src/lib.rs` with the auth adapter's modules dropped.
///
/// The files stay on disk; Rust compiles the module tree, not the directory, so
/// undeclaring them is enough to take `identity/claims.rs` — and its `uuid` —
/// out of the build.
const RESOURCE_ONLY_LIB: &str = r#"//! Product features — vertical slices shared across apps.

pub mod hello;
pub mod post;

pub use hello::HelloHttpModule;
"#;

/// The same controller the generator writes, minus the guards — so its imports
/// are `std`, `nest_rs` and `crate::post`, and nothing else.
const UNGUARDED_CRUD_CONTROLLER: &str = r#"use std::sync::Arc;

use nest_rs::http::{controller, crud};

use crate::post::{CreatePost, Entity as PostEntity, Post, PostService, UpdatePost};

#[controller(path = "/post")]
pub struct PostController {
    #[inject]
    svc: Arc<PostService>,
}

#[crud(
    service = svc,
    entity = PostEntity,
    output = Post,
    create = CreatePost,
    update = UpdatePost,
)]
impl PostController {}
"#;

/// The resource's HTTP module without the `AuthzHttpModule` import the guards
/// brought — the second and last thread tying the generated resource to the
/// auth adapter.
const UNGUARDED_CRUD_MODULE: &str = r#"use nest_rs::core::module;

use super::controller::PostController;
use crate::post::PostModule;

#[module(
    imports = [PostModule],
    providers = [PostController],
)]
pub struct PostHttpModule;
"#;

#[test]
fn crud_needs_no_dependency_the_controller_does_not_name() {
    // The test the suite above only *looked* like it was running.
    //
    // `#[crud]` emitted `::uuid::Uuid` for three routes, so a crate that wrote
    // the attribute and nothing else failed with `E0433` naming a crate the
    // developer never wrote — the hard "no" that a macro expansion may not put
    // a line in a manifest. It shipped anyway, because the witness that should
    // have caught it cannot reach `#[crud]` and the one above passes for an
    // unrelated reason: `g resource` bootstraps `g auth`, whose claims type
    // names `uuid`, so the dependency is there whether the macro needs it or not.
    //
    // This case takes that accident away. The auth modules leave the module
    // tree, the controller drops the guards that were the only thing importing
    // them, and `uuid` leaves the manifest — leaving a crate whose entire claim
    // on `uuid` is whatever `#[crud]` emits. `resource_deps()` never listed it,
    // so the generator always agreed; it is the decorator that has to.
    scaffold_write_and_check_in(
        &["new", "acme"],
        &[&["g", "resource", "post"]],
        &[],
        &[
            ("crates/features/src/lib.rs", RESOURCE_ONLY_LIB),
            (
                "crates/features/src/post/http/controller.rs",
                UNGUARDED_CRUD_CONTROLLER,
            ),
            (
                "crates/features/src/post/http/module.rs",
                UNGUARDED_CRUD_MODULE,
            ),
        ],
        "a CRUD resource in a crate that does not declare `uuid`",
        |workspace| {
            let manifest = workspace.join("crates/features/Cargo.toml");
            let kept: String = read(workspace, "crates/features/Cargo.toml")
                .lines()
                .filter(|line| !line.trim_start().starts_with("uuid"))
                .map(|line| format!("{line}\n"))
                .collect();
            assert!(
                !kept.contains("uuid"),
                "the dependency under test is gone from the manifest:\n{kept}",
            );
            std::fs::write(&manifest, kept).expect("the generated manifest is writable");
        },
    );
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
fn a_custom_env_prefix_reaches_every_artifact_that_names_a_variable() {
    // `--env-prefix` is only real if two sides agree: the variable that *sets*
    // the prefix on every process this project starts, and the `.env` keys
    // those processes then read. A project where one of them still says NESTRS
    // boots with defaults and no error — which is the failure this asserts
    // against, and the reason `g auth` runs here: it appends a key to an
    // existing cascade, so it is the generator most able to disagree.
    scaffold_write_and_check_in(
        &["new", "acme", "--env-prefix", "ACME"],
        &[&["g", "auth"]],
        // What the developer's shell, devcontainer or `nestrs run` supplies —
        // and what `g auth` must build its key from.
        &[("NESTRS_ENV_PREFIX", "ACME")],
        &[],
        "a workspace scaffolded with a custom env prefix",
        |workspace| {
            // The prefix is set on the process, not declared in a crate. The
            // Justfile is where `nestrs run` picks it up, so a missing export
            // there means every recipe starts an app reading NESTRS_* against
            // an ACME_* cascade.
            assert!(
                read(workspace, "Justfile").contains(r#"export NESTRS_ENV_PREFIX := "ACME""#),
                "the Justfile must set the prefix for every process it starts",
            );

            for file in [".env", ".env.development", ".env.example"] {
                let body = read(workspace, file);
                assert!(
                    !body.contains("NESTRS_"),
                    "{file} still writes a NESTRS_ key the app will never read:\n{body}",
                );
            }
            // `.env` must not carry the prefix *variable* either: it is read
            // after the prefix has already chosen which cascade to read, so the
            // framework aborts on it rather than let the rename silently fail.
            assert!(
                !read(workspace, ".env").contains("ENV_PREFIX"),
                "the prefix cannot come from `.env` — the runtime aborts on it",
            );
            assert!(read(workspace, ".env").contains("ACME_DATABASE__URL="));
            assert!(read(workspace, ".env.development").contains("ACME_LOG="));
            assert!(
                read(workspace, ".env").contains("ACME_AUTHN__SECRET="),
                "`g auth` must append its dev secret under the project's own prefix",
            );
        },
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
