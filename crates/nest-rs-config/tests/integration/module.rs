//! `ConfigModule::for_root()` registers the active environment, and says once
//! when the process declared none.

use nest_rs_config::{ConfigModule, Environment};
use nest_rs_core::{App, module};
use nest_rs_testing::LogCapture;

const UNDECLARED: &str = "no environment declared; running as production";

#[module(imports = [ConfigModule::for_root()])]
struct RootModule;

/// Boot `RootModule` with `<PREFIX>_ENV` holding `declared` (`""` declares
/// none), returning the environment it registered and what the boot filed.
#[expect(
    clippy::result_large_err,
    reason = "figment::Jail fixes the closure's error type"
)]
fn boot_declaring(declared: &str) -> (Environment, LogCapture) {
    let mut booted = None;
    figment::Jail::expect_with(|jail| {
        jail.set_env(Environment::var_name(), declared);
        let logs = LogCapture::install();
        let app = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("test runtime")
            .block_on(async { App::builder().module::<RootModule>().build().await })
            .expect("the root boots");
        let environment = *app
            .container()
            .get::<Environment>()
            .expect("the root registers the active environment");
        booted = Some((environment, logs));
        Ok(())
    });
    booted.expect("the jailed boot ran")
}

#[test]
fn an_undeclared_process_registers_production_and_says_so_once() {
    let (environment, logs) = boot_declaring("");
    assert_eq!(environment, Environment::Production);
    let event = logs.expect_one(nest_rs_config::TARGET, UNDECLARED);
    assert_eq!(event.level, "info");
    assert_eq!(event.field("declared").as_deref(), Some("false"));
    assert_eq!(event.field("environment").as_deref(), Some("production"));
}

#[test]
fn a_declared_environment_boots_silently() {
    for declared in ["production", "development", "test", "staging"] {
        let (environment, logs) = boot_declaring(declared);
        assert_eq!(environment.as_str(), declared);
        logs.expect_none(nest_rs_config::TARGET, UNDECLARED);
    }
}
