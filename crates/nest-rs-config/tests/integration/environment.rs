//! `<PREFIX>_ENV` read off the process: what an unrecognised value says, in a
//! child process, since the warning goes to stderr before any subscriber exists.

use std::process::Command;

use nest_rs_config::Environment;

const CHILD_ROLE: &str = "NEST_RS_CONFIG_ENVIRONMENT_CHILD";

const CHILD_TEST: &str = "environment::unrecognised_child_process";

const UNRECOGNISED: &str = "prd-with-a-private-word";

#[test]
#[expect(
    clippy::disallowed_methods,
    reason = "the parent test hands the child its role through the environment"
)]
fn unrecognised_child_process() {
    if std::env::var(CHILD_ROLE).is_ok() {
        for _ in 0..2 {
            println!("ENVIRONMENT={}", Environment::from_env().as_str());
        }
    }
}

#[test]
fn an_unrecognised_value_is_read_as_production_and_named_by_its_variable_alone() {
    let child = Command::new(std::env::current_exe().expect("the test binary"))
        .args(["--exact", CHILD_TEST, "--nocapture"])
        .env(CHILD_ROLE, "1")
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
