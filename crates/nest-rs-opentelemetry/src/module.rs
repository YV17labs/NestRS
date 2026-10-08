use nest_rs_core::{Registering, container::ContainerBuilder, module::Module};

use crate::OpenTelemetryError;

#[cfg(feature = "otlp")]
use crate::meter::OpenTelemetryMeter;

/// Registers the global OTel [`OpenTelemetryMeter`] as a provider, under the
/// `otlp` feature; it mounts no per-request layer.
///
/// **Ordering:** [`crate::OpenTelemetry::init`] must run before this module is
/// registered, or the boot fails with [`OpenTelemetryError::InitMissing`].
pub struct OpenTelemetryModule;

impl Module for OpenTelemetryModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        if !crate::init::initialized() {
            return builder.refuse(OpenTelemetryError::InitMissing);
        }
        #[cfg(feature = "otlp")]
        let builder = {
            let meter = opentelemetry::global::meter("nestrs");
            builder.provide_arc(std::sync::Arc::new(OpenTelemetryMeter(meter)))
        };
        builder
    }
}
