//! What the deployment asks to export — the OpenTelemetry specification's OTLP
//! endpoints — and the refusal [`App::run`](crate::App::run) files when the
//! process logs through the kernel's console alone, which exports nothing.

use crate::error::UnservedExportError;

/// The specification's switch for every signal.
const OTEL_SDK_DISABLED: &str = "OTEL_SDK_DISABLED";

/// The base endpoint every signal appends its path to.
const OTEL_EXPORTER_OTLP_ENDPOINT: &str = "OTEL_EXPORTER_OTLP_ENDPOINT";

/// Each signal's own endpoint and its selector.
const SIGNALS: [(&str, &str); 3] = [
    ("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT", "OTEL_TRACES_EXPORTER"),
    (
        "OTEL_EXPORTER_OTLP_METRICS_ENDPOINT",
        "OTEL_METRICS_EXPORTER",
    ),
    ("OTEL_EXPORTER_OTLP_LOGS_ENDPOINT", "OTEL_LOGS_EXPORTER"),
];

/// The variable that sets an OTLP endpoint for a signal the deployment leaves
/// on — the signal's own, then the base — or `None` when none does. A signal
/// is off when `OTEL_SDK_DISABLED` is `true` or its selector names `none`
/// alone, in any case; blank is unset. Any other selector is the exporter's to
/// judge, so it leaves the signal on here.
pub(crate) fn unserved_export(read: impl Fn(&str) -> Option<String>) -> Option<&'static str> {
    let read = |name: &str| read(name).filter(|value| !value.trim().is_empty());
    if read(OTEL_SDK_DISABLED).is_some_and(|value| value.trim().eq_ignore_ascii_case("true")) {
        return None;
    }
    SIGNALS
        .into_iter()
        .filter(|(_, selector)| !names_none_alone(read(selector)))
        .find_map(|(endpoint, _)| {
            [endpoint, OTEL_EXPORTER_OTLP_ENDPOINT]
                .into_iter()
                .find(|variable| read(variable).is_some())
        })
}

/// Whether a selector names `none` and nothing else.
fn names_none_alone(selector: Option<String>) -> bool {
    selector.is_some_and(|selector| {
        let mut names = selector
            .split(',')
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .peekable();
        names.peek().is_some() && names.all(|name| name.eq_ignore_ascii_case("none"))
    })
}

/// Refuses the run when the deployment sets an endpoint [`unserved_export`]
/// names and the kernel's console is the process's subscriber: an app that
/// installed a subscriber of its own is its own to judge.
#[expect(
    clippy::disallowed_methods,
    reason = "the specification's variables, read from the process as the SDK reads them"
)]
pub(crate) fn refuse_unserved_export() -> Result<(), UnservedExportError> {
    if !crate::logging::kernel_console_installed() {
        return Ok(());
    }
    match unserved_export(|name| std::env::var(name).ok()) {
        Some(variable) => Err(UnservedExportError { variable }),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The variables `set` holds, and nothing else.
    fn reading(set: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> + use<> {
        let set: Vec<(String, String)> = set
            .iter()
            .map(|(name, value)| ((*name).to_owned(), (*value).to_owned()))
            .collect();
        move |name: &str| {
            set.iter()
                .find(|(variable, _)| variable == name)
                .map(|(_, value)| value.clone())
        }
    }

    #[test]
    fn the_base_endpoint_or_a_signal_s_own_names_its_variable() {
        assert_eq!(
            unserved_export(reading(&[(OTEL_EXPORTER_OTLP_ENDPOINT, "http://c:4318")])),
            Some(OTEL_EXPORTER_OTLP_ENDPOINT)
        );
        for (endpoint, _) in SIGNALS {
            assert_eq!(
                unserved_export(reading(&[(endpoint, "http://c:4318/v1/x")])),
                Some(endpoint)
            );
        }
        assert_eq!(
            unserved_export(reading(&[
                (OTEL_EXPORTER_OTLP_ENDPOINT, "http://c:4318"),
                (SIGNALS[0].0, "http://c:4318/v1/traces"),
            ])),
            Some(SIGNALS[0].0),
            "a signal's own variable comes first"
        );
    }

    #[test]
    fn no_endpoint_or_a_blank_one_asks_for_nothing() {
        assert_eq!(unserved_export(reading(&[])), None);
        assert_eq!(
            unserved_export(reading(&[(OTEL_EXPORTER_OTLP_ENDPOINT, "  ")])),
            None
        );
    }

    /// The specification's switches turn a signal off whatever endpoint names
    /// it: the SDK's, every signal, and a selector naming `none` alone.
    #[test]
    fn a_signal_switched_off_asks_for_nothing() {
        let base = (OTEL_EXPORTER_OTLP_ENDPOINT, "http://c:4318");
        for disabled in ["true", " TRUE "] {
            assert_eq!(
                unserved_export(reading(&[base, (OTEL_SDK_DISABLED, disabled)])),
                None
            );
        }
        assert_eq!(
            unserved_export(reading(&[base, (OTEL_SDK_DISABLED, "false")])),
            Some(OTEL_EXPORTER_OTLP_ENDPOINT)
        );
        let every_signal_off: Vec<(&str, &str)> = std::iter::once(base)
            .chain(SIGNALS.iter().map(|(_, selector)| (*selector, " None ")))
            .collect();
        assert_eq!(unserved_export(reading(&every_signal_off)), None);

        assert_eq!(
            unserved_export(reading(&[
                (SIGNALS[0].0, "http://c:4318/v1/traces"),
                (SIGNALS[0].1, "none"),
            ])),
            None,
            "the traces' own endpoint, its selector off"
        );
        assert_eq!(
            unserved_export(reading(&[base, (SIGNALS[0].1, "none")])),
            Some(OTEL_EXPORTER_OTLP_ENDPOINT),
            "the base still reaches the metrics and the logs"
        );
        assert_eq!(
            unserved_export(reading(&[base, (SIGNALS[0].1, "otlp,none")])),
            Some(OTEL_EXPORTER_OTLP_ENDPOINT),
            "a selector naming another beside none is the exporter's to judge"
        );
    }
}
