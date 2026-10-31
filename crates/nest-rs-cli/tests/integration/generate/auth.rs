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
    assert!(lib.contains("pub use authn::{Claims, Role, Subject};"));

    // The dev token route is `#[public]` by necessity; the boot refusal keeps it
    // out of production.
    let controller = fs::read_to_string(src.join("authn/http/controller.rs")).unwrap();
    assert!(
        controller.contains("#[post(\"/dev-token\")]") && controller.contains("#[public]"),
        "the development token route is what a scaffolded app calls its guarded routes with: \
         {controller}"
    );
    // The development-only gate is the framework's: one guard that refuses
    // the request and, at its module-init hook, the boot outside development
    // and test — nothing of it is written into the app.
    assert!(
        controller.contains("use nest_rs::authn::{DevelopmentOnlyGuard, JwtService};")
            && controller.contains("#[use_guards(DevelopmentOnlyGuard)]"),
        "the controller binds the framework's gate, so the refusal travels with the route: \
         {controller}"
    );
    let module = fs::read_to_string(src.join("authn/http/module.rs")).unwrap();
    assert!(
        module.contains("providers = [DevelopmentOnlyGuard, DevTokenController]"),
        "the module lists the gate, so the boot refusal runs wherever the route is served: \
         {module}"
    );
    for written_by_hand in ["authn/http/audit.rs", "authn/http/guard.rs"] {
        assert!(
            !src.join(written_by_hand).exists(),
            "{written_by_hand} is the framework's now"
        );
    }

    let second = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "auth", "-p", dir.path().to_str().unwrap()])
        .current_dir(dir.path())
        .output()
        .unwrap();
    assert!(!second.status.success());
}

// The app lists the route `g auth` serves, and nothing an adapter brings: the
// authn and authz ports reach it through every adapter binding their guards.
#[test]
fn generate_auth_wires_the_route_it_serves_into_the_app() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");

    run_ok(&app, &["g", "auth"]);

    let module = fs::read_to_string(app.join("src/module.rs")).unwrap();
    assert!(
        module.contains("use features::authn::AuthnHttpModule;")
            && module.contains("AuthnHttpModule,"),
        "the imports array must list AuthnHttpModule: {module}"
    );
    for ident in ["AuthnModule", "AuthzModule"] {
        assert!(
            !module.contains(&format!("{ident},")),
            "the root lists no {ident}, which the adapters import: {module}"
        );
    }
    assert!(module.contains("        HttpModule,"), "{module}");
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

/// RFC 6749 §5.1: the development token answers the framework's token
/// response — the token a `Secret`, never cached — rather than a body and
/// headers of the app's own.
#[test]
fn generate_auth_dev_token_route_answers_the_framework_s_token_response() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    run_ok(dir.path(), &["g", "auth"]);

    let controller = fs::read_to_string(
        dir.path()
            .join("crates/features/src/authn/http/controller.rs"),
    )
    .unwrap();
    assert!(
        controller.contains("use nest_rs::oauth::server::AccessTokenResponse;")
            && controller.contains("-> Result<AccessTokenResponse> {")
            && controller.contains("AccessTokenResponse::bearer(")
            && controller.contains("self.jwt.sign(&claims).opaque()?,")
            && controller.contains("self.jwt.ttl_secs(),"),
        "the dev-token route returns the framework's response:\n{controller}",
    );
    for hand_built in ["DevTokenResponseDto", "\"Bearer\"", "#[response_header("] {
        assert!(
            !controller.contains(hand_built),
            "`{hand_built}` is the framework's to write:\n{controller}",
        );
    }
    let features = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(
        features.contains("\"oauth-server\""),
        "the features crate enables the response's capability: {features}",
    );
}

/// RFC 9068 §2.2: every access token names its subject — a user, or a client
/// acting on its own behalf by its client id — so the claims hold either kind,
/// and a token naming none is refused.
#[test]
fn generate_auth_claims_name_a_subject_of_either_kind() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    run_ok(dir.path(), &["g", "auth"]);

    let authn = dir.path().join("crates/features/src/authn");
    let claims = fs::read_to_string(authn.join("claims.rs")).unwrap();
    assert!(
        claims.contains("pub sub: Subject,")
            && claims.contains("User(Uuid),")
            && claims.contains("Client(String),"),
        "the subject is a user or a client:\n{claims}"
    );
    assert!(
        !claims.contains("sub: Option<"),
        "no subject is optional:\n{claims}"
    );
    assert!(
        claims.contains("Serialize, Deserialize, PrincipalIdentity)]\npub struct Claims {")
            && claims.contains("#[principal(actor)]\n    pub sub: Subject,"),
        "the claims derive their identity, `actor_id` reading the subject:\n{claims}"
    );
    assert!(
        !claims.contains("impl PrincipalIdentity") && !claims.contains("impl AmbientValue"),
        "the derive writes both impls, so the claims write neither:\n{claims}"
    );
    let controller = fs::read_to_string(authn.join("http/controller.rs")).unwrap();
    assert!(
        controller.contains("sub: Subject::User("),
        "the development token names a user:\n{controller}"
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
