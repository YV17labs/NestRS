//! `nestrs g resource` — the CRUD slice, its guarded HTTP form, and the columns
//! it has to declare in lockstep with `g migration`.

use crate::harness::{
    assert_256_bit_hex, assigned, files_under, run_err, run_ok, scaffolded_var, write_fake_app,
    write_fake_migrations_crate, write_fake_workspace,
};
use std::fs;

#[test]
fn generate_resource_creates_crud_slice_and_deps() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
        dir.path(),
        &["g", "resource", "posts", "-p", dir.path().to_str().unwrap()],
    );

    let feature = dir.path().join("crates/features/src/posts");
    assert!(feature.join("entity.rs").is_file());
    assert!(feature.join("service.rs").is_file());
    assert!(feature.join("http/controller.rs").is_file());

    let entity = fs::read_to_string(feature.join("entity.rs")).unwrap();
    assert!(entity.contains("#[expose("));
    assert!(entity.contains("name = \"Post\""));
    assert!(entity.contains("table_name = \"post\""));

    // `schemars` / `validator` / `uuid` / `chrono` are absent: `#[expose]` routes
    // those derives through the framework.
    let root_cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(root_cargo.contains("nest-rs"));
    assert!(root_cargo.contains("seaorm"), "{root_cargo}");
    assert!(
        root_cargo.contains("sea-orm = { version = \"2.0\""),
        "the generated pin tracks the released sea-orm, not a release candidate: {root_cargo}"
    );
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(features_cargo.contains("nest-rs"));
    assert!(features_cargo.contains("authz"), "{features_cargo}");

    // The feature *set* `/tutorial/entity/` prints; `authn` comes with the auth
    // adapter `g resource` bootstraps.
    for feature in ["seaorm", "http", "authz", "authn"] {
        let quoted = format!("\"{feature}\"");
        assert!(
            features_cargo.contains(&quoted),
            "the features crate enables `{feature}`: {features_cargo}",
        );
        assert!(
            root_cargo.contains(&quoted),
            "the workspace pin enables `{feature}`: {root_cargo}",
        );
    }

    // The order `/tutorial/entity/` prints: the generator's own, never re-sorted.
    assert!(
        features_cargo.contains(r#"["seaorm", "authz", "http", "authn"]"#),
        "the features crate lists its features as the tutorial prints them: {features_cargo}",
    );

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod posts;"));
}

#[test]
fn generate_resource_emits_the_guarded_form_and_bootstraps_auth() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let printed = run_ok(
        dir.path(),
        &["g", "resource", "posts", "-p", dir.path().to_str().unwrap()],
    );

    let feature = dir.path().join("crates/features/src/posts");
    let controller = fs::read_to_string(feature.join("http/controller.rs")).unwrap();
    assert!(
        controller.contains("#[use_guards(AuthnGuard, AuthzGuard)]"),
        "a DB-backed controller serves nothing without an ability guard: {controller}"
    );
    assert!(
        controller.contains("#[crud(service = svc)]"),
        "the resource controller uses the #[crud] form, typed off its service: {controller}"
    );

    let module = fs::read_to_string(feature.join("http/module.rs")).unwrap();
    assert!(
        module.contains("AuthzModule"),
        "the http module imports AuthzModule: {module}"
    );

    let src = dir.path().join("crates/features/src");
    assert!(src.join("authn/strategy.rs").is_file());
    assert!(src.join("authz/ability.rs").is_file());
    assert!(src.join("authz/guard.rs").is_file());
    assert!(src.join("authn/claims.rs").is_file());

    // Built through the CLI's own mirror: a literal would assert the default
    // prefix rather than the generator.
    let var = scaffolded_var("authn", "SECRET");
    assert_eq!(
        assigned(&dir.path().join(".env"), &var),
        None,
        "the committed `.env`, read in every environment, assigns no signing secret",
    );
    for file in [".env.local", ".env.test"] {
        let secret = assigned(&dir.path().join(file), &var).expect("a secret of its own");
        assert_256_bit_hex(&secret);
        assert!(!printed.contains(&secret), "{printed}");
        assert!(
            printed.contains(&format!("`{file}` holds")),
            "the run says where the secret is:\n{printed}",
        );
    }
}

/// Run from the workspace root of a one-app workspace, `g resource` writes
/// every import its code needs into that app, in one edit: the edge, the data
/// layer, and the token route of the auth adapter it bootstrapped — and none
/// the adapters bring.
#[test]
fn generate_resource_from_the_root_wires_everything_its_code_needs() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");
    let module_rs = app.join("src/module.rs");

    let printed = run_ok(
        dir.path(),
        &["g", "resource", "posts", "-p", dir.path().to_str().unwrap()],
    );

    let module = fs::read_to_string(&module_rs).unwrap();
    for (use_path, ident) in [
        ("features::posts::PostsHttpModule", "PostsHttpModule"),
        ("nest_rs::seaorm::SeaOrmModule", "SeaOrmModule"),
        (
            "nest_rs::seaorm::SeaOrmDatabaseModule",
            "SeaOrmDatabaseModule",
        ),
        ("features::authn::AuthnHttpModule", "AuthnHttpModule"),
    ] {
        assert!(
            module.contains(&format!("use {use_path};"))
                && module.contains(&format!("        {ident},")),
            "the app imports {ident}: {module}"
        );
    }
    assert!(
        printed.contains(
            "PostsHttpModule is wired into `api`, beside SeaOrmModule, SeaOrmDatabaseModule \
             and AuthnHttpModule."
        ),
        "{printed}"
    );
    for ident in ["AuthnModule", "AuthzModule"] {
        assert!(
            !module.contains(&format!("{ident},")),
            "the adapter imports {ident}, not the root: {module}"
        );
    }
}

