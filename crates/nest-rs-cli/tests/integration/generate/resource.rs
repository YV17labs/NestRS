//! `nestrs g resource` — the CRUD slice, its guarded HTTP form, and the columns
//! it has to declare in lockstep with `g migration`.

use crate::harness::{
    run_ok, scaffolded_var, write_fake_app, write_fake_migrations_crate, write_fake_workspace,
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

    let lib = fs::read_to_string(dir.path().join("crates/features/src/lib.rs")).unwrap();
    assert!(lib.contains("pub mod posts;"));
}

#[test]
fn generate_resource_emits_the_guarded_form_and_bootstraps_auth() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    run_ok(
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
        controller.contains("#[crud(") && controller.contains("entity = PostEntity"),
        "the resource controller uses the #[crud] form: {controller}"
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

    let env = fs::read_to_string(dir.path().join(".env")).unwrap();
    // Built through the CLI's own mirror: a literal would assert the default
    // prefix rather than the generator.
    let secret = scaffolded_var("authn", "SECRET");
    assert!(env.contains(&secret), "the .env must name {secret}: {env}");
}

// `g resource` bootstrapping the adapter owes the composition site the same
// entries.
#[test]
fn generate_resource_wires_the_auth_roots_it_bootstrapped() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let app = write_fake_app(dir.path(), "api");
    let module_rs = app.join("src/module.rs");
    let with_db = fs::read_to_string(&module_rs).unwrap().replace(
        "    ],",
        "        SeaOrmModule::for_root(None),\n        SeaOrmDatabaseModule,\n    ],",
    );
    fs::write(&module_rs, with_db).unwrap();

    run_ok(&app, &["g", "resource", "posts"]);

    let module = fs::read_to_string(&module_rs).unwrap();
    for ident in ["PostsHttpModule", "AuthnModule", "AuthzModule"] {
        assert!(module.contains(&format!("{ident},")), "{ident}: {module}");
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
        service.contains("fn soft_delete_column()"),
        "the flag makes the column addressable; the override is what tombstones: {service}",
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
