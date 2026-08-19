//! `load_project_env` owns the whole env invariant (M7 regression): whichever
//! harness entry point runs first, the environment is decided *before* any
//! cascade read. nextest runs one process per test, so calling the loader as
//! the first gesture here reproduces the db-first harness path
//! (`EphemeralDatabase::create` before `TestApp::builder`) exactly.

use nest_rs_config::Environment;
use nest_rs_testing::load_project_env;

#[test]
fn first_cascade_load_defaults_nestrs_env_to_test() {
    // Simulate the db-first entry: the loader is this process's very first
    // gesture. An explicit NESTRS_ENV from the outer shell must win
    // (set-if-absent); otherwise the loader must decide `test`.
    let pre = std::env::var_os(Environment::var_name());

    load_project_env();

    match pre {
        // The default happened inside the `Once`, before the `.env` lookup —
        // `.env.local` is skipped and `.env.test.local` participates on every
        // later read of the cascade.
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

    // A later entry point (TestAppBuilder::new) re-invokes the loader; the
    // consumed `Once` must leave the decision unchanged.
    let decided = std::env::var_os(Environment::var_name());
    load_project_env();
    assert_eq!(std::env::var_os(Environment::var_name()), decided);
}
