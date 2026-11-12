use features::audio::{AudioConfig, SEED_OPEN, TARGET};
use features::testing::assert_passes_under_profile;
use nest_rs::config::{Environment, Surface};
use nest_rs::testing::{LogCapture, load_project_env};

const PROBE: &str = "audio::config::the_seed_follows_whether_this_process_is_deployed";

const OPENING_PROBE: &str = "audio::config::an_opened_seed_says_itself_where_it_is_deployed";

#[test]
fn the_default_leaves_the_seed_to_the_profile() {
    assert_eq!(AudioConfig::default().synthetic_seed, Surface::ByProfile);
}

#[test]
fn the_suites_profile_seeds_the_synthetic_jobs() {
    load_project_env();

    assert!(AudioConfig::default().seeds());
}

#[test]
fn the_seed_follows_whether_this_process_is_deployed() {
    assert_eq!(
        AudioConfig::default().seeds(),
        !Environment::from_env().is_deployed()
    );
}

#[test]
fn a_deployment_and_an_undeclared_process_seed_nothing() {
    for profile in [Some("production"), Some("staging"), None] {
        assert_passes_under_profile(PROBE, profile);
    }
}

#[test]
fn an_opened_seed_says_itself_where_it_is_deployed() {
    let logs = LogCapture::install();
    let opened = AudioConfig {
        synthetic_seed: Surface::Open,
    };

    assert!(opened.seeds());
    assert!(opened.seeds());

    let said = logs.find(TARGET, SEED_OPEN);
    assert_eq!(
        said.len(),
        usize::from(Environment::from_env().is_deployed())
    );
    assert!(said.iter().all(|line| line.level == "warn"));
}

#[test]
fn a_deployment_opening_the_seed_is_told_at_warn() {
    for profile in [
        Some("production"),
        Some("staging"),
        Some("development"),
        None,
    ] {
        assert_passes_under_profile(OPENING_PROBE, profile);
    }
}
