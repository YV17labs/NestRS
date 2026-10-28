//! Covers `src/app_dir.rs` — the app crate's directory `#[nest_rs::main]`
//! captures where it expands, under which a development artifact lands.

use std::path::{Path, PathBuf};

use nest_rs_core::__private::in_app_dir;

#[nest_rs_core::main]
async fn where_the_schema_lands() -> PathBuf {
    in_app_dir(Path::new("schema.graphql"))
}

#[test]
fn a_relative_artifact_lands_in_the_crate_main_expanded_in() {
    assert_eq!(
        in_app_dir(Path::new("schema.graphql")),
        Path::new("schema.graphql"),
        "before any `main`, the working directory's",
    );
    assert_eq!(
        where_the_schema_lands(),
        Path::new(env!("CARGO_MANIFEST_DIR")).join("schema.graphql"),
        "under `main`, the directory of the crate it expanded in",
    );
    let absolute = std::env::temp_dir().join("schema.graphql");
    assert_eq!(
        in_app_dir(&absolute),
        absolute,
        "an absolute path is written as given"
    );
}
