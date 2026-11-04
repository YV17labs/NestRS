use std::process::Command;

use features::audio::AudioConfig;
use nest_rs::config::{Environment, Surface};
use nest_rs::testing::load_project_env;

const PROBE: &str = "audio::config::the_seed_follows_whether_this_process_is_deployed";

#[test]
fn the_default_leaves_the_seed_to_the_profile() {
    assert_eq!(AudioConfig::default().synthetic_seed, Surface::ByProfile);
}

#[test]
fn the_suites_profile_seeds_the_synthetic_jobs() {
    load_project_env();

    assert!(AudioConfig::default().synthetic_seed.is_open());
}

#[test]
fn the_seed_follows_whether_this_process_is_deployed() {
    assert_eq!(
        AudioConfig::default().synthetic_seed.is_open(),
        !Environment::from_env().is_deployed()
    );
}

#[test]
fn a_deployment_and_an_undeclared_process_seed_nothing() {
    for profile in [Some("production"), Some("staging"), None] {
        let mut probe = Command::new(std::env::current_exe().expect("the suite's own binary"));
        probe.args(["--exact", PROBE]);
        match profile {
            Some(profile) => probe.env(Environment::var_name(), profile),
            None => probe.env_remove(Environment::var_name()),
        };

        let ran = probe
            .output()
            .expect("the probe runs in a process of its own");
        let said = String::from_utf8_lossy(&ran.stdout);
        assert!(
            ran.status.success() && said.contains("1 passed"),
            "{profile:?} seeds: {said}"
        );
    }
}
