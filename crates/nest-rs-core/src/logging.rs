//! Baseline console logging, installed at boot when no global `tracing`
//! subscriber is set — so a bare app logs out of the box, with zero
//! per-request cost beyond `tracing`'s own filtering.
//!
//! An observability stack (e.g. `nest-rs-opentelemetry`) installs its richer
//! subscriber in `main` *before* the app builds; this fallback detects it and
//! steps aside. Configuration is env-only:
//!
//! - `NESTRS_LOG` (falling back to `RUST_LOG`) — `EnvFilter` directives,
//!   default `info`. Set-but-unparseable aborts boot, the same posture as
//!   every other framework var.
//! - `NESTRS_LOG_FORMAT` — `text` or `json`; defaults by build profile
//!   (text in debug, JSON in release), unrecognized values keep the default.
//! - `NESTRS_LOG_SOURCE_LOCATION` — append the emitting `file:line` to each
//!   event; off by default (widens every line, leaks source paths in prod).
//!
//! `NESTRS` is the default prefix; under
//! [`EnvPrefix::VAR`](crate::EnvPrefix::VAR) the three become `<PREFIX>_LOG*`.
//! `RUST_LOG` is not prefixed — it is the ecosystem's variable, not ours.
//!
//! The same three variables drive the console layer of any richer subscriber
//! the framework ships (`nest-rs-opentelemetry`), so an app's log config
//! survives adopting or dropping the observability stack unchanged.
//!
//! Behind the default `logging` cargo feature — an embedder that installs its
//! own subscriber can disable it and drop `tracing-subscriber` from the
//! kernel's dependency tree entirely.

use anyhow::Result;
use tracing_subscriber::EnvFilter;

use crate::env_prefix::EnvPrefix;

/// Console output shape for the fallback subscriber.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LogFormat {
    Text,
    Json,
}

impl LogFormat {
    /// `text`/`json` (trimmed, case-insensitive); anything else defers to the
    /// build-profile default — same tolerance as the rest of the framework's
    /// format vars.
    fn resolve(raw: Option<&str>) -> Self {
        let profile_default = if cfg!(debug_assertions) {
            Self::Text
        } else {
            Self::Json
        };
        match raw.map(|v| v.trim().to_ascii_lowercase()) {
            Some(v) if v == "text" => Self::Text,
            Some(v) if v == "json" => Self::Json,
            _ => profile_default,
        }
    }
}

/// `1`/`true`/`yes`/`on` ⇒ `true`; anything else (including unset) keeps the
/// caller's default — the same tolerant grammar as the format variable.
fn bool_from_env(name: &str) -> bool {
    std::env::var(name).is_ok_and(|v| {
        matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "1" | "true" | "yes" | "on"
        )
    })
}

/// Build the filter from `<PREFIX>_LOG` / `RUST_LOG` / `"info"`. A set-but-
/// unparseable directive is a config error that aborts boot, never a silent
/// downgrade to the default.
fn filter_from_env() -> Result<EnvFilter> {
    let log_var = EnvPrefix::var("LOG");
    let (var, spec) = match std::env::var(&log_var) {
        Ok(v) => (log_var.as_str(), v),
        Err(_) => match std::env::var("RUST_LOG") {
            Ok(v) => ("RUST_LOG", v),
            Err(_) => ("", "info".to_owned()),
        },
    };
    EnvFilter::try_new(&spec)
        .map_err(|e| anyhow::anyhow!("invalid log filter {spec:?} (from {var}): {e}"))
}

/// Install the fallback console subscriber unless one is already set.
/// Called by [`App`](crate::App) on both boot paths before the first boot
/// event, so module/route logs render even in a bare app.
pub(crate) fn init_fallback() -> Result<()> {
    if tracing::dispatcher::has_been_set() {
        return Ok(());
    }
    let filter = filter_from_env()?;
    let source_location = bool_from_env(&EnvPrefix::var("LOG_SOURCE_LOCATION"));
    let builder = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_file(source_location)
        .with_line_number(source_location);
    // A lost race against a concurrent install is the "already set" case —
    // the fallback steps aside; it never unseats another subscriber.
    let format = std::env::var(EnvPrefix::var("LOG_FORMAT")).ok();
    let _ = match LogFormat::resolve(format.as_deref()) {
        LogFormat::Text => builder.try_init(),
        LogFormat::Json => builder
            .json()
            .with_current_span(false)
            .with_span_list(false)
            .try_init(),
    };
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_resolves_canonical_names_case_insensitively() {
        assert_eq!(LogFormat::resolve(Some("json")), LogFormat::Json);
        assert_eq!(LogFormat::resolve(Some("JSON")), LogFormat::Json);
        assert_eq!(LogFormat::resolve(Some("  text  ")), LogFormat::Text);
    }

    #[test]
    fn format_defaults_by_build_profile_when_absent_or_unrecognized() {
        let expected = if cfg!(debug_assertions) {
            LogFormat::Text
        } else {
            LogFormat::Json
        };
        assert_eq!(LogFormat::resolve(None), expected);
        assert_eq!(LogFormat::resolve(Some("yaml")), expected);
    }

    #[test]
    fn a_valid_filter_directive_parses() {
        assert!(EnvFilter::try_new("debug,hyper=warn").is_ok());
    }

    #[test]
    fn an_invalid_filter_directive_is_rejected() {
        // The boot path maps this to an error naming the variable — a set-but-
        // unparseable filter must abort, not degrade to `info`.
        assert!(EnvFilter::try_new("foo=notalevel").is_err());
    }
}
