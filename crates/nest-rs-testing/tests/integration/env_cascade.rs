//! Whichever harness entry point runs first, `load_project_env` decides the
//! environment before any cascade read.

use nest_rs_config::Environment;
use nest_rs_testing::load_project_env;

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the test asserts what the harness wrote into the process environment"
)]
fn first_cascade_load_defaults_nestrs_env_to_test() {
    let pre = std::env::var_os(Environment::var_name());

    load_project_env();

    match pre {
        None => assert_eq!(
            std::env::var(Environment::var_name()).as_deref(),
            Ok("test"),
            "load_project_env must default {}=test before reading the cascade",
            Environment::var_name(),
        ),
        Some(explicit) => assert_eq!(
            std::env::var_os(Environment::var_name()),
            Some(explicit),
            "an explicit {} must win over the harness default",
            Environment::var_name(),
        ),
    }

    let decided = std::env::var_os(Environment::var_name());
    load_project_env();
    assert_eq!(std::env::var_os(Environment::var_name()), decided);
}
