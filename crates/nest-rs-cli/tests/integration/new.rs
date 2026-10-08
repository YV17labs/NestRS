//! `nestrs new` — the greenfield workspace, and adding an app to an existing
//! one.

use crate::harness::{
    assert_env_example_points_test_overrides_somewhere_loaded, run_ok, write_fake_workspace,
};
use std::fs;
use std::process::Command;

/// The `cov` recipe asks for no manual install.
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
/// else, so that line must open a sentence: the block is one line, or the line
/// before it closed one.
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

/// `clippy` and `rustfmt` are in rustup's default profile only by chance, and
/// `llvm-tools-preview` must follow `channel`: `llvm-profdata` reads only a
/// `.profraw` from the LLVM rustc was built with.
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
/// Asserts the load-bearing parts rather than the prose, and a fully rendered
/// span target.
#[track_caller]
fn assert_agents_md_carries_the_conventions(root: &std::path::Path) {
    let agents = fs::read_to_string(root.join("AGENTS.md")).expect("AGENTS.md is scaffolded");
    assert!(agents.contains("## Layout — two homes"), "{agents}");
    // Claude Code reads CLAUDE.md alone; a symlink would need Developer Mode on
    // Windows.
    let claude = fs::read_to_string(root.join("CLAUDE.md")).expect("CLAUDE.md is scaffolded");
    assert!(claude.contains("@AGENTS.md"), "{claude}");
    // A marker that appears in neither file passes whatever either grows into.
    assert!(
        !claude.contains("## Reserved vocabulary"),
        "CLAUDE.md is the pointer — duplicating the conventions is what drifts:\n{claude}"
    );
    for rule in [
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
    assert!(root.join("apps/hello/src/module.rs").is_file());
    assert!(!root.join("apps/hello/src/controller.rs").exists());
    // An empty `e2e` suite beside it so the nextest filtersets resolve.
    assert!(root.join("apps/hello/tests/integration/main.rs").is_file());
    assert!(root.join("apps/hello/tests/e2e/main.rs").is_file());
    let smoke = fs::read_to_string(root.join("apps/hello/tests/integration/main.rs")).unwrap();
    assert!(
        !smoke.contains("with_test_telemetry"),
        "that builder method is behind an optional feature the scaffold does not enable"
    );
    assert!(root.join("crates/migrations/src/bin/migrate.rs").is_file());
    assert!(root.join("crates/migrations/src/migrator.rs").is_file());
    assert!(root.join("crates/seed/src/main.rs").is_file());
    assert_agents_md_carries_the_conventions(&root);
    assert!(!root.join(".dockerignore").exists());
    assert!(root.join("Justfile").is_file());
    let justfile = fs::read_to_string(root.join("Justfile")).unwrap();
    assert!(justfile.contains("dev app=\"hello\""));
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

/// The features crate declares `tracing` and `anyhow`: the docs write both in
/// feature code. Text-level here; `scaffold.rs` compiles a feature that uses both.
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
    assert!(!app.join("src/controller.rs").exists());

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3000"));
    assert!(!module.contains("for_root(None)"));

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
    assert!(lib.contains("pub mod users;"), "features lib.rs: {lib}");

    assert!(app.join("tests/integration/main.rs").is_file());
}

/// `nestrs new posts` where a `posts` feature already exists would clobber
/// product code.
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

    // The DB URL is active so `nestrs run db up` works out of the box.
    let env = fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(!env.contains("NESTRS_HTTP__PORT"));
    assert!(env.contains("NESTRS_SEAORM__URL=postgres://"));

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3000"));
    assert!(!module.contains("OpenTelemetryModule"));
}

/// The smoke test boots the narrowest module that serves the greeting, so a
/// wired `SeaOrmDatabaseModule` never reaches it.
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

/// `#[config]` carries the `Validate` derive through the framework, so the
/// scaffold writes no `validator` entry: a pin would put a second copy in the
/// graph.
#[test]
fn the_scaffold_leaves_validator_to_the_framework() {
    let dir = tempfile::tempdir().unwrap();
    run_ok(dir.path(), &["new", "acme"]);
    let cargo = fs::read_to_string(dir.path().join("acme/Cargo.toml")).unwrap();
    assert!(!cargo.contains("validator"), "{cargo}");
}
