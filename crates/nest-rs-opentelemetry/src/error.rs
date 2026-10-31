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
         Add `let _otel = nest_rs::opentelemetry::OpenTelemetry::init()?;` at the top of \
         `main`, before building the app."
    )]
    InitMissing,
    /// A setting was refused, named and never quoted: an `OTEL_*` variable or a
    /// field set in code that does not parse or is outside its range, an
    /// endpoint OTLP/HTTP cannot speak to, a file a certificate variable names
    /// that cannot be read, or TLS material that cannot be used.
    #[error(transparent)]
    Config(#[from] nest_rs_config::ConfigError),
    /// A console setting does not parse — `RUST_LOG`, `<PREFIX>_LOG_FORMAT`,
    /// `<PREFIX>_LOG_SOURCE_LOCATION`, or `OpenTelemetryConfig::log_filter` set
    /// in code — named, never quoted.
    #[error(transparent)]
    LogSettings(#[from] nest_rs_core::logging::LogSettingsError),
    /// Building the OTLP exporter pipeline failed (bad endpoint, transport
    /// error). Only present under the `otlp` feature.
    #[cfg(feature = "otlp")]
    #[error("OTLP exporter build failed: {0}")]
    Otlp(String),
}
