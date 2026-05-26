use nestrs_core::config::env_var;

/// Configuration for [`crate::Telemetry::init`].
///
/// # Environment variables
///
/// Follows the framework-wide `NESTRS_<DOMAIN>__<KEY>` scheme documented in
/// `nestrs_core::config` — double underscore separates the domain, the leaf
/// key keeps snake_case. Three domains are owned here: `log`, `service`,
/// `telemetry`. The `http` domain is owned by the `OtelHttp` interceptor directly.
///
/// | Setting              | Variable                            | Values / default                  |
/// |----------------------|-------------------------------------|-----------------------------------|
/// | log filter           | `NESTRS_LOG__LEVEL`                 | `EnvFilter` syntax, default `info`|
/// | log format           | `NESTRS_LOG__FORMAT`                | `text` (default) \| `json`        |
/// | service name         | `NESTRS_SERVICE__NAME`              | string                            |
/// | service version      | `NESTRS_SERVICE__VERSION`           | string                            |
/// | environment          | `NESTRS_SERVICE__ENVIRONMENT`       | string (`prod`, `staging`, …)     |
/// | instance id          | `NESTRS_SERVICE__INSTANCE_ID`       | string (default: fresh UUID v7)   |
/// | OTLP endpoint        | `NESTRS_TELEMETRY__OTLP_ENDPOINT`   | base URL, e.g. `http://otel:4318` |
/// | sampler ratio        | `NESTRS_TELEMETRY__SAMPLE_RATIO`    | `[0.0, 1.0]`, default `1.0`       |
///
/// The OTel exporter is wired only when [`Self::otlp_endpoint`] is `Some`;
/// otherwise telemetry stays console-only.
#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    /// `service.name` resource attribute.
    pub service_name: String,
    /// `service.version` resource attribute.
    pub service_version: Option<String>,
    /// `deployment.environment` resource attribute. Free-form (`prod`,
    /// `staging`, `dev`).
    pub deployment_environment: Option<String>,
    /// `service.instance.id`. Defaults to a fresh UUID v7 per process — so
    /// restarts produce distinct identities in the backend.
    pub service_instance_id: Option<String>,
    /// `EnvFilter`-syntax filter applied to the console layer **and** the
    /// OTel log appender — same gate for both.
    pub log_filter: String,
    /// Console layer encoding. JSON for machine ingestion, text for humans.
    pub log_format: LogFormat,
    /// OTLP base endpoint (e.g. `http://localhost:4318`). The exporter
    /// appends `/v1/traces`, `/v1/metrics`, `/v1/logs` per signal.
    pub otlp_endpoint: Option<String>,
    /// Head-based sample ratio in `[0.0, 1.0]`. `1.0` keeps every trace;
    /// pick `0.05` or `0.1` in prod. Wrapped in `ParentBased` so child
    /// spans inherit the parent's sampling decision.
    pub trace_sample_ratio: f64,
}

/// Console log encoding.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    /// Human-readable single-line format (tracing-subscriber default).
    #[default]
    Text,
    /// One JSON object per event. Suitable for `jq` and log shippers.
    Json,
}

impl LogFormat {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim().to_ascii_lowercase().as_str() {
            "text" => Some(Self::Text),
            "json" => Some(Self::Json),
            _ => None,
        }
    }
}

impl TelemetryConfig {
    pub fn new(service_name: impl Into<String>) -> Self {
        Self {
            service_name: service_name.into(),
            service_version: None,
            deployment_environment: None,
            service_instance_id: None,
            log_filter: "info".into(),
            log_format: LogFormat::Text,
            otlp_endpoint: None,
            trace_sample_ratio: 1.0,
        }
    }

    /// Read every `NESTRS_*` var listed in the type-level docs. The
    /// `service_name` argument is the default — it is overridden if
    /// `NESTRS_SERVICE__NAME` is set.
    pub fn from_env(service_name: impl Into<String>) -> Self {
        let mut cfg = Self::new(service_name);

        if let Some(v) = env_var("NESTRS_SERVICE__NAME") {
            cfg.service_name = v;
        }
        cfg.service_version = env_var("NESTRS_SERVICE__VERSION");
        cfg.deployment_environment = env_var("NESTRS_SERVICE__ENVIRONMENT");
        cfg.service_instance_id = env_var("NESTRS_SERVICE__INSTANCE_ID");

        if let Some(v) = env_var("NESTRS_LOG__LEVEL") {
            cfg.log_filter = v;
        }
        if let Some(raw) = env_var("NESTRS_LOG__FORMAT") {
            if let Some(fmt) = LogFormat::parse(&raw) {
                cfg.log_format = fmt;
            }
        }

        cfg.otlp_endpoint = env_var("NESTRS_TELEMETRY__OTLP_ENDPOINT");
        if let Some(raw) = env_var("NESTRS_TELEMETRY__SAMPLE_RATIO") {
            if let Ok(r) = raw.parse::<f64>() {
                cfg.trace_sample_ratio = r.clamp(0.0, 1.0);
            }
        }

        cfg
    }

    pub fn with_log_filter(mut self, filter: impl Into<String>) -> Self {
        self.log_filter = filter.into();
        self
    }

    pub fn with_log_format(mut self, format: LogFormat) -> Self {
        self.log_format = format;
        self
    }

    pub fn with_otlp_endpoint(mut self, endpoint: impl Into<String>) -> Self {
        self.otlp_endpoint = Some(endpoint.into());
        self
    }

    pub fn with_service_version(mut self, version: impl Into<String>) -> Self {
        self.service_version = Some(version.into());
        self
    }

    pub fn with_deployment_environment(mut self, env: impl Into<String>) -> Self {
        self.deployment_environment = Some(env.into());
        self
    }

    pub fn with_trace_sample_ratio(mut self, ratio: f64) -> Self {
        self.trace_sample_ratio = ratio.clamp(0.0, 1.0);
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_sample_everything() {
        let cfg = TelemetryConfig::new("svc");
        assert_eq!(cfg.trace_sample_ratio, 1.0);
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.log_filter, "info");
        assert_eq!(cfg.log_format, LogFormat::Text);
    }

    #[test]
    fn ratio_is_clamped() {
        let cfg = TelemetryConfig::new("svc").with_trace_sample_ratio(2.5);
        assert_eq!(cfg.trace_sample_ratio, 1.0);
        let cfg = TelemetryConfig::new("svc").with_trace_sample_ratio(-1.0);
        assert_eq!(cfg.trace_sample_ratio, 0.0);
    }

    #[test]
    fn log_format_parses_canonical_names_only() {
        assert_eq!(LogFormat::parse("json"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse("JSON"), Some(LogFormat::Json));
        assert_eq!(LogFormat::parse("  text  "), Some(LogFormat::Text));
        assert_eq!(LogFormat::parse("console"), None);
        assert_eq!(LogFormat::parse("yaml"), None);
    }
}
