/// Failure raised while initializing the telemetry subscriber; every variant
/// aborts boot rather than degrading silently.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum OpenTelemetryError {
    /// Subscriber setup failed — e.g. a global subscriber was already installed.
    #[error("OpenTelemetry init failed: {0}")]
    Init(String),
    /// `OpenTelemetryModule` was imported in an app whose `main` never called
    /// `OpenTelemetry::init`: the global tracer and meter are no-ops, so every
    /// signal would be dropped in silence.
    #[error(
        "OpenTelemetryModule was imported without calling `OpenTelemetry::init` first — the \
         global tracer and meter are no-ops, so traces and metrics would be silently dropped. \
         Add `let _otel = nest_rs::opentelemetry::OpenTelemetry::init(\"<service>\")?;` at the \
         top of `main`, before building the app."
    )]
    InitMissing,
    /// A `<PREFIX>_OPENTELEMETRY__*` variable could not be read — both of its
    /// spellings set, or a `<KEY>_FILE` naming a file that cannot be read.
    #[error(transparent)]
    Config(#[from] nest_rs_config::ConfigError),
    /// A set-but-unparseable log filter (`<PREFIX>_LOG`, or a `with_log_filter`
    /// builder value) aborts boot naming the bad directive.
    #[error("invalid log filter {value:?}: {source}")]
    InvalidLogFilter {
        /// The offending directive string.
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
