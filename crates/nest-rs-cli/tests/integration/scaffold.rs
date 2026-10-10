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
/// success, and hand back what it printed.
///
/// The environment is explicit: the CLI reads the project's env prefix from its
/// own.
fn nestrs(dir: &Path, args: &[&str], env: &[(&str, &str)]) -> String {
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
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Point the generated `nest-rs` requirement at this working tree.
///
/// The version under development is not on crates.io, and the registry would
/// prove yesterday's release. The umbrella alone is enough: its siblings resolve
/// through the repo's paths.
fn patch_to_working_tree(workspace: &Path) {
    let manifest = workspace.join("Cargo.toml");
    let mut raw = std::fs::read_to_string(&manifest).expect("the generated manifest is readable");
    raw.push_str(&format!(
        "\n[patch.crates-io]\nnest-rs = {{ path = \"{}\" }}\n",
        repo().join("crates/nest-rs").display(),
    ));
    std::fs::write(&manifest, raw).expect("the manifest is writable");
}

/// `cargo clippy --workspace --all-targets --all-features -- -D warnings` over the generated
/// tree — **the gate the generated project sets for itself**.
///
/// Every scaffold builds into one shared target directory; nextest runs each
/// test in its own process, so a file lock takes them one at a time.
#[expect(
    clippy::disallowed_methods,
    reason = "CARGO is the cargo that runs the suite, set by cargo itself"
)]
fn cargo_check(workspace: &Path) -> Result<(), String> {
    let target = repo().join("target/scaffold-check");
    std::fs::create_dir_all(&target).map_err(|err| format!("no target dir: {err}"))?;
    let lock = std::fs::File::create(target.join("scaffold.lock"))
        .map_err(|err| format!("no lock file: {err}"))?;
    lock.lock()
        .map_err(|err| format!("the lock is not taken: {err}"))?;
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".into()))
        .args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--all-features",
            "--",
            "-D",
            "warnings",
        ])
        .current_dir(workspace)
        .env("CARGO_TARGET_DIR", &target)
        .output()
        .map_err(|err| format!("cargo did not run: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(String::from_utf8_lossy(&output.stderr).into_owned())
}

/// Scaffold `acme`, run `generate` inside it, and compile the result.
fn scaffold_and_check(generate: &[&[&str]], what: &str) {
    scaffold_write_and_check(generate, &[], what);
}

/// The same, with source files written into the generated tree before the
/// check — for the half of the contract the generators do not emit: what the
/// docs tell the reader to *write* into a scaffolded feature.
fn scaffold_write_and_check(generate: &[&[&str]], write: &[(&str, &str)], what: &str) {
    scaffold_write_and_check_in(&["new", "acme"], generate, &[], write, what, |_, _| {});
}

/// The same, with the `nestrs new` invocation, the environment every `nestrs`
/// runs under, and an inspection hook over the generated tree and what each
/// generator printed.
fn scaffold_write_and_check_in(
    new: &[&str],
    generate: &[&[&str]],
    env: &[(&str, &str)],
    write: &[(&str, &str)],
    what: &str,
    inspect: impl FnOnce(&Path, &[String]),
) {
    let dir = tempfile::tempdir().expect("a temp dir");
    nestrs(dir.path(), new, env);
    let workspace = dir.path().join("acme");
    let printed: Vec<String> = generate
        .iter()
        .map(|args| nestrs(&workspace, args, env))
        .collect();
    for (path, body) in write {
        std::fs::write(workspace.join(path), body).expect("the generated tree is writable");
    }

    inspect(&workspace, &printed);

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
    scaffold_write_and_check_in(
        &["new", "acme"],
        &[],
        &[],
        &[],
        "the scaffolded workspace",
        |workspace, _| {
            // `cargo check` never reads the Justfile.
            let justfile = read(workspace, "Justfile");
            assert!(
                !justfile.contains("{{env_prefix"),
                "the Justfile carries an unrendered placeholder:\n{justfile}",
            );
        },
    );
}

/// The empty `define` the auth adapter scaffolds, as `ability.rs` spells it.
const EMPTY_DEFINE: &str = "fn define(&self, _actor: &Claims, _ab: &mut AbilityBuilder) {}";

#[test]
fn a_generated_resource_and_entity_compile_with_the_grants_they_print() {
    // `#[crud]` and `#[expose]` need a real entity and service, so their contract
    // is proved here, not in `nest-rs-macro-hygiene`. The entity sits on a plain
    // `g feature` port, whose service is no `CrudService`; the grants each
    // generator prints are pasted into `define`, as the developer would.
    scaffold_write_and_check_in(
        &["new", "acme"],
        &[
            &["g", "resource", "post"],
            &["g", "feature", "blog"],
            &["g", "entity", "blog/article"],
        ],
        &[],
        &[],
        "a generated resource and entity, with the grants they print",
        |workspace, printed| {
            let grants: Vec<&str> = printed
                .iter()
                .flat_map(|out| out.lines().map(str::trim))
                .filter(|line| line.starts_with("ab.can("))
                .collect();
            assert_eq!(
                grants.len(),
                2,
                "each generator prints its grant:\n{printed:?}"
            );

            let path = "crates/features/src/authz/ability.rs";
            let ability = read(workspace, path);
            assert!(
                ability.contains(EMPTY_DEFINE),
                "the scaffolded ability has its empty `define`:\n{ability}",
            );
            let define = EMPTY_DEFINE
                .replace("_ab:", "ab:")
                .replace("{}", &format!("{{\n{}\n}}", grants.join("\n")));
            std::fs::write(workspace.join(path), ability.replace(EMPTY_DEFINE, &define))
                .expect("the generated tree is writable");
        },
    );
}

/// `crates/features/src/lib.rs` with the auth adapter's modules dropped.
///
/// Rust compiles the module tree, not the directory, so undeclaring them takes
/// `authn/claims.rs` — and its `uuid` — out of the build.
const RESOURCE_ONLY_LIB: &str = r#"pub mod hello;
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

/// The resource's HTTP module without the `AuthzModule` import the guards
/// brought.
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
    // Without the auth adapter (whose claims name `uuid`) and the guards, the
    // crate's whole claim on `uuid` is whatever `#[crud]` emits: a macro expansion
    // may not put a line in a manifest.
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
        |workspace, _| {
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
/// A transcription, so it can drift from the page.
const LIFECYCLE_PAGE_SERVICE: &str = r#"use nest_rs::core::{hooks, injectable};

#[injectable]
#[derive(Default)]
pub struct BlogService;

#[hooks]
impl BlogService {
    #[on_application_bootstrap]
    async fn warm(&self) -> anyhow::Result<()> {
        tracing::info!(target: crate::blog::TARGET, entries = 0, "cache warmed");
        Ok(())
    }

    #[on_application_shutdown]
    async fn flush(&self) {
        tracing::info!(target: crate::blog::TARGET, pending = 0, "buffers flushed");
    }
}
"#;

#[test]
fn a_feature_can_log_and_return_a_fallible_hook() {
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
    // `--env-prefix` holds only when the variable setting the prefix and the `.env`
    // keys agree; `g auth` appends to an existing cascade, so it runs here.
    scaffold_write_and_check_in(
        &["new", "acme", "--env-prefix", "ACME"],
        &[&["g", "auth"]],
        &[("NESTRS_ENV_PREFIX", "ACME")],
        &[],
        "a workspace scaffolded with a custom env prefix",
        |workspace, _| {
            assert!(
                read(workspace, "Justfile").contains(r#"export NESTRS_ENV_PREFIX := "ACME""#),
                "the Justfile must set the prefix for every process it starts",
            );

            for file in [
                ".env",
                ".env.development",
                ".env.example",
                ".env.local",
                ".env.test",
            ] {
                let body = read(workspace, file);
                assert!(
                    !body.contains("NESTRS_"),
                    "{file} still writes a NESTRS_ key the app will never read:\n{body}",
                );
            }
            assert!(
                !read(workspace, ".env").contains("ENV_PREFIX"),
                "the prefix cannot come from `.env` — the runtime aborts on it",
            );
            assert!(read(workspace, ".env").contains("ACME_SEAORM__URL="));
            assert!(read(workspace, ".env.development").contains("ACME_LOG="));
            for file in [".env.local", ".env.test"] {
                assert!(
                    read(workspace, file).contains("\nACME_AUTHN__SECRET="),
                    "`g auth` must write {file}'s secret under the project's own prefix",
                );
            }
            assert!(
                !read(workspace, ".env").contains("\nACME_AUTHN__SECRET="),
                "the committed `.env`, read in every environment, holds no secret",
            );
        },
    );
}

/// `nestrs g <edge> <feature>` for every framework edge but `skip`, each run
/// **from inside the scaffolded app** (`-p apps/hello`), so the generator also
/// edits the app's `module.rs` and manifest. The kernel's vocabulary, so an
/// edge it gains fails here until its generator ships.
fn every_edge<'a>(feature: &'a str, skip: &[&str]) -> Vec<Vec<&'a str>> {
    nest_rs_core::Edge::ALL
        .iter()
        .map(|edge| edge.as_str())
        .filter(|edge| !skip.contains(edge))
        .map(|edge| vec!["g", edge, feature, "-p", "apps/hello"])
        .collect()
}

/// `generate`, borrowed in the shape [`scaffold_and_check`] takes.
fn check_all(generate: &[Vec<&str>], what: &str) {
    let generate: Vec<&[&str]> = generate.iter().map(Vec::as_slice).collect();
    scaffold_and_check(&generate, what);
}

#[test]
fn every_adapter_over_a_plain_port_compiles() {
    let mut generate = vec![vec!["g", "feature", "blog"]];
    generate.extend(every_edge("blog", &[]));
    // An app added to a workspace that already has one.
    generate.push(vec!["new", "admin"]);
    check_all(
        &generate,
        "every adapter over a plain port, and a second app",
    );
}

#[test]
fn every_adapter_over_a_resource_port_compiles() {
    // The authz bridges share the `authz/` tree and name providers behind
    // features only a compile can judge. `http` is skipped: `g resource` writes it.
    let mut generate = vec![vec!["g", "resource", "post", "-p", "apps/hello"]];
    generate.extend(every_edge("post", &["http"]));
    generate.push(vec!["g", "migration", "create_post"]);
    check_all(
        &generate,
        "every adapter over a resource port, and its migration",
    );
}
