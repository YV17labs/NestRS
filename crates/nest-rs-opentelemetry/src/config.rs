use nest_rs_config::env_var;

/// Configuration for [`crate::OpenTelemetry::init`].
///
/// Env vars under the `otel` domain
/// (`NESTRS_OPENTELEMETRY__{LOG_LEVEL,LOG_FORMAT,SERVICE_NAME,SERVICE_VERSION,
/// SERVICE_ENVIRONMENT,SERVICE_INSTANCE_ID,OTLP_ENDPOINT,SAMPLE_RATIO}`).
/// OTel exporter is wired only when `otlp_endpoint` is set; otherwise the
/// subscriber stays console-only.
#[derive(Clone, Debug)]
pub struct OpenTelemetryConfig {
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

impl OpenTelemetryConfig {
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

    /// `service_name` is the default; `NESTRS_OPENTELEMETRY__SERVICE_NAME` overrides.
    pub fn from_env(service_name: impl Into<String>) -> Self {
        let mut cfg = Self::new(service_name);

        if let Some(v) = env_var("NESTRS_OPENTELEMETRY__SERVICE_NAME") {
            cfg.service_name = v;
        }
        cfg.service_version = env_var("NESTRS_OPENTELEMETRY__SERVICE_VERSION");
        cfg.deployment_environment = env_var("NESTRS_OPENTELEMETRY__SERVICE_ENVIRONMENT");
        cfg.service_instance_id = env_var("NESTRS_OPENTELEMETRY__SERVICE_INSTANCE_ID");

        if let Some(v) = env_var("NESTRS_OPENTELEMETRY__LOG_LEVEL") {
            cfg.log_filter = v;
        }
        if let Some(raw) = env_var("NESTRS_OPENTELEMETRY__LOG_FORMAT")
            && let Some(fmt) = LogFormat::parse(&raw)
        {
            cfg.log_format = fmt;
        }

        cfg.otlp_endpoint = env_var("NESTRS_OPENTELEMETRY__OTLP_ENDPOINT");
        if let Some(raw) = env_var("NESTRS_OPENTELEMETRY__SAMPLE_RATIO")
            && let Ok(r) = raw.parse::<f64>()
        {
            cfg.trace_sample_ratio = r.clamp(0.0, 1.0);
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
        let cfg = OpenTelemetryConfig::new("svc");
        assert_eq!(cfg.trace_sample_ratio, 1.0);
        assert!(cfg.otlp_endpoint.is_none());
        assert_eq!(cfg.log_filter, "info");
        assert_eq!(cfg.log_format, LogFormat::Text);
    }

    #[test]
    fn ratio_is_clamped() {
        let cfg = OpenTelemetryConfig::new("svc").with_trace_sample_ratio(2.5);
        assert_eq!(cfg.trace_sample_ratio, 1.0);
        let cfg = OpenTelemetryConfig::new("svc").with_trace_sample_ratio(-1.0);
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

    #[test]
    fn new_takes_the_service_name_as_owned_string() {
        // Accepts `&str` and `String` — verify both compile and yield the same value.
        let from_str = OpenTelemetryConfig::new("svc-a");
        let from_string = OpenTelemetryConfig::new(String::from("svc-a"));
        assert_eq!(from_str.service_name, from_string.service_name);
        assert_eq!(from_str.service_name, "svc-a");
    }

    #[test]
    fn defaults_have_no_optional_attrs() {
        let cfg = OpenTelemetryConfig::new("svc");
        assert!(cfg.service_version.is_none());
        assert!(cfg.deployment_environment.is_none());
        assert!(cfg.service_instance_id.is_none());
    }

    #[test]
    fn with_log_filter_overrides_the_default() {
        let cfg = OpenTelemetryConfig::new("svc").with_log_filter("debug,hyper=warn");
        assert_eq!(cfg.log_filter, "debug,hyper=warn");
    }

    #[test]
    fn with_log_format_pins_the_supplied_variant() {
        let cfg = OpenTelemetryConfig::new("svc").with_log_format(LogFormat::Json);
        assert_eq!(cfg.log_format, LogFormat::Json);
    }

    #[test]
    fn with_otlp_endpoint_attaches_the_value() {
        let cfg = OpenTelemetryConfig::new("svc").with_otlp_endpoint("http://otel:4318");
        assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://otel:4318"));
    }

    #[test]
    fn with_service_version_attaches_the_value() {
        let cfg = OpenTelemetryConfig::new("svc").with_service_version("v1.2.3");
        assert_eq!(cfg.service_version.as_deref(), Some("v1.2.3"));
    }

    #[test]
    fn with_deployment_environment_attaches_the_value() {
        let cfg = OpenTelemetryConfig::new("svc").with_deployment_environment("prod");
        assert_eq!(cfg.deployment_environment.as_deref(), Some("prod"));
    }

    #[test]
    fn log_format_default_is_text() {
        // Pin the default at the trait level — flipping to Json would change
        // every default app's log output overnight.
        assert_eq!(LogFormat::default(), LogFormat::Text);
    }

    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn with_env<R>(vars: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        let _guard = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        for (k, v) in vars {
            match v {
                Some(value) => unsafe { std::env::set_var(k, value) },
                None => unsafe { std::env::remove_var(k) },
            }
        }
        let out = f();
        for (k, _) in vars {
            unsafe { std::env::remove_var(k) };
        }
        out
    }

    #[test]
    fn from_env_falls_back_to_defaults_for_every_optional_field() {
        with_env(
            &[
                ("NESTRS_OPENTELEMETRY__SERVICE_NAME", None),
                ("NESTRS_OPENTELEMETRY__SERVICE_VERSION", None),
                ("NESTRS_OPENTELEMETRY__SERVICE_ENVIRONMENT", None),
                ("NESTRS_OPENTELEMETRY__SERVICE_INSTANCE_ID", None),
                ("NESTRS_OPENTELEMETRY__LOG_LEVEL", None),
                ("NESTRS_OPENTELEMETRY__LOG_FORMAT", None),
                ("NESTRS_OPENTELEMETRY__OTLP_ENDPOINT", None),
                ("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", None),
            ],
            || {
                let cfg = OpenTelemetryConfig::from_env("default-svc");
                assert_eq!(cfg.service_name, "default-svc");
                assert!(cfg.service_version.is_none());
                assert!(cfg.deployment_environment.is_none());
                assert!(cfg.service_instance_id.is_none());
                assert!(cfg.otlp_endpoint.is_none());
                assert_eq!(cfg.log_filter, "info");
                assert_eq!(cfg.log_format, LogFormat::Text);
                assert_eq!(cfg.trace_sample_ratio, 1.0);
            },
        );
    }

    #[test]
    fn from_env_overrides_each_field_when_set() {
        with_env(
            &[
                ("NESTRS_OPENTELEMETRY__SERVICE_NAME", Some("override-svc")),
                ("NESTRS_OPENTELEMETRY__SERVICE_VERSION", Some("9.9.9")),
                ("NESTRS_OPENTELEMETRY__SERVICE_ENVIRONMENT", Some("prod")),
                (
                    "NESTRS_OPENTELEMETRY__SERVICE_INSTANCE_ID",
                    Some("pinned-1"),
                ),
                ("NESTRS_OPENTELEMETRY__LOG_LEVEL", Some("debug,hyper=warn")),
                ("NESTRS_OPENTELEMETRY__LOG_FORMAT", Some("json")),
                (
                    "NESTRS_OPENTELEMETRY__OTLP_ENDPOINT",
                    Some("http://otel:4318"),
                ),
                ("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", Some("0.25")),
            ],
            || {
                let cfg = OpenTelemetryConfig::from_env("default-svc");
                assert_eq!(cfg.service_name, "override-svc");
                assert_eq!(cfg.service_version.as_deref(), Some("9.9.9"));
                assert_eq!(cfg.deployment_environment.as_deref(), Some("prod"));
                assert_eq!(cfg.service_instance_id.as_deref(), Some("pinned-1"));
                assert_eq!(cfg.log_filter, "debug,hyper=warn");
                assert_eq!(cfg.log_format, LogFormat::Json);
                assert_eq!(cfg.otlp_endpoint.as_deref(), Some("http://otel:4318"));
                assert!((cfg.trace_sample_ratio - 0.25).abs() < f64::EPSILON);
            },
        );
    }

    #[test]
    fn from_env_clamps_ratio_outside_zero_to_one() {
        with_env(
            &[("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", Some("2.5"))],
            || {
                assert_eq!(OpenTelemetryConfig::from_env("svc").trace_sample_ratio, 1.0);
            },
        );
        with_env(
            &[("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", Some("-0.5"))],
            || {
                assert_eq!(OpenTelemetryConfig::from_env("svc").trace_sample_ratio, 0.0);
            },
        );
    }

    #[test]
    fn from_env_ignores_unparseable_ratio_and_log_format() {
        with_env(
            &[
                ("NESTRS_OPENTELEMETRY__SAMPLE_RATIO", Some("not-a-number")),
                ("NESTRS_OPENTELEMETRY__LOG_FORMAT", Some("console")),
            ],
            || {
                // Both stick to defaults — never panic on bad input.
                let cfg = OpenTelemetryConfig::from_env("svc");
                assert_eq!(cfg.trace_sample_ratio, 1.0);
                assert_eq!(cfg.log_format, LogFormat::Text);
            },
        );
    }
}
