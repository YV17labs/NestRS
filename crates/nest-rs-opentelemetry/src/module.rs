use std::any::TypeId;

use nest_rs_core::{Registering, container::ContainerBuilder, module::Module};

use crate::Metrics;

nest_rs_core::inventory::submit! {
    nest_rs_core::DependencyRemedy::new(
        TypeId::of::<Metrics>,
        "Import `nest_rs::opentelemetry::OpenTelemetryModule` in the module whose provider \
         injects `Metrics`; it provides them over the meter `OpenTelemetry::init()` installs \
         in `main`.",
    )
}

/// Provides [`Metrics`] over the global meter, for the module whose provider
/// injects them; it mounts nothing, and an app recording no metric of its own
/// imports it nowhere.
///
/// Telemetry itself is `let _otel = OpenTelemetry::init()?;` in `main`, which
/// installs the meter this module hands out. A suite boots without it, and its
/// `Metrics` record nothing.
pub struct OpenTelemetryModule;

impl Module for OpenTelemetryModule {
    fn register(builder: ContainerBuilder, _: Registering<Self>) -> ContainerBuilder {
        let meter = opentelemetry::global::meter("nestrs");
        builder.provide_arc(std::sync::Arc::new(Metrics::new(meter)))
    }
}
