//! `nestrs new` — the greenfield workspace, and adding an app to an existing
//! one.

use crate::harness::{
    assert_env_example_points_test_overrides_somewhere_loaded, run_ok, write_fake_workspace,
};
use std::fs;
use std::process::Command;

/// R9-2: `cov` used to be the one recipe a fresh project could not run —
/// `cargo-llvm-cov` was outside the bootstrap and the LLVM tools it shells out
/// to were pinned nowhere, so the recipe's own comment told the developer to run
/// two installs by hand. Both halves are wired now, and that sentence is the
/// defect this asserts is gone: a scaffold that asks for a manual step has not
/// delivered "one command".
#[track_caller]
fn assert_cov_asks_for_no_manual_install(test_just: &str) {
    assert!(test_just.contains("cov:"), "{test_just}");
    assert!(
        !test_just.contains("rustup component add") && !test_just.contains("cargo install"),
        "the `cov` recipe sends the developer off to install something by hand — \
         `cargo-llvm-cov` is bootstrapped and `llvm-tools-preview` is pinned in \
         `rust-toolchain.toml`: {test_just}",
    );
    assert!(
        test_just.contains("rust-toolchain.toml"),
        "…and it says where the LLVM tools come from: {test_just}",
    );
    assert!(
        test_just.contains("LLVM_COV") && test_just.contains("LLVM_PROFDATA"),
        "…keeping the escape hatch for a toolchain that ignores that file: {test_just}",
    );
}

/// `just --list` renders the **last** comment line above a recipe and nothing
/// else, so a two-line explanation leaves `nestrs run test` describing the
/// recipe with whatever fragment happened to wrap last — `cov` read
/// `# entirely — set LLVM_COV / LLVM_PROFDATA there.` and `e2e` read
/// `# `--no-tests=pass` keeps this green until you write the first one.`. The
/// summary therefore goes at the *bottom* of a block, however odd that reads in
/// the file.
///
/// Asserted as the property rather than per recipe: the line `just` renders must
/// open a sentence, which it does when the block is one line or when the line
/// before it closed one. A recipe that grows a second comment line later is
/// covered without anyone remembering this.
#[track_caller]
fn assert_every_recipe_is_listed_by_a_whole_sentence(test_just: &str) {
    let lines: Vec<&str> = test_just.lines().map(str::trim).collect();
    for (index, line) in lines.iter().enumerate() {
        // A recipe header, and not `_default`, which `--list` hides.
        if line.starts_with('#') || line.starts_with('_') || !line.contains(':') || index < 2 {
            continue;
        }
        let (doc, before) = (lines[index - 1], lines[index - 2]);
        if !doc.starts_with('#') || !before.starts_with('#') {
            continue;
        }
        assert!(
            before.ends_with('.'),
            "`just --list` documents `{line}` with `{doc}`, which continues the \
             line before it — put the one-sentence summary last in the block",
        );
    }
}

/// The generated `rust-toolchain.toml` is what makes `nestrs run lint` and
/// `nestrs run test cov` work on a project minutes old, and it does so silently:
/// `clippy` and `rustfmt` happen to be in rustup's default profile, so a
/// scaffold declaring nothing works on most machines and fails on a minimal one
/// with an error naming rustup rather than the recipe. `llvm-tools-preview` is
/// the one nobody has by default, and it is pinned here rather than installed
/// per machine because `llvm-profdata` only reads a `.profraw` written by the
/// LLVM that rustc was built with — the component has to follow `channel`.
///
/// Asserted on the written file rather than on the const: a component silently
/// dropped from the list is exactly the drift nothing else here would catch.
#[track_caller]
fn assert_toolchain_pins_what_the_recipes_shell_out_to(root: &std::path::Path) {
    let toolchain = fs::read_to_string(root.join("rust-toolchain.toml")).unwrap();
    for component in ["clippy", "rustfmt", "llvm-tools-preview"] {
        assert!(
            toolchain.contains(&format!("\"{component}\"")),
            "`rust-toolchain.toml` pins no `{component}`, so a recipe that shells \
             out to it fails on a toolchain that does not happen to carry it: \
             {toolchain}",
        );
    }
}

