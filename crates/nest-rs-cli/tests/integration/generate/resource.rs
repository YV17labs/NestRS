//! `nestrs g resource` — the CRUD slice, its guarded HTTP form, and the columns
//! one list writes into its entity, its migration and its round trip.

use crate::harness::{
    assert_256_bit_hex, assigned, files_under, migration_named, run_err, run_ok, scaffolded_var,
    write_fake_app, write_fake_migrations_crate, write_fake_workspace,
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
        controller.contains("#[crud(service = PostsService)]"),
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

/// Unedited, the three files `g resource` writes about one table agree: the
/// entity's placeholder column is the one its migration creates and its round
/// trip posts, beside the house columns `timestamps` and `soft_delete` read.
#[test]
fn generate_resource_writes_the_migration_its_entity_and_round_trip_agree_with() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    write_fake_app(dir.path(), "blog");
    let root = dir.path().to_str().unwrap();

    let printed = run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);

    let feature = dir.path().join("crates/features/src/posts");
    let entity = fs::read_to_string(feature.join("entity.rs")).unwrap();
    for flag in ["soft_delete", "timestamps"] {
        assert!(
            entity.contains(flag),
            "entity must declare `{flag}`: {entity}"
        );
    }
    for column in [
        "pub name: String,",
        "created_at",
        "updated_at",
        "deleted_at",
    ] {
        assert!(
            entity.contains(column),
            "the entity carries `{column}`: {entity}"
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

    let (stem, migration) = migration_named(dir.path(), "create_posts");
    for line in [
        ".table(Post::Table)",
        ".col(ColumnDef::new(Post::Id).uuid().not_null().primary_key())",
        ".col(ColumnDef::new(Post::Name).string().not_null())",
        "ColumnDef::new(Post::CreatedAt)",
        "ColumnDef::new(Post::UpdatedAt)",
        "ColumnDef::new(Post::DeletedAt)",
        "    Name,\n",
    ] {
        assert!(migration.contains(line), "{line}: {migration}");
    }
    assert!(
        !migration.contains("TODO"),
        "nothing is left to fill: {migration}"
    );
    let lib = fs::read_to_string(dir.path().join("crates/migrations/src/lib.rs")).unwrap();
    assert!(lib.contains(&format!("mod {stem};")), "{lib}");
    let migrator =
        fs::read_to_string(dir.path().join("crates/migrations/src/migrator.rs")).unwrap();
    assert!(
        migrator.contains(&format!("Box::new({stem}::Migration)")),
        "{migrator}"
    );

    let test = fs::read_to_string(dir.path().join("apps/blog/tests/e2e/posts.rs")).unwrap();
    for line in [
        r#".body_json(&json!({ "name": "Name 1" }))"#,
        r#"assert_eq!(fields.get("name").string(), "Name 1");"#,
    ] {
        assert!(test.contains(line), "{line}: {test}");
    }
    assert!(
        !printed.contains("nestrs g migration"),
        "the migration is written, not a next step: {printed}"
    );
    assert!(
        printed.contains(&format!("crates/migrations/src/{stem}.rs")),
        "the run names the migration it wrote: {printed}"
    );
}

/// `g resource` takes its columns as `<name>:<type>[:nullable]`, and the one
/// list writes the entity's fields, the migration's columns and enum, and the
/// round trip's body and assertions.
#[test]
fn generate_resource_takes_its_columns_from_the_command_line() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    write_fake_app(dir.path(), "blog");
    let root = dir.path().to_str().unwrap();

    run_ok(
        dir.path(),
        &[
            "g",
            "resource",
            "posts",
            "title:string",
            "body:text",
            "views:integer",
            "bio:text:nullable",
            "-p",
            root,
        ],
    );

    let entity =
        fs::read_to_string(dir.path().join("crates/features/src/posts/entity.rs")).unwrap();
    for line in [
        "    #[expose(input(create, update), validate(length(min = 1)))]\n    pub title: String,\n",
        "    #[sea_orm(column_type = \"Text\")]\n    #[expose(input(create, update), validate(length(min = 1)))]\n    pub body: String,\n",
        "    #[expose(input(create, update))]\n    pub views: i32,\n",
        "    #[sea_orm(column_type = \"Text\")]\n    #[expose(input(create, update))]\n    pub bio: Option<String>,\n",
    ] {
        assert!(entity.contains(line), "{line}: {entity}");
    }
    assert!(
        !entity.contains("pub name:"),
        "no placeholder beside named columns: {entity}"
    );

    let (_, migration) = migration_named(dir.path(), "create_posts");
    for line in [
        ".col(ColumnDef::new(Post::Title).string().not_null())",
        ".col(ColumnDef::new(Post::Body).text().not_null())",
        ".col(ColumnDef::new(Post::Views).integer().not_null())",
        ".col(ColumnDef::new(Post::Bio).text().null())",
        "    Title,\n    Body,\n    Views,\n    Bio,\n",
    ] {
        assert!(migration.contains(line), "{line}: {migration}");
    }

    let test = fs::read_to_string(dir.path().join("apps/blog/tests/e2e/posts.rs")).unwrap();
    for line in [
        r#""title": "Title 1""#,
        r#""body": "Body 1""#,
        r#""views": 1"#,
        r#""bio": "Bio 1""#,
        r#"assert_eq!(fields.get("title").string(), "Title 1");"#,
        r#"assert_eq!(fields.get("views").i64(), 1);"#,
    ] {
        assert!(test.contains(line), "{line}: {test}");
    }
}

