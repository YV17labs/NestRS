/// Failure raised while initializing the telemetry subscriber; every variant
/// aborts boot rather than degrading silently.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenTelemetryError {
    /// Subscriber setup failed — e.g. a global subscriber was already installed.
    /// Carries the underlying message.
    #[error("OpenTelemetry init failed: {0}")]
    Init(String),
    /// A set-but-unparseable log filter (`NESTRS_OPENTELEMETRY__LOG_LEVEL`, or a
    /// `with_log_filter` builder value) aborts boot naming the bad directive
    /// rather than silently degrading to `info` — framework config contract:
    /// set-but-unparseable is an error, never a fallback.
    #[error("invalid log filter {value:?}: {source}")]
    InvalidLogFilter {
        /// The offending directive string, echoed back so the operator can see
        /// exactly what failed to parse.
        value: String,
        /// The underlying `EnvFilter` parse error.
        source: tracing_subscriber::filter::ParseError,
    },
    /// Building the OTLP exporter pipeline failed (bad endpoint, transport
    /// error). Only present under the `otlp` feature.
    #[cfg(feature = "otlp")]
    #[error("OTLP exporter build failed: {0}")]
    Otlp(String),
}
