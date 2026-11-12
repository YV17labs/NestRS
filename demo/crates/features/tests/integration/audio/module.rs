use std::path::Path;

use features::testing::SUITES_BUCKET;
use nest_rs::config::{Config, Environment, load_cascade};
use nest_rs::storage::StorageConfig;
use nest_rs::testing::load_project_env;

fn bucket() -> String {
    let url = StorageConfig::load()
        .expect("the store's config parses")
        .url;
    url.strip_prefix("s3://")
        .and_then(|rest| rest.split('/').next())
        .expect("the audio feature files into an s3:// store")
        .to_owned()
}

#[test]
fn development_opens_no_bucket_the_suites_file_into() {
    load_cascade(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        Environment::Development,
    );

    assert_ne!(
        bucket(),
        SUITES_BUCKET,
        "development reads the bucket the e2e suites file into"
    );
}

#[test]
fn the_suites_file_their_objects_into_a_bucket_of_their_own() {
    load_project_env();

    assert_eq!(
        bucket(),
        SUITES_BUCKET,
        "the e2e suites file their objects into the bucket a running app reads"
    );
}