/// `AGENTS.md` is the only place a generated project states its layout and
/// naming rules — a tree of four files teaches nothing about the fifth. A
/// scaffold that drops it hands the next contributor, human or agent, a blank
/// slate, and the conventions get re-derived differently in every project.
///
/// Asserts the load-bearing parts rather than the prose: the layout section, the
/// four naming levels, the provider procedure, the reserved vocabulary, the
/// crate-type table, and a fully rendered span target (an unsubstituted
/// placeholder would ship as advice).
#[track_caller]
fn assert_agents_md_carries_the_conventions(root: &std::path::Path) {
    let agents = fs::read_to_string(root.join("AGENTS.md")).expect("AGENTS.md is scaffolded");
    assert!(agents.contains("## Layout — two homes"), "{agents}");
    // Claude Code reads CLAUDE.md and nothing else, so the conventions reach it
    // only through the import. A symlink would need Developer Mode on Windows.
    let claude = fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md is scaffolded");
    assert!(claude.contains("@AGENTS.md"), "{claude}");
    // Assert on a heading the conventions actually carry: a marker that appears
    // in neither file passes whatever either one grows into.
    assert!(
        !claude.contains("## Reserved vocabulary"),
        "CLAUDE.md is the pointer — duplicating the conventions is what drifts:\n{claude}"
    );
    for rule in [
        // The five naming levels, the decision procedure, and the two rules a
        // generated project cannot infer from four files: what a name may not
        // be, and what happens when a role repeats.
        "## Names — five levels",
        "## Modules — two files, two jobs",
        "## Providers — three questions",
        "## Several of the same role",
        "## Reserved vocabulary",
        "`*_module.rs`",
        "`config.rs`",
    ] {
        assert!(
            agents.contains(rule),
            "AGENTS.md is missing {rule}:\n{agents}"
        );
    }
    assert!(
        agents.contains("## Crates — a type, and a direction"),
        "the crate-type table is what says which crate may depend on which:\n{agents}"
    );
    assert!(
        agents.contains("features::users"),
        "the span-target example must be rendered:\n{agents}"
    );
    assert!(
        !agents.contains("{{"),
        "an unrendered placeholder shipped into AGENTS.md:\n{agents}"
    );
}

