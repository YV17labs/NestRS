use nest_rs_core::{container::ContainerBuilder, module::Module};

use crate::OpenTelemetryError;

#[cfg(feature = "otlp")]
use crate::meter::OpenTelemetryMeter;

/// Registers the global OTel [`OpenTelemetryMeter`] as a provider, under the
/// `otlp` feature.
///
/// **It mounts no per-request layer, and that is the design.** Everything this
/// crate adds to a span is seeded onto the framework's span constructor at
/// `init` (see `linker`), so it reaches every edge — a queue job in a headless
/// worker as much as an HTTP request — instead of the one transport an
/// interceptor could have been attached to.
///
/// **Ordering:** [`crate::OpenTelemetry::init`] must run before this module is
/// registered, or the global tracer/meter are no-ops and signals are silently
/// dropped — the boot fails with [`OpenTelemetryError::InitMissing`].
pub struct OpenTelemetryModule;

impl Module for OpenTelemetryModule {
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(std::any::TypeId::of::<Self>()) {
            return builder;
        }
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
