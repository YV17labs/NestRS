//! Boot contract of `OpenTelemetryModule` (`src/module.rs`).

use nest_rs_core::{Container, Module};
use nest_rs_opentelemetry::OpenTelemetryModule;

#[test]
#[should_panic(expected = "without calling `OpenTelemetry::init`")]
fn importing_the_module_without_init_panics() {
    let _ = OpenTelemetryModule::register(Container::builder());
}
