//! Covers `src/templates/` and `src/commands/generate/` — by compiling what
//! they wrote, and running a generated binary where its output is the contract.

use std::path::Path;

use nest_rs_core::EnvPrefix;
use nest_rs_core::logging::var;

use crate::generated::{EVERY_COLUMN_TYPE, cargo, cargo_check, nestrs, patch_to_working_tree};

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
    // is proved here, not in `nest-rs-macro-hygiene`. The resource takes every
    // column type, into its entity, its migration and its round trip. One entity
    // sits on a plain `g feature` port, whose service is no `CrudService`; a
    // second on the resource's port moves its first into `entities/`, every path
    // rewritten; `g migration` reads each one's columns. The grants each
    // generator prints are pasted into `define`, as the developer would.
    let mut resource = vec!["g", "resource", "post"];
    resource.extend(EVERY_COLUMN_TYPE);
    scaffold_write_and_check_in(
        &["new", "acme"],
        &[
            &resource,
            &["g", "feature", "blog"],
            &["g", "entity", "blog/article"],
            &["g", "entity", "post/comment"],
            &["g", "migration", "create_article"],
            &["g", "migration", "create_comment"],
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
                3,
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

use crate::post::PostService;

#[controller(path = "/post")]
pub struct PostController {
    #[inject]
    svc: Arc<PostService>,
}

#[crud(service = svc)]
impl PostController {}
"#;

/// The resource's HTTP module without the `AuthzModule` import the guards
/// brought.
const UNGUARDED_CRUD_MODULE: &str = r#"use nest_rs::core::module;
use nest_rs::http::HttpModule;

use super::controller::PostController;
use crate::post::PostModule;

#[module(
    imports = [PostModule, HttpModule],
    providers = [PostController],
)]
pub struct PostHttpModule;
"#;

/// The app's root as `nestrs new` wrote it, and its empty `e2e` suite: what
/// the resource wired into them names the auth adapter this test undeclares.
const HELLO_ONLY_ROOT: &str = r#"use nest_rs::core::module;

use features::hello::HelloHttpModule;

#[module(
    imports = [
        HelloHttpModule,
    ],
)]
pub struct HelloModule;
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
            ("apps/hello/src/module.rs", HELLO_ONLY_ROOT),
            ("apps/hello/tests/e2e/main.rs", ""),
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

/// The `mod.rs` of a `g feature blog` port once it logs, the target declared
/// as `/fundamentals/lifecycle/` spells it.
const LOGGING_BLOG_MOD: &str = r#"pub const TARGET: &str = "features::blog";

mod module;
mod service;

pub use module::BlogModule;
pub use service::BlogService;
"#;

#[test]
fn a_feature_can_log_and_return_a_fallible_hook() {
    scaffold_write_and_check(
        &[&["g", "feature", "blog"]],
        &[
            (
                "crates/features/src/blog/service.rs",
                LIFECYCLE_PAGE_SERVICE,
            ),
            ("crates/features/src/blog/mod.rs", LOGGING_BLOG_MOD),
        ],
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
            assert!(read(workspace, ".env.development").contains("ACME_SEAORM__URL="));
            assert!(read(workspace, ".env.test").contains("ACME_SEAORM__URL="));
            assert!(
                read(workspace, ".env.development").contains("\nRUST_LOG="),
                "the console's filter is the ecosystem's, which no prefix renames",
            );
            assert!(
                read(workspace, "db.just").contains(r#"export ACME_ENV := "development""#),
                "the db recipes declare the profile under the project's own prefix",
            );
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
    // The authz adapters share the `authz/` tree and name providers behind
    // features only a compile can judge. `http` is skipped: `g resource` writes it,
    // with its table's migration. `g migration nestrs_outbox` runs before
    // `g queue`, which registers the same entry: after it, the command would find
    // it listed and write nothing.
    let mut generate = vec![
        vec!["g", "resource", "post", "-p", "apps/hello"],
        vec!["g", "migration", "nestrs_outbox"],
    ];
    generate.extend(every_edge("post", &["http"]));
    check_all(
        &generate,
        "every adapter over a resource port, the outbox's migration and its own",
    );
}

/// The generated `migrate` binary runs no `App`, so it installs the console
/// itself: under `<PREFIX>_LOG_FORMAT=json` its lines are JSON, as an app's
/// are. Its database is unreachable, and the line it files before connecting
/// is the one read.
#[test]
fn the_generated_migration_binary_writes_the_console_its_log_settings_describe() {
    let dir = tempfile::tempdir().expect("a temp dir");
    nestrs(dir.path(), &["new", "acme"], &[]);
    let workspace = dir.path().join("acme");
    patch_to_working_tree(&workspace);

    let format = EnvPrefix::var(var::FORMAT);
    let url = nest_rs_config::var_name("seaorm", "URL");
    let budget = nest_rs_config::var_name("seaorm", "CONNECT_TIMEOUT_SECS");
    let output = cargo(
        &workspace,
        &[
            "run",
            "-q",
            "-p",
            "migrations",
            "--bin",
            "migrate",
            "--",
            "status",
        ],
        &[
            ("RUST_LOG", "info"),
            (format.as_str(), "json"),
            (
                url.as_str(),
                "postgres://acme@127.0.0.1:1/acme?sslmode=verify-full",
            ),
            (budget.as_str(), "1"),
        ],
    )
    .unwrap_or_else(|err| panic!("{err}"));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        !output.status.success(),
        "nothing listens on the database's port: {stdout}"
    );
    let connecting = stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .find(|event| event["fields"]["message"] == "connecting to database")
        .unwrap_or_else(|| {
            panic!(
                "the binary's console wrote its line as JSON:\n{stdout}\n{}",
                String::from_utf8_lossy(&output.stderr)
            )
        });
    assert_eq!(connecting["level"], "INFO", "{connecting}");
}