#[test]
fn new_workspace_greenfield() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "acme"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let root = dir.path().join("acme");
    assert!(root.join("Cargo.toml").is_file());
    assert!(
        root.join("crates/features/src/hello/http/controller.rs")
            .is_file()
    );
    // The default app and demo feature are both named `hello`.
    assert!(root.join("apps/hello/src/module.rs").is_file());
    assert!(!root.join("apps/hello/src/controller.rs").exists());
    // The scaffolded smoke test needs no live infra ⇒ `integration` suite,
    // with an empty `e2e` suite beside it so the nextest filtersets resolve.
    assert!(root.join("apps/hello/tests/integration/main.rs").is_file());
    assert!(root.join("apps/hello/tests/e2e/main.rs").is_file());
    let smoke = fs::read_to_string(root.join("apps/hello/tests/integration/main.rs")).unwrap();
    assert!(
        !smoke.contains("with_test_telemetry"),
        "that builder method is behind an optional feature the scaffold does not enable"
    );
    // The db verbs name these two crates in every recipe.
    assert!(root.join("crates/migrations/src/bin/migrate.rs").is_file());
    assert!(root.join("crates/migrations/src/migrator.rs").is_file());
    assert!(root.join("crates/seed/src/main.rs").is_file());
    assert_agents_md_carries_the_conventions(&root);
    // No Dockerfile ships in workspace mode, so nothing to ignore for.
    assert!(!root.join(".dockerignore").exists());
    assert!(root.join("Justfile").is_file());
    let justfile = fs::read_to_string(root.join("Justfile")).unwrap();
    assert!(justfile.contains("dev app=\"hello\""));
    // `build --all` is a conditional on the single `build` recipe, not a separate recipe.
    assert!(!justfile.contains("build-all"));
    assert!(justfile.contains(r#"if app == "--all""#));
    assert!(justfile.contains("mod test"));
    assert!(justfile.contains("mod db"));
    assert_toolchain_pins_what_the_recipes_shell_out_to(&root);

    let test_just = fs::read_to_string(root.join("test.just")).unwrap();
    assert!(test_just.contains("unit:"));
    assert!(test_just.contains("e2e:"));
    assert!(test_just.contains("cargo test --workspace --doc"));
    assert_cov_asks_for_no_manual_install(&test_just);
    assert_every_recipe_is_listed_by_a_whole_sentence(&test_just);
    let db_just = fs::read_to_string(root.join("db.just")).unwrap();
    assert!(db_just.contains("up:"));
    assert!(db_just.contains("reset: fresh seed"));

    let module = fs::read_to_string(root.join("apps/hello/src/module.rs")).unwrap();
    assert!(module.contains("HelloHttpModule"));
    assert!(module.contains("features::hello"));
    assert!(module.contains("port: 3000"));

    let env = fs::read_to_string(root.join(".env")).unwrap();
    assert!(!env.contains("NESTRS_HTTP__PORT"));
    assert_env_example_points_test_overrides_somewhere_loaded(&root);

    let cargo = fs::read_to_string(root.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("members = [\"crates/*\", \"apps/*\"]"));
    assert_feature_code_can_log_and_fail(&cargo, &root);
}

/// R12 L-1: the workspace shipped `tracing-subscriber` in the root manifest and
/// no `tracing` anywhere — so the crate the developer actually writes in could
/// configure logging and not emit a line. Thirteen docs pages write `tracing::`
/// in feature code and none says to add it; the same page adds
/// `-> anyhow::Result<()>` on a `#[hooks]` method, and `anyhow` was missing from
/// the features crate too. Both are the developer's *own* source naming its own
/// crate (`CLAUDE.md`: the manifest names what the source names), so the
/// scaffold declares them rather than the umbrella re-exporting them.
///
/// Text-level here; `tests/e2e/scaffold.rs` compiles a feature that uses both.
fn assert_feature_code_can_log_and_fail(workspace: &str, root: &std::path::Path) {
    let features = fs::read_to_string(root.join("crates/features/Cargo.toml")).unwrap();
    for dep in ["anyhow", "tracing"] {
        assert!(
            workspace.contains(&format!("\n{dep} = ")),
            "`[workspace.dependencies]` must declare `{dep}`: {workspace}"
        );
        assert!(
            features.contains(&format!("{dep}.workspace = true")),
            "the features crate must declare `{dep}`: {features}"
        );
    }
}

#[test]
fn new_app_inside_nestrs_workspace() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "demo-api"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = dir.path().join("apps/demo-api");
    assert!(app.join("src/module.rs").is_file());
    assert!(app.join("src/main.rs").is_file());
    // Logic never lands in an app crate — the greeting is a feature.
    assert!(!app.join("src/controller.rs").exists());

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3000"));
    assert!(!module.contains("for_root(None)"));

    // The norm: an app added to a workspace gets its own `hello` feature, so it
    // answers on `/` the first time it runs rather than 404ing.
    let feature = dir.path().join("crates/features/src/demo_api");
    assert!(feature.join("service.rs").is_file());
    let controller = fs::read_to_string(feature.join("http/controller.rs")).unwrap();
    assert!(
        controller.contains(r#"#[controller(path = "/")]"#) && controller.contains("#[public]"),
        "the scaffolded app must mount a public GET /: {controller}"
    );
    assert!(
        module.contains("DemoApiHttpModule") && module.contains("features::demo_api"),
        "and the app must import it: {module}"
    );

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod demo_api;"), "features lib.rs: {lib}");
    // The pre-existing feature declaration survives the edit.
    assert!(lib.contains("pub mod users;"), "features lib.rs: {lib}");

    // A smoke test ships with the greeting it asserts.
    assert!(app.join("tests/integration/main.rs").is_file());
}

/// `nestrs new posts` where a `posts` feature already exists would clobber
/// product code. Refuse instead — the app name is free to change.
#[test]
fn new_app_refuses_to_reuse_an_existing_feature_name() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    fs::create_dir_all(dir.path().join("crates/features/src/users")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "users"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(!dir.path().join("apps/users").exists());
}

#[test]
fn new_app_picks_next_http_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    fs::create_dir_all(dir.path().join("apps/auth/src")).unwrap();
    fs::write(
        dir.path().join("apps/auth/src/module.rs"),
        "HttpModule::for_root(HttpConfig { port: 3001, ..Default::default() })",
    )
    .unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "blog"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let module = fs::read_to_string(dir.path().join("apps/blog/src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3002"));
}

