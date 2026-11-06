//! `nestrs g entity` — one entity in an existing feature: where it lands, what
//! it deliberately does not name, and the layout it refuses to make ambiguous.

use crate::harness::{files_under, run_err, run_ok, write_fake_workspace};
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
    assert!(feature.join("entity.rs").is_file());
    assert!(!feature.join("entities").exists());

    let entity = fs::read_to_string(feature.join("entity.rs")).unwrap();
    assert!(entity.contains("#[expose("), "{entity}");
    assert!(entity.contains("name = \"Post\""), "{entity}");
    assert!(entity.contains("table_name = \"post\""), "{entity}");
    assert!(entity.contains("DeriveEntityModel"), "{entity}");

    let mod_rs = fs::read_to_string(feature.join("mod.rs")).unwrap();
    assert!(mod_rs.contains("mod entity;"), "{mod_rs}");
    assert!(mod_rs.contains("pub use entity::*;"), "{mod_rs}");

    // `seaorm` is the single feature behind `#[expose]`.
    let root_cargo = fs::read_to_string(dir.path().join("Cargo.toml")).unwrap();
    let features_cargo = fs::read_to_string(dir.path().join("crates/features/Cargo.toml")).unwrap();
    assert!(features_cargo.contains("\"seaorm\""), "{features_cargo}");
    assert!(root_cargo.contains("\"seaorm\""), "{root_cargo}");
    assert!(root_cargo.contains("sea-orm"), "{root_cargo}");
    assert!(features_cargo.contains("sea-orm"), "{features_cargo}");
    assert!(
        !features_cargo.contains("serde") && !root_cargo.contains("serde"),
        "`#[expose]` carries the round trip a mask needs, so an entity derives no serde: \
         {root_cargo}\n{features_cargo}",
    );
    assert!(
        !features_cargo.contains("authz"),
        "an entity alone needs no authz feature: {features_cargo}",
    );
    assert!(
        !features_cargo.contains("\"http\"") && !root_cargo.contains("\"http\""),
        "an entity alone serves nothing over HTTP: {root_cargo}\n{features_cargo}",
    );
}

/// `#[expose(service = …)]` is left out: `g entity` writes no service, and a
/// plain port's service is no `CrudService`.
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
        stdout.contains("ab.can(nest_rs::authz::Action::Manage, crate::posts::Entity);"),
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
    assert!(feature.join("entities/publication.rs").is_file());
    assert!(!feature.join("entity.rs").exists());

    let index = fs::read_to_string(feature.join("entities/mod.rs")).unwrap();
    assert!(index.contains("pub mod publication;"), "{index}");
    assert!(
        index.contains("pub mod post;"),
        "the first one survives: {index}"
    );

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

/// A module holds either one `entity.rs` or an `entities/` folder, so a second
/// entity moves the first into `entities/`, in the same run: its `super::`
/// paths gain the level it gains, and every path naming it follows.
#[test]
fn generate_entity_moves_the_lone_entity_beside_the_second() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "resource", "posts", "-p", path]);

    let printed = run_ok(dir.path(), &["g", "entity", "posts/comments", "-p", path]);

    let feature = dir.path().join("crates/features/src/posts");
    let read = |file: &str| fs::read_to_string(feature.join(file)).unwrap();
    assert!(!feature.join("entity.rs").exists());
    let moved = read("entities/post.rs");
    assert!(
        moved.contains("service = super::super::service::PostsService"),
        "the moved entity reaches its service one level up: {moved}"
    );
    let entities = read("entities/mod.rs");
    for line in ["pub mod post;", "pub mod comment;"] {
        assert!(entities.contains(line), "{line}: {entities}");
    }
    assert!(read("entities/comment.rs").contains("name = \"Comment\""));
    let index = read("mod.rs");
    for line in [
        "mod entities;",
        "pub use entities::post::*;",
        "pub use entities::comment;",
    ] {
        assert!(index.contains(line), "{line}: {index}");
    }
    assert!(!index.contains("mod entity;"), "{index}");
    assert!(
        read("service.rs").contains("use super::entities::post::{"),
        "{}",
        read("service.rs")
    );
    assert!(
        printed.contains(
            "> crates/features/src/posts/entity.rs → crates/features/src/posts/entities/post.rs"
        ),
        "the run reports the move: {printed}"
    );
}

/// A reference the move cannot rewrite with certainty — the module imported
/// by name — refuses the run, naming its file and line, and writes nothing.
#[test]
fn generate_entity_refuses_a_move_it_cannot_rewrite_with_certainty() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "entity", "posts/articles", "-p", path]);
    let service = dir.path().join("crates/features/src/posts/service.rs");
    let imported = format!(
        "use super::entity;\n{}",
        fs::read_to_string(&service).unwrap()
    );
    fs::write(&service, imported).unwrap();
    let before = files_under(dir.path());

    let refused = run_err(dir.path(), &["g", "entity", "posts/comments", "-p", path]);

    assert!(
        refused.contains("crates/features/src/posts/service.rs:1")
            && refused.contains("an import of the `entity` module itself")
            && refused.contains("entities/article.rs"),
        "the refusal names the reference and where the entity was going: {refused}"
    );
    assert_eq!(files_under(dir.path()), before, "a refusal writes nothing");
}

