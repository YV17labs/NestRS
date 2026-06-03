use nestrs_core::{container::ContainerBuilder, module::Module};

#[cfg(feature = "http")]
use crate::interceptor::OtelHttp;
#[cfg(feature = "http")]
use nestrs_core::Discoverable;

/// Registers the crate-private HTTP interceptor (per-request tracing / access
/// log) and, with the `otlp` feature, the global OTel [`Meter`] as a provider.
///
/// **Ordering:** [`crate::Telemetry::init`] must run before this module is
/// registered, or the global tracer/meter are no-ops and signals are silently
/// dropped — boot panics with a clear message.
pub struct TelemetryModule;

impl Module for TelemetryModule {
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        if !builder.mark_registered(std::any::TypeId::of::<Self>()) {
            return builder;
        }
        // Module::register has no Result to thread back, so a panic is the
        // only way to surface the ordering contract before signals are lost.
        if !crate::telemetry::initialized() {
            panic!(
                "TelemetryModule was imported without calling `Telemetry::init` first — \
                 the global tracer and meter are no-ops, so traces and metrics would be \
                 silently dropped. Add `let _telemetry = \
                 nestrs_telemetry::Telemetry::init(\"<service>\")?;` at the top of `main`, \
                 before building the app."
            );
        }
        #[cfg(feature = "http")]
        let builder = <OtelHttp as Discoverable>::register(builder);
        #[cfg(feature = "otlp")]
        let builder = {
            let meter = opentelemetry::global::meter("nestrs");
            builder.provide_arc(std::sync::Arc::new(Meter(meter)))
        };
        builder
    }
}

#[cfg(feature = "otlp")]
pub struct Meter(pub opentelemetry::metrics::Meter);

#[cfg(feature = "otlp")]
impl std::ops::Deref for Meter {
    type Target = opentelemetry::metrics::Meter;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