/// A column list the grammar does not read is refused before anything is
/// written, the refusal naming the grammar or the clash.
#[test]
fn a_column_the_grammar_does_not_read_is_refused() {
    for (columns, says) in [
        (&["title"][..], "`title` names no type"),
        (
            &["title:strin"],
            "string, text, integer, bigint, float, boolean, uuid, date, datetime",
        ),
        (&["title:string:null"], "`nullable`"),
        (&["Title:string"], "snake_case"),
        (&["type:string"], "keyword"),
        (&["created_at:datetime"], "every table"),
        (&["title:string", "title:text"], "twice"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        write_fake_workspace(dir.path());
        write_fake_migrations_crate(dir.path());
        let before = files_under(dir.path());
        let mut args = vec!["g", "resource", "posts"];
        args.extend_from_slice(columns);
        args.extend(["-p", dir.path().to_str().unwrap()]);

        let refused = run_err(dir.path(), &args);

        assert!(refused.contains(says), "{columns:?}: {says}: {refused}");
        assert_eq!(
            files_under(dir.path()),
            before,
            "{columns:?}: nothing is written"
        );
    }
}

/// The table `g resource` wrote a migration for is not created twice: a `g
/// migration` creating it, however spelled, is refused naming the file and
/// the table, while a later change to it is a migration of its own; and a
/// resource over a table a migration creates is refused with its own way
/// through, never `g migration`'s.
#[test]
fn a_table_a_registered_migration_creates_is_not_created_twice() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    let root = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);
    let (stem, _) = migration_named(dir.path(), "create_posts");
    let before = files_under(dir.path());

    for name in ["create_posts", "create_post", "create_posts_table"] {
        let refused = run_err(dir.path(), &["g", "migration", name, "-p", root]);
        for says in [
            format!("crates/migrations/src/{stem}.rs"),
            "the table `post`".to_owned(),
            "`add_<column>_to_post`".to_owned(),
        ] {
            assert!(refused.contains(&says), "{name}: {says}: {refused}");
        }
        assert_eq!(
            files_under(dir.path()),
            before,
            "{name}: nothing is written"
        );
    }

    run_ok(
        dir.path(),
        &["g", "migration", "add_body_to_posts", "-p", root],
    );
    let refused = run_err(
        dir.path(),
        &["g", "migration", "add_body_to_posts", "-p", root],
    );
    assert!(
        refused.contains("a migration named `add_body_to_posts` is registered already"),
        "{refused}"
    );
    run_ok(
        dir.path(),
        &["g", "migration", "create_index_on_posts", "-p", root],
    );
    let (_, index) = migration_named(dir.path(), "create_index_on_posts");
    assert!(
        index.contains("TODO") && !index.contains("Post::Name"),
        "an index is no create-table of the entity's columns: {index}"
    );

    fs::remove_dir_all(dir.path().join("crates/features/src/posts")).unwrap();
    let before = files_under(dir.path());
    let refused = run_err(dir.path(), &["g", "resource", "posts", "-p", root]);
    for says in [
        format!("crates/migrations/src/{stem}.rs"),
        "delete that file and its two registrations".to_owned(),
        "`nestrs g entity posts`".to_owned(),
    ] {
        assert!(refused.contains(&says), "{says}: {refused}");
    }
    assert!(
        !refused.contains("add_<column>"),
        "`g migration`'s remedy is not the resource's: {refused}"
    );
    assert_eq!(files_under(dir.path()), before, "nothing is written");
}

