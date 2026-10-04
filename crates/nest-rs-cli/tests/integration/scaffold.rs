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
/// The environment is explicit because the CLI reads the project's env prefix
/// from its own: a generator writing variable names behaves differently in a
/// shell that names one and a shell that does not. Passing it here is what a
/// developer's `direnv`, devcontainer or `nestrs run` does.
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

/// `cargo clippy --workspace --all-targets --all-features -- -D warnings` over the generated
/// tree — **the gate the generated project sets for itself**.
///
/// It used to be a bare `cargo check`, and that gap shipped two defects: a
/// template importing a name no rendered body used, and a borrow the lint
/// rejects. Both compiled, so both reached a user on their first `just lint` and
/// nowhere earlier. A generator that emits code failing the lint it also emits
/// is a generator defect, so this suite holds it to the same bar rather than a
/// lower one.
///
/// Every scaffold builds into one shared target directory so the framework's
/// artifacts are reused rather than rebuilt per test. Two builds writing it at
/// once corrupt each other, and nextest runs each test in its own process, so a
/// file lock takes them one at a time.
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
    scaffold_write_and_check_in(&["new", "acme"], generate, &[], write, what, |_, _| {});
}

/// The same, with the `nestrs new` invocation, the environment every `nestrs`
/// runs under, and an inspection hook over the generated tree and what each
/// generator printed — for a flag whose effect is spread across the Justfile and
/// the `.env` cascade, or a step a generator tells the developer to take.
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
    // The first thing anyone does with the CLI. If this breaks, `nestrs new`
    // hands a new user a repository that does not build.
    scaffold_write_and_check_in(
        &["new", "acme"],
        &[],
        &[],
        &[],
        "the scaffolded workspace",
        |workspace, _| {
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

/// The empty `define` the auth adapter scaffolds, as `ability.rs` spells it.
const EMPTY_DEFINE: &str = "fn define(&self, _actor: &Claims, _ab: &mut AbilityBuilder) {}";

#[test]
fn a_generated_resource_and_entity_compile_with_the_grants_they_print() {
    // `#[crud]` and `#[expose]` are absent from `nest-rs-macro-hygiene` because
    // they need a real entity and service, so their contract is proved here. The
    // entity sits on a plain `g feature` port on purpose: its service is no
    // `CrudService`, so `#[expose(service = …)]` naming it would fail inside the
    // expansion, the case `g resource` never exercises. A resource serves
    // nothing until its grant is written, and the developer writes it by pasting
    // what each generator prints into `define`, renaming `_ab` as the step says.
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
/// The files stay on disk; Rust compiles the module tree, not the directory, so
/// undeclaring them is enough to take `authn/claims.rs` — and its `uuid` —
/// out of the build.
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
    // a line in a manifest. It shipped anyway, because the compile witness did
    // not apply `#[crud]` then (`nest-rs-macro-hygiene` does now) and the case
    // above passes for an unrelated reason: `g resource` bootstraps `g auth`, whose claims type
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
        |workspace, _| {
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
            assert!(read(workspace, ".env").contains("ACME_SEAORM__URL="));
            assert!(read(workspace, ".env.development").contains("ACME_LOG="));
            assert!(
                read(workspace, ".env").contains("ACME_AUTHN__SECRET="),
                "`g auth` must append its dev secret under the project's own prefix",
            );
        },
    );
}

/// Every edge the CLI generates an adapter for, by its folder — the closed
/// vocabulary `Transport` carries. Listed rather than read from the enum, which
/// is crate-private; `naming`'s unit suite joins the two, so an edge the CLI
/// gains and this list does not fails the crate's own tests instead of shipping
/// a generator no compiler has read.
const EDGES: [&str; 7] = [
    "http", "graphql", "ws", "queue", "schedule", "mcp", "events",
];

/// `nestrs g <edge> <feature>` for every edge in [`EDGES`] but `skip`, each run
/// **from inside the scaffolded app** (`-p apps/hello`). That is where a
/// developer stands, and it is what makes a generator write its edits into the
/// app's `module.rs` and manifest as well as the feature's — two more files the
/// compiler has to agree with, and ones no run from the workspace root touches.
fn every_edge<'a>(feature: &'a str, skip: &[&str]) -> Vec<Vec<&'a str>> {
    EDGES
        .iter()
        .filter(|edge| !skip.contains(edge))
        .map(|&edge| vec!["g", edge, feature, "-p", "apps/hello"])
        .collect()
}

/// `generate`, borrowed in the shape [`scaffold_and_check`] takes.
fn check_all(generate: &[Vec<&str>], what: &str) {
    let generate: Vec<&[&str]> = generate.iter().map(Vec::as_slice).collect();
    scaffold_and_check(&generate, what);
}

#[test]
fn every_adapter_over_a_plain_port_compiles() {
    // The guarantee the CLI page makes — a freshly generated port plus **any**
    // adapter compiles — held over this port for one edge of seven: the suite
    // compiled `events` here and nothing else, so the HTTP, GraphQL, WS, queue,
    // schedule and MCP skeletons were proved by the text
    // assertions alone, which read a wrong import as readily as a right one.
    let mut generate = vec![vec!["g", "feature", "blog"]];
    generate.extend(every_edge("blog", &[]));
    // The other path out of `nestrs new`: an app added to a workspace that has
    // one, with its own `hello` feature and app crate beside the first.
    generate.push(vec!["new", "admin"]);
    check_all(
        &generate,
        "every adapter over a plain port, and a second app",
    );
}

#[test]
fn every_adapter_over_a_resource_port_compiles() {
    // F4: `g ws` and `g mcp` named `AuthzWsModule` / `features::authz::mcp` in
    // their own output while writing neither. They write both now — and a
    // bridge module is exactly the shape the text assertions cannot judge:
    // they assert on the *text* a generator produced, so a `#[module]` naming a
    // provider behind a feature the manifest never enabled reads as correct
    // there and fails on the user's first `cargo check`. The bridges share the
    // `authz/` tree, so this also pins that they land side by side without
    // clobbering each other's index lines.
    //
    // The resolver is the case that made this every edge rather than two: over a
    // resource, `g graphql` bound `#[use_guards(AuthnGuard, AuthzGuard)]` on a
    // `#[resolver]`, which the guard-capability check refuses — `/graphql`
    // authenticates through its bridge, and `AuthnGuard` has no `check_graphql`.
    // It shipped in 6.0 and 6.1, because this test compiled `ws` and `mcp` over a
    // resource and never `graphql`.
    //
    // `http` is skipped because `g resource` writes it — the guarded `#[crud]`
    // controller — and refuses a second. The migration is the resource's table,
    // the pair the scaffolded README generates together.
    let mut generate = vec![vec!["g", "resource", "post", "-p", "apps/hello"]];
    generate.extend(every_edge("post", &["http"]));
    generate.push(vec!["g", "migration", "create_post"]);
    check_all(
        &generate,
        "every adapter over a resource port, and its migration",
    );
}
