use nestrs_core::{container::ContainerBuilder, module::Module, Discoverable};

use crate::interceptor::OtelHttp;

/// Telemetry module — the crate's public entry point. Compose with
/// `#[module(imports = [TelemetryModule, ...])]`.
///
/// Registers the HTTP interceptor (`OtelHttp`, crate-private) so importing this
/// module activates per-request tracing / access logging — apps never name the
/// interceptor type. When the `otlp` feature is on, it also registers the global
/// OTel [`Meter`] as a provider so services can `#[inject]` it directly:
///
/// ```ignore
/// #[injectable]
/// pub struct UserService {
///     #[inject] meter: std::sync::Arc<nestrs_telemetry::Meter>,
/// }
/// ```
///
/// Without the `otlp` feature it registers only the interceptor, not the meter.
///
/// **Ordering:** [`crate::Telemetry::init`] must run before the module is
/// registered, so the global meter provider is installed first.
pub struct TelemetryModule;

impl Module for TelemetryModule {
    fn register(mut builder: ContainerBuilder) -> ContainerBuilder {
        // Idempotent like a macro-generated module: a diamond import registers once.
        if !builder.mark_registered(std::any::TypeId::of::<Self>()) {
            return builder;
        }
        // The interceptor is the feature's discoverable HTTP surface: its
        // `Discoverable::register` attaches the `HttpInterceptorMeta` the transport
        // reads, so `imports = [TelemetryModule]` wires it without the app naming it.
        let builder = <OtelHttp as Discoverable>::register(builder);
        #[cfg(feature = "otlp")]
        let builder = {
            let meter = opentelemetry::global::meter("nestrs");
            builder.provide_arc(std::sync::Arc::new(Meter(meter)))
        };
        builder
    }
}

/// Wrapper around the global OTel meter so it can be registered as a typed
/// provider in the nestrs container.
#[cfg(feature = "otlp")]
pub struct Meter(pub opentelemetry::metrics::Meter);

#[cfg(feature = "otlp")]
impl std::ops::Deref for Meter {
    type Target = opentelemetry::metrics::Meter;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