/// A workspace without the migrations crate gets it, with the resource's
/// migration registered: `g resource` takes the step `g migration` would.
#[test]
fn generate_resource_bootstraps_the_migrations_crate_when_absent() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let root = dir.path().to_str().unwrap();

    run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);

    let (stem, _) = migration_named(dir.path(), "create_posts");
    let lib = fs::read_to_string(dir.path().join("crates/migrations/src/lib.rs")).unwrap();
    assert!(lib.contains(&format!("mod {stem};")), "{lib}");
    assert!(
        dir.path()
            .join("crates/migrations/src/bin/migrate.rs")
            .is_file()
    );
    let manifest = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    assert!(manifest.contains("sea-orm-migration"), "{manifest}");
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

/// Wired into an app, `g resource` writes the resource's round trip into the
/// app's `e2e` suite: the test, the suite root's fixture booting the module
/// `main` runs on a throwaway database, and the dev-dependencies they name. A
/// second resource joins the same root.
#[test]
fn generate_resource_writes_the_round_trip_test_of_the_app_it_wires() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    let app = write_fake_app(dir.path(), "blog");
    let root = dir.path().to_str().unwrap();

    let printed = run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);

    let suite = app.join("tests/e2e");
    let test = fs::read_to_string(suite.join("posts.rs")).unwrap();
    for line in [
        "use crate::boot;",
        "async fn posts_round_trip() {",
        ".post(\"/posts\")",
        "let bearer = app.bearer(&Claims {",
        "created.assert_status(StatusCode::CREATED);",
    ] {
        assert!(test.contains(line), "{line}: {test}");
    }
    let main = fs::read_to_string(suite.join("main.rs")).unwrap();
    for line in [
        "mod posts;",
        "use blog::BlogModule;",
        ".module::<BlogModule>()",
        ".ephemeral_database::<migrations::Migrator>()",
    ] {
        assert!(main.contains(line), "{line}: {main}");
    }
    assert!(
        !main.contains("#[tokio::test]"),
        "the root holds no test: {main}"
    );
    let app_cargo = fs::read_to_string(app.join("Cargo.toml")).unwrap();
    let dev = app_cargo
        .split_once("[dev-dependencies]")
        .map(|(_, dev)| dev)
        .unwrap_or_else(|| panic!("a dev-dependencies table: {app_cargo}"));
    for dep in ["migrations", "serde_json", "uuid"] {
        assert!(dev.contains(dep), "{dep}: {app_cargo}");
    }
    assert!(
        fs::read_to_string(dir.path().join("Cargo.toml"))
            .unwrap()
            .contains("serde_json = \"1.0\""),
        "the workspace pins what the app's suite names"
    );
    assert!(
        printed.contains("Its round trip is `tests/e2e/posts.rs` in that app"),
        "{printed}"
    );

    run_ok(dir.path(), &["g", "resource", "orders", "-p", root]);
    let main = fs::read_to_string(suite.join("main.rs")).unwrap();
    assert!(
        main.contains("mod orders;") && main.contains("mod posts;"),
        "{main}"
    );
    assert_eq!(main.matches("fn boot(").count(), 1, "one fixture: {main}");
    assert!(suite.join("orders.rs").is_file());
}

/// A resource no app serves gets no round-trip test: there is no composition
/// to boot.
#[test]
fn generate_resource_wired_nowhere_writes_no_round_trip_test() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_fake_migrations_crate(dir.path());
    let root = dir.path().to_str().unwrap();

    let printed = run_ok(dir.path(), &["g", "resource", "posts", "-p", root]);

    assert!(!printed.contains("round trip"), "{printed}");
    assert!(
        files_under(dir.path())
            .keys()
            .all(|path| !path.starts_with("apps")),
        "no app, no suite written"
    );
}
