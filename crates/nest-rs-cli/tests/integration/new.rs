//! `nestrs new` — the standalone crate, the greenfield workspace, and adding an
//! app to an existing one.

use crate::harness::{
    assert_env_example_points_test_overrides_somewhere_loaded, run_ok, write_fake_workspace,
};
use std::fs;
use std::process::Command;

#[test]
fn new_standalone_hello_template() {
    let dir = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["new", "demo-api", "--standalone"])
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
    assert!(app.join("src/lib.rs").is_file());
    assert!(app.join("src/controller.rs").is_file());
    // The scaffolded smoke test needs no live infra ⇒ `integration` suite.
    assert!(app.join("tests/integration/main.rs").is_file());
    // …and an empty `e2e` suite beside it: the test recipes filter on
    // `binary(e2e)`, which nextest refuses to parse when nothing matches.
    assert!(app.join("tests/e2e/main.rs").is_file());
    assert!(app.join("Cargo.toml").is_file());
    assert!(app.join("Dockerfile").is_file());
    assert!(app.join(".dockerignore").is_file());
    assert!(app.join("rust-toolchain.toml").is_file());
    assert!(app.join(".env").is_file());
    assert!(app.join(".env.development").is_file());
    let dev_env = fs::read_to_string(app.join(".env.development")).unwrap();
    assert!(dev_env.contains("NESTRS_LOG=debug"));
    assert_env_example_points_test_overrides_somewhere_loaded(&app);

    let main_rs = fs::read_to_string(app.join("src/main.rs")).unwrap();
    // Baseline logging is nest-rs-core's job now — a scaffold must not pull
    // the observability stack; it stays a documented opt-in.
    assert!(!main_rs.contains("OpenTelemetry"));
    assert!(main_rs.contains("Environment::init"));

    let module_rs = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(!module_rs.contains("OpenTelemetryModule"));

    let cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    assert!(cargo.contains("[workspace]"));
    // One entry, capabilities as features — `http` carries guards, pipes and
    // the layer crates the decorators expand into.
    assert!(cargo.contains("nest-rs"), "{cargo}");
    assert!(cargo.contains("\"http\""), "{cargo}");
    assert!(!cargo.contains("nest-rs-opentelemetry"));
    assert!(app.join(".gitignore").is_file());
    assert!(app.join("Justfile").is_file());
    let justfile = fs::read_to_string(app.join("Justfile")).unwrap();
    assert!(justfile.contains("build:"));
    assert!(justfile.contains("cargo build --release"));
    assert!(justfile.contains("mod test"));
    // The db verbs drive the workspace's `migrations`/`seed` crates, which a
    // single crate has nowhere to put — so neither the module nor the file.
    assert!(!justfile.contains("mod db"));
    assert!(!app.join("db.just").exists());
    assert!(app.join("test.just").is_file());
    let test_just = fs::read_to_string(app.join("test.just")).unwrap();
    assert!(test_just.contains("unit:"));
    assert!(test_just.contains("e2e:"));
    assert!(test_just.contains("doc:"));
    assert!(test_just.contains("cargo test --doc"));
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

    let test_just = fs::read_to_string(root.join("test.just")).unwrap();
    assert!(test_just.contains("unit:"));
    assert!(test_just.contains("e2e:"));
    assert!(test_just.contains("cargo test --workspace --doc"));
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
    assert!(env.contains("NESTRS_DATABASE__URL=postgres://"));

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(module.contains("HttpConfig { port: 3000"));
    assert!(!module.contains("OpenTelemetryModule"));
}

/// B8: the scaffolded smoke test booted the app **root**, so the moment a
/// resource was wired the way `g resource` instructs, the root imported
/// `DatabaseModule`, the connection opened during `build()`, and the suite
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

    // Standalone has no separate feature crate — its root module *is* the one
    // that serves the greeting, so the narrowest boot is unchanged there.
    let solo = tempfile::tempdir().unwrap();
    run_ok(solo.path(), &["new", "solo", "--standalone"]);
    let smoke = fs::read_to_string(solo.path().join("solo/tests/integration/main.rs")).unwrap();
    assert!(smoke.contains("TestApp::builder()"), "{smoke}");
}

/// P3 / G15, inverted: `validator` used to need a pin matching the framework's
/// own major, because a `#[config]` struct derived `Validate` at the call site
/// and two copies of the trait in one graph read as "you wrote the impl wrong".
/// `#[config]` now carries the derive and points it back through the framework,
/// so the scaffold must **not** write the entry at all — a pin here would put a
/// second copy back in the graph, which is the defect it was invented to avoid.
#[test]
fn the_scaffold_leaves_validator_to_the_framework() {
    for args in [vec!["new", "acme"], vec!["new", "solo", "--standalone"]] {
        let name = args[1];
        let dir = tempfile::tempdir().unwrap();
        run_ok(dir.path(), &args);
        let cargo = fs::read_to_string(dir.path().join(name).join("Cargo.toml")).unwrap();
        assert!(!cargo.contains("validator"), "{cargo}");
    }
}
