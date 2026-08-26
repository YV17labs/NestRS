/// Injectable wrapper over the global OTel [`Meter`](opentelemetry::metrics::Meter),
/// registered by [`OpenTelemetryModule`](crate::OpenTelemetryModule) under the
/// `otlp` feature so feature services can create instruments without reaching
/// for the global directly. Derefs to the inner meter.
pub struct OpenTelemetryMeter(pub opentelemetry::metrics::Meter);

impl std::ops::Deref for OpenTelemetryMeter {
    type Target = opentelemetry::metrics::Meter;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}
