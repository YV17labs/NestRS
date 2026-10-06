//! Boot contract of `OpenTelemetryModule` (`src/module.rs`).

use nest_rs_core::App;
use nest_rs_opentelemetry::{OpenTelemetryError, OpenTelemetryModule};

#[test]
fn importing_the_module_without_init_fails_the_boot_naming_init() {
    let Err(refused) = App::new::<OpenTelemetryModule>() else {
        panic!("a module whose signals would all be dropped boots");
    };
    assert!(
        matches!(
            refused.downcast_ref::<OpenTelemetryError>(),
            Some(OpenTelemetryError::InitMissing)
        ),
        "{refused:#}"
    );
    assert!(
        format!("{refused}").contains("without calling `OpenTelemetry::init`"),
        "{refused}"
    );
}
