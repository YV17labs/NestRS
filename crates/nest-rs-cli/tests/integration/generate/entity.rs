//! `nestrs g entity` — one entity in an existing feature: where it lands, what
//! it deliberately does not name, and the layout it refuses to make ambiguous.

use crate::harness::{run_ok, write_fake_workspace};
use std::fs;
use std::process::Command;

/// A `g feature` port whose entities already live in `entities/` — the shape
/// `demo/crates/features/src/users/` carries, and the one a second entity joins.
fn write_feature_with_entities_folder(root: &std::path::Path, feature: &str) {
    run_ok(
        root,
        &["g", "feature", feature, "-p", root.to_str().unwrap()],
    );
    let dir = root.join("crates/features/src").join(feature);
    fs::create_dir_all(dir.join("entities")).unwrap();
    fs::write(dir.join("entities/mod.rs"), "pub mod post;\n").unwrap();
    fs::write(dir.join("entities/post.rs"), "// the first entity\n").unwrap();
    let mod_rs = fs::read_to_string(dir.join("mod.rs"))
        .unwrap()
        .replace("mod module;", "mod entities;\nmod module;")
        .replace(
            "pub use module::",
            "pub use entities::post::*;\npub use module::",
        );
    fs::write(dir.join("mod.rs"), mod_rs).unwrap();
}

#[test]
fn generate_entity_writes_the_lone_entity_and_wires_the_port() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);

    run_ok(dir.path(), &["g", "entity", "posts", "-p", path]);

    let feature = dir.path().join("crates/features/src/posts");
    // A feature's first entity is the lone `entity.rs`, never `entities/` — one
    // role, one file per folder.
    assert!(feature.join("entity.rs").is_file());
    assert!(!feature.join("entities").exists());

    let entity = fs::read_to_string(feature.join("entity.rs")).unwrap();
    assert!(entity.contains("#[expose("), "{entity}");
    assert!(entity.contains("name = \"Post\""), "{entity}");
    assert!(entity.contains("table_name = \"post\""), "{entity}");
    assert!(entity.contains("DeriveEntityModel"), "{entity}");

    // The port's index gains both halves in one edit.
    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("mod entity;"), "{mod_rs}");
    assert!(mod_rs.contains("pub use entity::*;"), "{mod_rs}");

    // Everything the entity's own source names, in both manifests. `seaorm` is
    // the single feature behind `#[expose]`: it activates `nest-rs-resource` and
    // `nest-rs-seaorm` together, because each half's expansion names the other's
    // crate and two features implying each other is a cycle Cargo rejects.
    let root_cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(features_cargo.contains("\"seaorm\""), "{features_cargo}");
    assert!(root_cargo.contains("\"seaorm\""), "{root_cargo}");
    for krate in ["sea-orm", "serde"] {
        assert!(root_cargo.contains(krate), "{root_cargo}");
        assert!(features_cargo.contains(krate), "{features_cargo}");
    }
    // `authz` belongs to `#[crud]`, which a bare entity has no part of.
    assert!(
        !features_cargo.contains("authz"),
        "an entity alone needs no authz feature: {features_cargo}",
    );
}

/// The omission is the design: `#[expose(service = …)]` names the one
/// `CrudService` whose `type Entity` is this entity, `g entity` writes no
/// service, and a plain port's service is not a `CrudService` at all — naming it
/// fails inside the macro expansion, which no text assertion would ever see. So
/// the file names none and the printed steps say why.
#[test]
fn generate_entity_names_no_service_it_could_not_name_truthfully() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);

    let stdout = run_ok(dir.path(), &["g", "entity", "posts", "-p", path]);

    let entity =
        fs::read_to_string(dir.path().join("crates/features/src/posts/entity.rs")).unwrap();
    assert!(
        !entity.contains("service ="),
        "the entity names no service: {entity}",
    );
    assert!(
        stdout.contains("service = super::service::PostsService"),
        "…and the next steps name the link to add: {stdout}",
    );
    assert!(
        stdout.contains("nestrs g migration create_posts"),
        "…plus the migration for the columns it declares: {stdout}",
    );
    assert!(
        stdout.contains("ab.can(Action::Manage, posts_entity::Entity);"),
        "…plus the ability grant its reads need: {stdout}",
    );
}

