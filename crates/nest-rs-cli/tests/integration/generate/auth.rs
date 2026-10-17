//! `nestrs g auth` — the one auth adapter a workspace gets, and its three roots
//! at the composition site.

use crate::harness::{
    assert_256_bit_hex, assigned, run_ok, scaffolded_var, write_fake_app, write_fake_workspace,
};
use std::fs;
use std::process::Command;

#[test]
fn generate_auth_scaffolds_the_adapter_once() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &["g", "auth", "-p", dir.path().to_str().unwrap()],
    );

    let src = dir.path().join("crates/features/src");
    let ability = fs::read_to_string(src.join("authz/ability.rs")).unwrap();
    assert!(ability.contains("impl AbilityFactory for AuthzAbility"));
    // Both branches are scaffolded empty: `define_visitor` is the only place an
    // anonymous read can be granted.
    assert!(
        ability.contains("fn define(") && ability.contains("fn define_visitor("),
        "the scaffolded policy carries both branches: {ability}"
    );
    let guard = fs::read_to_string(src.join("authz/guard.rs")).unwrap();
    assert!(guard.contains("AbilityGuard<AuthzAbility>"));

    let lib = fs::read_to_string(src.join("lib.rs")).unwrap();
    assert!(lib.contains("pub mod authn;"));
    assert!(lib.contains("pub mod authz;"));
    assert!(lib.contains("pub use authn::{Claims, Role};"));

    // The dev token route is `#[public]` by necessity; the boot refusal keeps it
    // out of production.
    let controller = fs::read_to_string(src.join("authn/http/controller.rs")).unwrap();
    assert!(
        controller.contains("#[post(\"/dev-token\")]") && controller.contains("#[public]"),
        "the development token route is what a scaffolded app calls its guarded routes with: \
         {controller}"
    );
    // On its own provider: a `#[controller]` registers no instance to run hooks on.
    let audit = fs::read_to_string(src.join("authn/http/audit.rs")).unwrap();
    assert!(
        audit.contains("#[on_module_init]") && audit.contains("if is_development()"),
        "the module refuses the boot when this is not a development run: {audit}"
    );
    assert!(
        !controller.contains("#[hooks]"),
        "the refusal must not sit on the controller, where the hook is skipped: {controller}"
    );

    // **Absence must answer `false`**: `<PREFIX>_ENV` is unset in most scaffolds
    // and CI, so the scaffold asks `Environment::declared`, which answers `None`
    // for unset and misspelled alike.
    assert!(
        audit.contains("Environment::declared()")
            && !audit.contains("Environment::from_env()")
            && !audit.contains(r#"Ok("development")"#),
        "the predicate asks the framework's classifier positively, so an unset or hand-parsed \
         variable is not a development run: {audit}"
    );
    // A guard asking the same question covers registration from another module.
    let guard = fs::read_to_string(src.join("authn/http/guard.rs")).unwrap();
    assert!(
        guard.contains("impl Guard for DevOnlyGuard") && guard.contains("if is_development()"),
        "the route's own refusal is a guard: {guard}"
    );
    assert!(
        controller.contains("#[use_guards(DevOnlyGuard)]"),
        "the controller binds it, so the refusal travels with the route: {controller}"
    );

    let second = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "auth", "-p", dir.path().to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!second.status.success());
}

// Every auth root reaches the composition site from one run, in one edit:
// separate edits would drop all but the last.
#[test]
fn generate_auth_wires_every_root_into_the_app() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");

    run_ok(&app, &["g", "auth"]);

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    for (use_path, ident) in [
        ("features::authn::AuthnModule", "AuthnModule"),
        ("features::authn::AuthnHttpModule", "AuthnHttpModule"),
        ("features::authz::AuthzModule", "AuthzModule"),
    ] {
        assert!(module.contains(&format!("use {use_path};")), "{module}");
        assert!(
            module.contains(&format!("{ident},")),
            "the imports array must list {ident}: {module}"
        );
    }
    assert!(module.contains("HttpModule::for_root"), "{module}");
}

fn generated_secrets() -> (String, String) {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let printed = run_ok(dir.path(), &["g", "auth"]);

    let var = scaffolded_var("AUTHN", "SECRET");
    assert_eq!(
        assigned(&dir.path().join(".env"), &var),
        None,
        "the committed `.env`, read in every environment, assigns no signing secret",
    );
    let local = assigned(&dir.path().join(".env.local"), &var).expect("`.env.local` holds one");
    let test = assigned(&dir.path().join(".env.test"), &var).expect("`.env.test` holds one");
    assert!(
        !printed.contains(&local) && !printed.contains(&test),
        "the run prints no secret it drew:\n{printed}",
    );
    (local, test)
}

#[test]
fn generate_auth_draws_a_secret_per_file_and_commits_none_in_env() {
    let (local, test) = generated_secrets();
    assert_256_bit_hex(&local);
    assert_256_bit_hex(&test);
    assert_ne!(local, test, "the suites' key is not the developer's");

    let (other_local, other_test) = generated_secrets();
    assert_ne!(local, other_local, "no two projects share a key");
    assert_ne!(test, other_test, "no two projects share a key");
}