/// `g resource`'s entity declares the `created_at`/`updated_at`/`deleted_at`
/// columns `g migration` scaffolds.
#[test]
fn generate_resource_declares_the_columns_its_migration_scaffolds() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);

    let feature = dir.path().join("crates/features/src/posts");
    let entity = fs::read_to_string(feature.join("entity.rs")).unwrap();
    for flag in ["soft_delete", "timestamps"] {
        assert!(
            entity.contains(flag),
            "entity must declare `{flag}`: {entity}"
        );
    }
    for column in ["created_at", "updated_at", "deleted_at"] {
        assert!(
            entity.contains(column),
            "entity must carry `{column}`, which the migration creates: {entity}",
        );
    }
    assert!(
        !entity.contains("impl ActiveModelBehavior for ActiveModel {}"),
        "`timestamps` emits the behaviour — a second empty impl is a conflict: {entity}",
    );

    let service = fs::read_to_string(feature.join("service.rs")).unwrap();
    assert!(
        !service.contains("soft_delete"),
        "the entity's flag alone tombstones and hides deleted rows: {service}",
    );

    write_fake_migrations_crate(dir.path());
    run_ok(dir.path(), &["g", "migration", "create_posts", "-p", root]);
    let migrations = dir.path().join("crates/migrations/src");
    let generated = fs::read_dir(&migrations)
        .unwrap()
        .filter_map(Result::ok)
        .map(|e| e.path())
        .find(|p| {
            p.file_name()
                .is_some_and(|n| n.to_string_lossy().contains("create_posts"))
        })
        .expect("the migration file");
    let migration = fs::read_to_string(generated).unwrap();
    for column in ["CreatedAt", "UpdatedAt", "DeletedAt"] {
        assert!(migration.contains(column), "{migration}");
    }
}

/// The `nestrs_` prefix names the framework's tables, the outbox's among them:
/// a resource whose table would take it is refused before anything is written,
/// so its next step never names a migration `g migration` refuses.
#[test]
fn a_resource_whose_table_takes_the_nestrs_prefix_is_refused() {
    for name in ["nestrs_widget", "nestrs-outbox"] {
        let dir = tempfile::tempdir().unwrap();
        write_fake_workspace(dir.path());
        let before = files_under(dir.path());

        let refused = run_err(
            dir.path(),
            &["g", "resource", name, "-p", dir.path().to_str().unwrap()],
        );

        assert!(
            refused.contains("claims the `nestrs_` prefix, which names the framework's own tables"),
            "`{name}`: the refusal names the reserved prefix: {refused}"
        );
        assert!(
            files_under(dir.path()) == before,
            "`{name}`: nothing is written"
        );
    }
}

/// An app already importing the data layer — pinned, here — keeps its own
/// import: the generator writes only what the root does not name.
#[test]
fn generate_resource_leaves_a_pinned_data_layer_as_it_is() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");
    let module_rs = app.join("src/module.rs");
    let pinned = fs::read_to_string(&module_rs).unwrap().replace(
        "    ],",
        "        SeaOrmModule::for_root(database()),\n        SeaOrmDatabaseModule,\n    ],",
    );
    fs::write(&module_rs, pinned).unwrap();

    run_ok(&app, &["g", "resource", "posts"]);

    let module = fs::read_to_string(&module_rs).unwrap();
    assert_eq!(module.matches("SeaOrmModule").count(), 1, "{module}");
    assert_eq!(
        module.matches("SeaOrmDatabaseModule").count(),
        1,
        "{module}"
    );
    assert!(module.contains("        PostsHttpModule,"), "{module}");
}

/// With several apps and none named, the generator writes the feature and
/// wires nothing, listing the apps and every import to write; `--app` names
/// the one, and a name no app answers to is refused before anything is
/// written.
#[test]
fn generate_resource_in_a_workspace_of_several_apps_wires_the_named_one() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let hello = write_fake_app(dir.path(), "hello");
    let blog = write_fake_app(dir.path(), "blog");
    let root = dir.path().to_str().unwrap();
    let untouched = fs::read_to_string(hello.join("src/module.rs")).unwrap();

    let before = files_under(dir.path());
    let refused = run_err(
        dir.path(),
        &["g", "resource", "posts", "-p", root, "--app", "ghost"],
    );
    assert!(
        refused.contains("no app `ghost` in this workspace — its apps: blog, hello"),
        "{refused}"
    );
    assert_eq!(
        files_under(dir.path()),
        before,
        "a refused run writes nothing"
    );

    let printed = run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);
    assert!(
        printed.contains("Not wired: this workspace has several apps (blog, hello)"),
        "{printed}"
    );
    assert!(
        printed.contains(
            "Import `features::posts::PostsHttpModule`, `nest_rs::seaorm::SeaOrmModule`, \
             `nest_rs::seaorm::SeaOrmDatabaseModule` and `features::authn::AuthnHttpModule` \
             in the one that serves it"
        ),
        "{printed}"
    );
    for app in [&hello, &blog] {
        assert_eq!(
            fs::read_to_string(app.join("src/module.rs")).unwrap(),
            untouched,
            "no app is wired"
        );
    }

    run_ok(dir.path(), &["g", "feature", "notes", "-p", root]);
    let printed = run_ok(
        dir.path(),
        &["g", "http", "notes", "-p", root, "--app", "blog"],
    );
    assert!(
        printed.contains("NotesHttpModule is wired into `blog`."),
        "{printed}"
    );
    let composed = fs::read_to_string(blog.join("src/module.rs")).unwrap();
    assert!(composed.contains("        NotesHttpModule,"), "{composed}");
    assert_eq!(
        fs::read_to_string(hello.join("src/module.rs")).unwrap(),
        untouched,
        "only the named app is wired"
    );
}