#[test]
fn new_app_inside_workspace_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    fs::create_dir_all(dir.path().join("apps/blog")).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "blog"])
        .current_dir(dir.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("already exists"));
    assert!(stderr.contains("blog"));
}

#[test]
fn new_workspace_app_scaffold() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "blog", "-o"])
        .arg(dir.path())
        .output()
        .unwrap();

    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let app = dir.path().join("apps/blog");
    assert!(app.join("src/module.rs").is_file());
    assert!(app.join("src/lib.rs").is_file());
    assert!(!app.join("src/controller.rs").exists());
    assert!(dir.path().join(".env").is_file());
    assert!(dir.path().join(".env.development").is_file());
    assert!(dir.path().join("Justfile").is_file());
    assert!(dir.path().join(".gitignore").is_file());
    assert!(dir.path().join("compose.yml").is_file());

    // The committed `.env` points at the compose services (T26): the DB URL is
    // active so `nestrs run db up` works out of the box; the port stays code.
    let env = fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(!env.contains("NESTRS_HTTP__PORT"));
    assert!(env.contains("NESTRS_SEAORM__URL=postgres://"));

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3000"));
    assert!(!module.contains("OpenTelemetryModule"));
}

/// B8: the scaffolded smoke test booted the app **root**, so the moment a
/// resource was wired the way `g resource` instructs, the root imported
/// `SeaOrmDatabaseModule`, the connection opened during `build()`, and the suite
/// three separate places define as infrastructure-free failed on a 30 s pool
/// timeout. It must boot the narrowest module that serves the greeting.
#[test]
fn the_scaffolded_smoke_test_boots_the_feature_not_the_app_root() {
    let dir = tempfile::tempdir().unwrap();
    run_ok(dir.path(), &["new", "acme"]);

    let smoke =
        fs::read_to_string(dir.path().join("acme/apps/hello/tests/integration/main.rs")).unwrap();
    assert!(
        smoke.contains("HelloHttpModule"),
        "the smoke test must boot the feature's HTTP module: {smoke}",
    );
    assert!(
        !smoke.contains("module::<HelloModule>"),
        "booting the app root drags in every connection it later imports: {smoke}",
    );
}

/// P3 / G15, inverted: `validator` used to need a pin matching the framework's
/// own major, because a `#[config]` struct derived `Validate` at the call site
/// and two copies of the trait in one graph read as "you wrote the impl wrong".
/// `#[config]` now carries the derive and points it back through the framework,
/// so the scaffold must **not** write the entry at all — a pin here would put a
/// second copy back in the graph, which is the defect it was invented to avoid.
#[test]
fn the_scaffold_leaves_validator_to_the_framework() {
    let dir = tempfile::tempdir().unwrap();
    run_ok(dir.path(), &["new", "acme"]);
    let cargo = fs::read_to_string(dir.path().join("acme/Cargo.toml")).unwrap();
    assert!(!cargo.contains("validator"), "{cargo}");
}