#[test]
fn generate_auth_tells_a_teammate_which_variable_their_env_local_needs() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    run_ok(dir.path(), &["g", "auth"]);

    let var = scaffolded_var("AUTHN", "SECRET");
    let example = fs::read_to_string(dir.path().join(".env.example")).unwrap();
    assert!(
        example.lines().any(|line| line == format!("# {var}=")),
        "`.env.example` names the variable, commented and empty:\n{example}",
    );
    let env = fs::read_to_string(dir.path().join(".env")).unwrap();
    assert!(
        env.contains("`.env.local`") && env.contains("openssl rand -hex 32"),
        "`.env` says where the secret lives and how to draw one:\n{env}",
    );
}

/// A file that exists is edited, and an edit prints what it added: never the value.
#[test]
fn generate_auth_appending_to_an_existing_env_local_prints_no_secret() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    fs::write(dir.path().join(".env.local"), "# mine\n").unwrap();

    let printed = run_ok(dir.path(), &["g", "auth"]);

    let var = scaffolded_var("AUTHN", "SECRET");
    let secret = assigned(&dir.path().join(".env.local"), &var).expect("appended");
    assert!(
        fs::read_to_string(dir.path().join(".env.local"))
            .unwrap()
            .starts_with("# mine\n"),
        "the developer's own lines stay first",
    );
    assert!(!printed.contains(&secret), "{printed}");
    assert!(
        printed.contains(&format!("{var}=<redacted>")),
        "the diff names the variable it added:\n{printed}",
    );
}

#[test]
fn generate_auth_leaves_a_file_that_already_names_authn_untouched() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let mine = format!(
        "{}=/run/secrets/authn.pem\n",
        scaffolded_var("AUTHN", "PRIVATE_KEY_FILE"),
    );
    fs::write(dir.path().join(".env.local"), &mine).unwrap();

    run_ok(dir.path(), &["g", "auth"]);

    assert_eq!(
        fs::read_to_string(dir.path().join(".env.local")).unwrap(),
        mine,
        "a developer's own key material is never overwritten nor joined by a secret",
    );
    assert!(
        assigned(
            &dir.path().join(".env.test"),
            &scaffolded_var("AUTHN", "SECRET")
        )
        .is_some(),
        "the other files are still written",
    );
}

/// RFC 6749 §5.1: the development token is never cached either.
#[test]
fn generate_auth_dev_token_route_answers_no_store() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    run_ok(dir.path(), &["g", "auth"]);

    let controller = fs::read_to_string(
        dir.path()
            .join("crates/features/src/authn/http/controller.rs"),
    )
    .unwrap();
    assert!(
        controller.contains(r#"#[response_header("cache-control", "no-store")]"#)
            && controller.contains(r#"#[response_header("pragma", "no-cache")]"#),
        "the dev-token route carries both directives:\n{controller}",
    );
}

/// `.env` is read in every run, so a key set there would sit beside either
/// secret: the boot refuses an HS256 secret next to EdDSA keys.
#[test]
fn generate_auth_draws_no_secret_beside_key_material_env_already_sets() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let env = format!(
        "{}{}=/run/secrets/authn.pub\n{}=/run/secrets/authn.pem\n",
        fs::read_to_string(dir.path().join(".env")).unwrap(),
        scaffolded_var("AUTHN", "PUBLIC_KEY_FILE"),
        scaffolded_var("AUTHN", "PRIVATE_KEY_FILE"),
    );
    fs::write(dir.path().join(".env"), &env).unwrap();

    let printed = run_ok(dir.path(), &["g", "auth"]);

    assert_eq!(fs::read_to_string(dir.path().join(".env")).unwrap(), env);
    let var = scaffolded_var("AUTHN", "SECRET");
    for file in [".env.local", ".env.test"] {
        assert_eq!(
            assigned(&dir.path().join(file), &var),
            None,
            "no secret in `{file}`, which every run reads beside `.env`",
        );
        assert!(
            printed.contains(&format!("`{file}` got no secret: `.env`")),
            "the run says why `{file}` has none:\n{printed}",
        );
    }
}

/// Each secret is kept out by the files of its own runs, the cascade's and no
/// other, and a commented variable sets nothing.
#[test]
fn generate_auth_reads_the_files_each_secrets_runs_read() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let var = scaffolded_var("AUTHN", "SECRET");
    fs::write(
        dir.path().join(".env.development"),
        format!(
            "{}=https://issuer.example/jwks\n",
            scaffolded_var("AUTHN", "JWKS_URI")
        ),
    )
    .unwrap();
    fs::write(dir.path().join(".env.test.local"), format!("# {var}=\n")).unwrap();

    let printed = run_ok(dir.path(), &["g", "auth"]);

    assert_eq!(
        assigned(&dir.path().join(".env.local"), &var),
        None,
        "development reads `.env.development` beside `.env.local`",
    );
    assert!(
        printed.contains("`.env.local` got no secret: `.env.development`"),
        "{printed}"
    );
    let test = assigned(&dir.path().join(".env.test"), &var)
        .expect("the test runs read no `.env.development`, and a comment sets nothing");
    assert_256_bit_hex(&test);
    assert!(
        printed.contains("`.env.test` holds the test suites' HS256 secret."),
        "{printed}"
    );
}
