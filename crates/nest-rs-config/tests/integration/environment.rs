//! `<PREFIX>_ENV` read off the process: what an unrecognised value says, in a
//! child process, since the warning goes to stderr before any subscriber exists;
//! and the remedy an app reading no config is told when a provider injects it.

use std::sync::Arc;

use nest_rs_config::Environment;
use nest_rs_core::{App, MissingDependencyError, injectable, module};

const CHILD_TEST: &str = "environment::unrecognised_child_process";

const UNRECOGNISED: &str = "prd-with-a-private-word";

#[test]
fn unrecognised_child_process() {
    if crate::is_child() {
        for _ in 0..2 {
            println!("ENVIRONMENT={}", Environment::from_env().as_str());
        }
    }
}

#[test]
fn an_unrecognised_value_is_read_as_production_and_named_by_its_variable_alone() {
    let child = crate::child(CHILD_TEST)
        .env(Environment::var_name(), UNRECOGNISED)
        .output()
        .expect("the child runs");

    let (stdout, stderr) = (
        String::from_utf8_lossy(&child.stdout),
        String::from_utf8_lossy(&child.stderr),
    );
    assert!(child.status.success(), "{stdout}{stderr}");
    assert_eq!(
        stdout
            .lines()
            .filter(|line| *line == "ENVIRONMENT=production")
            .count(),
        2,
        "{stdout}"
    );
    let warnings: Vec<&str> = stderr
        .lines()
        .filter(|line| line.contains("WARNING"))
        .collect();
    assert_eq!(warnings.len(), 1, "one warning a process: {stderr}");
    assert!(
        warnings[0].contains(&Environment::var_name()) && warnings[0].contains("`production`"),
        "{stderr}"
    );
    assert!(
        !stderr.contains(UNRECOGNISED),
        "a deployment's value is never quoted: {stderr}"
    );
}

/// Reads the active environment, in an app that loads no config.
#[injectable]
struct ReadsTheEnvironment {
    #[inject]
    #[expect(dead_code, reason = "injected for the boot to resolve, never read")]
    environment: Arc<Environment>,
}

#[module(providers = [ReadsTheEnvironment])]
struct LoadsNoConfigModule;

#[tokio::test]
async fn an_app_loading_no_config_is_told_what_registers_the_environment() {
    let Err(refused) = App::builder().module::<LoadsNoConfigModule>().build().await else {
        panic!("no config loads, so nothing registers the environment");
    };
    let unmet = refused
        .downcast_ref::<MissingDependencyError>()
        .unwrap_or_else(|| panic!("not the unmet dependency: {refused:#}"));
    assert!(unmet.dependency.ends_with("Environment"), "{unmet:?}");
    let remedy = unmet
        .remedy
        .as_deref()
        .unwrap_or_else(|| panic!("the crate owning `Environment` names a remedy: {refused:#}"));
    assert!(
        remedy.contains("first config the boot loads")
            && remedy.contains("Environment::from_env()"),
        "{remedy}"
    );
}
