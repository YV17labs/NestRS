#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenTelemetryError {
    #[error("OpenTelemetry init failed: {0}")]
    Init(String),
    /// A set-but-unparseable log filter (`NESTRS_OPENTELEMETRY__LOG_LEVEL`, or a
    /// `with_log_filter` builder value) aborts boot naming the bad directive
    /// rather than silently degrading to `info` — framework config contract:
    /// set-but-unparseable is an error, never a fallback.
    #[error("invalid log filter {value:?}: {source}")]
    InvalidLogFilter {
        value: String,
        source: tracing_subscriber::filter::ParseError,
    },
    #[cfg(feature = "otlp")]
    #[error("OTLP exporter build failed: {0}")]
    Otlp(String),
}