/// `g graphql|mcp` over a port with no entity wrote its `#[subject]` under the
/// port's singular, the name the entity's wire type takes. `g entity` retires
/// it as written: the file and its two lines in `mod.rs` go, each operation
/// is gated on the entity instead, and the grant is printed, never written —
/// `ability.rs` is the developer's.
#[test]
fn generate_entity_retires_the_subject_the_generators_wrote() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "graphql", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "mcp", "posts", "-p", path]);
    let feature = dir.path().join("crates/features/src/posts");
    let read = |file: &str| fs::read_to_string(feature.join(file)).unwrap();
    let ability = dir.path().join("crates/features/src/authz/ability.rs");
    let policy = fs::read_to_string(&ability).unwrap();
    assert!(feature.join("subject.rs").is_file());
    assert!(read("graphql/resolver.rs").contains("#[authorize(Read, Post)]"));

    let printed = run_ok(dir.path(), &["g", "entity", "posts", "-p", path]);

    assert!(
        !feature.join("subject.rs").exists(),
        "the subject is retired"
    );
    let index = read("mod.rs");
    for gone in ["mod subject;", "pub use subject::Post;"] {
        assert!(!index.contains(gone), "{gone}: {index}");
    }
    for line in ["mod entity;", "pub use entity::*;"] {
        assert!(index.contains(line), "{line}: {index}");
    }
    for file in ["graphql/resolver.rs", "mcp/tool.rs"] {
        let handler = read(file);
        assert!(
            handler.contains("#[authorize(Read, PostEntity)]")
                && handler.contains("Entity as PostEntity")
                && !handler.contains("#[authorize(Read, Post)]"),
            "{file} is gated on the entity: {handler}"
        );
    }
    assert_eq!(
        fs::read_to_string(&ability).unwrap(),
        policy,
        "the policy is the developer's: nothing granted"
    );
    assert!(
        printed.contains("ab.can(nest_rs::authz::Action::Read, crate::posts::Entity);")
            && printed.contains("crate::posts::Post"),
        "the run prints the grant that replaces the subject's: {printed}"
    );
}

/// A `subject.rs` that does not parse says nothing of the name it holds, so no entity
/// is written over a guess: the run names the file and its line.
#[test]
fn generate_entity_refuses_a_subject_that_does_not_parse() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "mcp", "posts", "-p", path]);
    let subject = dir.path().join("crates/features/src/posts/subject.rs");
    let source = fs::read_to_string(&subject).unwrap();
    let line = source.lines().count() + 1;
    fs::write(&subject, format!("{source}fn half(\n")).unwrap();
    let before = files_under(dir.path());

    let refused = run_err(dir.path(), &["g", "entity", "posts", "-p", path]);

    assert!(
        refused.contains("subject.rs") && refused.contains(&format!("line {line}")),
        "line {line}: {refused}"
    );
    assert_eq!(files_under(dir.path()), before, "a refusal writes nothing");
}

/// A subject or a posture edited beyond what the generator wrote is the
/// developer's: `g entity` refuses, naming the file, and writes nothing. An
/// entity of another name joins the port beside its subject.
#[test]
fn generate_entity_refuses_a_subject_edited_beyond_the_template() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "posts", "-p", path]);
    run_ok(dir.path(), &["g", "mcp", "posts", "-p", path]);
    let tool = dir.path().join("crates/features/src/posts/mcp/tool.rs");
    let edited = fs::read_to_string(&tool)
        .unwrap()
        .replace("#[authorize(Read, Post)]", "#[authorize(Update, Post)]");
    fs::write(&tool, edited).unwrap();
    let before = files_under(dir.path());

    let refused = run_err(dir.path(), &["g", "entity", "posts", "-p", path]);

    assert!(
        refused.contains("crates/features/src/posts/mcp/tool.rs") && refused.contains("`Post`"),
        "the refusal names the file edited beyond the template: {refused}"
    );
    assert_eq!(files_under(dir.path()), before, "a refusal writes nothing");

    run_ok(dir.path(), &["g", "entity", "posts/comments", "-p", path]);
    let entity =
        fs::read_to_string(dir.path().join("crates/features/src/posts/entity.rs")).unwrap();
    assert!(entity.contains("name = \"Comment\""), "{entity}");
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

/// The `nestrs_` prefix names the framework's tables: an entity whose table
/// would take it, named after the slash or after its feature, is refused
/// before anything is written.
#[test]
fn an_entity_whose_table_takes_the_nestrs_prefix_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    write_fake_workspace(dir.path());
    let path = dir.path().to_str().unwrap();
    run_ok(dir.path(), &["g", "feature", "shelf", "-p", path]);
    run_ok(dir.path(), &["g", "feature", "nestrs_crates", "-p", path]);
    let before = files_under(dir.path());

    for target in ["shelf/nestrs_crate", "shelf/nestrs_outbox", "nestrs_crates"] {
        let refused = run_err(dir.path(), &["g", "entity", target, "-p", path]);

        assert!(
            refused.contains("claims the `nestrs_` prefix, which names the framework's own tables"),
            "`{target}`: the refusal names the reserved prefix: {refused}"
        );
        assert!(
            files_under(dir.path()) == before,
            "`{target}`: nothing is written"
        );
    }
}