#[test]
fn generate_entity_takes_its_own_name_after_the_slash() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);

    run_ok(dir.path(), &["g", "entity", "posts/comments", "-p", path]);

    let entity =
        fs::read_to_string(dir.path().join("crates/features/src/posts/entity.rs")).unwrap();
    assert!(entity.contains("name = \"Comment\""), "{entity}");
    assert!(entity.contains("table_name = \"comment\""), "{entity}");
}

#[test]
fn generate_entity_joins_a_feature_that_already_keeps_entities_in_a_folder() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    write_feature_with_entities_folder(dir.path(), "posts");

    run_ok(
        dir.path(),
        &[
            "g",
            "entity",
            "posts/publications",
            "-p",
            dir.path().to_str().unwrap(),
        ],
    );

    let feature = dir.path().join("crates/features/src/posts");
    // Bare, singular, snake — beside `post.rs`, exactly as `entities/user.rs`
    // and `entities/user_identity.rs` sit together in the exemplar.
    assert!(feature.join("entities/publication.rs").is_file());
    assert!(!feature.join("entity.rs").exists());

    let index = fs::read_to_string(feature.join("entities/mod.rs")).unwrap();
    assert!(index.contains("pub mod publication;"), "{index}");
    assert!(
        index.contains("pub mod post;"),
        "the first one survives: {index}"
    );

    // The module, not a glob: two entities re-exported flat would collide on
    // `Entity`, `Model` and `Column`.
    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(
        mod_rs.contains("pub use entities::publication;"),
        "{mod_rs}"
    );
    assert_eq!(
        mod_rs.matches("mod entities;").count(),
        1,
        "the module is declared once, not once per entity: {mod_rs}",
    );
}

/// A module holds either one `entity.rs` or an `entities/` folder — writing the
/// second entity beside the first would leave two homes for one role, and moving
/// the first is a refactor of files the developer has edited. So the generator
/// refuses, and the refusal has to name the move rather than just decline.
#[test]
fn generate_entity_refuses_to_leave_a_feature_with_two_entity_homes() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "entity", "posts/articles", "-p", path]);

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "entity", "posts/comments", "-p", path])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("entities"), "{stderr}");
    // The remedy names the *existing* entity's file, read from its own
    // `table_name` — the feature's singular would have said `post.rs` here.
    assert!(
        stderr.contains("entities/article.rs"),
        "the remedy names the file the existing entity moves to: {stderr}",
    );
    // Nothing was written: the refusal happens before the transaction.
    assert!(
        !dir.path()
            .join("crates/features/src/posts/entities")
            .exists()
    );
}

#[test]
fn generate_entity_requires_an_existing_feature() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "entity", "ghost", "-p", dir.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("not found"), "{stderr}");
    assert!(
        stderr.contains("nestrs g feature ghost"),
        "the diagnostic names the remedy: {stderr}",
    );
}

#[test]
fn generate_entity_requires_a_workspace() {
    let dir = tempfile::tempdir().unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_nestrs"))
        .args(["g", "entity", "posts", "-p", dir.path().to_str().unwrap()])
        .output()
        .unwrap();

    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("not inside a nestrs workspace"),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn generate_entity_dry_run_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);

    let stdout = run_ok(
        dir.path(),
        &["g", "entity", "posts", "--dry-run", "-p", path],
    );

    assert!(stdout.contains("Dry run — no files written."), "{stdout}");
    assert!(
        !dir.path()
            .join("crates/features/src/posts/entity.rs")
            .exists()
    );
    let mod_rs = fs::read_to_string(dir.path().join("crates/features/src/posts/mod.rs")).unwrap();
    assert!(!mod_rs.contains("mod entity;"), "{mod_rs}");
}
