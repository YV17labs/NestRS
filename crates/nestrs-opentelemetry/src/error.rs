#[derive(Debug, thiserror::Error)]
pub enum OpenTelemetryError {
    #[error("OpenTelemetry init failed: {0}")]
    Init(String),
    #[cfg(feature = "otlp")]
    #[error("OTLP exporter build failed: {0}")]
    Otlp(String),
}
