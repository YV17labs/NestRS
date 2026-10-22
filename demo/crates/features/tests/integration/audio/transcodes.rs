use std::path::Path;

use features::audio::Transcodes;
use features::testing::RedisDatabase;
use nest_rs::config::{
    Config, ConfigService, Environment, Namespaced, instance_namespace, load_cascade, read,
};
use nest_rs::redis::RedisConfig;
use nest_rs::testing::load_project_env;

fn transcodes() -> RedisConfig {
    read(
        &ConfigService::for_namespace(&instance_namespace::<Transcodes>(RedisConfig::NAMESPACE)),
        RedisConfig::defaults(),
    )
    .expect("the Transcodes instance's Redis config parses")
}

#[test]
fn development_files_the_transcodes_on_no_database_a_suite_holds() {
    load_cascade(
        Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")),
        Environment::Development,
    );

    assert_eq!(
        RedisDatabase::held(&transcodes().url),
        None,
        "development files the transcode records on a database an e2e suite writes to"
    );
}

#[test]
fn the_suites_file_the_transcodes_on_a_database_of_their_own_apart_from_the_queue() {
    load_project_env();
    let queue = RedisConfig::load().expect("the suites' Redis config parses");

    assert_eq!(
        (
            RedisDatabase::held(&queue.url),
            RedisDatabase::held(&transcodes().url)
        ),
        (Some(RedisDatabase::SUITES), Some(RedisDatabase::TRANSCODES)),
        "the e2e suites file the queue and the transcode records on databases of their own"
    );
}
