use features::audio::AudioConfig;
use nest_rs::config::Config;
use nest_rs::testing::load_project_env;

#[test]
fn the_struct_default_seeds_nothing() {
    assert!(!AudioConfig::default().synthetic_seed);
}

#[test]
fn the_suites_profile_seeds_the_synthetic_jobs() {
    load_project_env();

    assert!(AudioConfig::defaults().synthetic_seed);
}
