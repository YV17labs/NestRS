use nestrs_config::env_var;

/// Configuration for [`crate::Telemetry::init`].
///
/// Env vars under the `telemetry` domain
/// (`NESTRS_TELEMETRY__{LOG_LEVEL,LOG_FORMAT,SERVICE_NAME,SERVICE_VERSION,
/// SERVICE_ENVIRONMENT,SERVICE_INSTANCE_ID,OTLP_ENDPOINT,SAMPLE_RATIO}`).
/// OTel exporter is wired only when `otlp_endpoint` is set; otherwise
/// telemetry stays console-only.
#[derive(Clone, Debug)]
pub struct TelemetryConfig {
    pub service_name: String,
    pub service_version: Option<String>,
    pub deployment_environment: Option<String>,
    /// Defaults to a fresh UUID v7 per process so restarts get distinct
    /// identities in the backend.
    pub service_instance_id: Option<String>,
    /// `EnvFilter` syntax; applied to console layer and OTel log appender.
    pub log_filter: String,
    pub log_format: LogFormat,
    /// Base endpoint (e.g. `http://localhost:4318`); exporter appends
    /// `/v1/traces`, `/v1/metrics`, `/v1/logs`.
    pub otlp_endpoint: Option<String>,
    /// `[0.0, 1.0]`; wrapped in `ParentBased` so children inherit the
    /// parent's sampling decision.
    pub trace_sample_ratio: f64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum LogFormat {
    #[default]
    Text,
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

    /// `service_name` is the default; `NESTRS_TELEMETRY__SERVICE_NAME` overrides.
    pub fn from_env(service_name: impl Into<String>) -> Self {
        let mut cfg = Self::new(service_name);

        if let Some(v) = env_var("NESTRS_TELEMETRY__SERVICE_NAME") {
            cfg.service_name = v;
        }
        cfg.service_version = env_var("NESTRS_TELEMETRY__SERVICE_VERSION");
        cfg.deployment_environment = env_var("NESTRS_TELEMETRY__SERVICE_ENVIRONMENT");
        cfg.service_instance_id = env_var("NESTRS_TELEMETRY__SERVICE_INSTANCE_ID");

        if let Some(v) = env_var("NESTRS_TELEMETRY__LOG_LEVEL") {
            cfg.log_filter = v;
        }
        if let Some(raw) = env_var("NESTRS_TELEMETRY__LOG_FORMAT") {
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
