//! Configuration failures.

use std::fmt::Write as _;

use nest_rs_core::DecodeError;
use thiserror::Error;
use validator::{ValidationErrors, ValidationErrorsKind};

/// A configuration load failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Names the offending variable so the misconfig is obvious at boot.
    ///
    /// Built only through [`ConfigError::parse`], which says the message
    /// without any value a decoder quoted in it — so a reason handed over
    /// whole, a serde or TOML error included, cannot carry a secret into the
    /// boot error.
    #[error("invalid value for {var}: {message}")]
    #[non_exhaustive]
    Parse {
        /// The offending `<PREFIX>_<DOMAIN>__<KEY>` variable name.
        var: String,
        /// Why the value was rejected.
        message: String,
    },
    /// A structured value that did not decode as its type.
    ///
    /// Carries [`DecodeError`], never serde's own sentence: that one quotes the
    /// value it refused, and a structured value is where a deployment writes
    /// records with credentials in them — an OAuth client list carries each
    /// client's secret. So the refusal says where the value failed, what kind of
    /// value it found and what it expected, whichever spelling supplied it.
    #[error("invalid value for {var}: {source}")]
    Decode {
        /// The variable that supplied the value — `<KEY>` or `<KEY>_FILE`.
        var: String,
        /// Where and why it did not decode, without the value.
        #[source]
        source: DecodeError,
    },
    /// A duration's variable — its key ends in `_SECS` or `_MS` — asked of a
    /// [`ConfigService`](crate::ConfigService) reader, which holds a value to no
    /// range. A defect in the config's `from_env`, refused at the first boot
    /// that reads it, whatever the deployment set.
    #[error(
        "{var} is a duration, and a duration is read through `DurationBounds`, which holds it \
         to a floor and a ceiling and names its unit — never through a `ConfigService` reader"
    )]
    UnboundedDuration {
        /// The variable the reader was asked for.
        var: String,
    },
    /// A loaded config failed `validator::Validate`.
    ///
    /// Renders the **namespace** and one line per offending field. The
    /// namespace is what disambiguates which config failed when several are
    /// loaded, and rendering `validator`'s own `Debug` payload instead put a
    /// raw `[{"min": Number(1), "value": String("")}]` — the submitted value
    /// included — into an operator-facing line.
    #[error(
        "configuration validation failed for '{namespace}'\n{}",
        render(errors)
    )]
    Validation {
        /// The `#[config(namespace = "…")]` of the config that failed.
        namespace: &'static str,
        /// The field-level failures. Deliberately **not** `#[source]`: the
        /// rendered message already lists them, and a `Display` chain would
        /// print `validator`'s raw payload underneath the curated list.
        errors: ValidationErrors,
    },
    /// Two config types read one environment variable.
    ///
    /// `<PREFIX>_<DOMAIN>__<KEY>` is a flat, process-global name space, and a
    /// namespace is read off the declaring file's path, so two types may well
    /// meet under one prefix — a registry and the members it discovers, a
    /// config and a sub-struct it delegates to. What may not be shared is a
    /// **variable**: two types reading one name means
    /// a deployment setting it configures whichever happens to read it, both
    /// silently, and what the operator sees is "the value I set did nothing".
    ///
    /// Raised at boot, from the resolved name rather than from the key: a key is
    /// a literal in a position nothing can enumerate — read through a `const`,
    /// through an inherent sub-struct's `from_env`, or built at the call site —
    /// so the only place the full name is knowable is where it is actually
    /// asked for.
    #[error(
        "`{var}` is read by two configuration types — `{owner}` and `{claimant}`. \
         A `<PREFIX>_<DOMAIN>__<KEY>` variable belongs to one type: setting it would \
         configure whichever read it, with nothing to say which. Give one of the two \
         its own key, or its own `#[config(namespace = \"…\")]`."
    )]
    ContestedVariable {
        /// The fully-qualified variable both types read.
        var: String,
        /// The type that claimed it first.
        owner: &'static str,
        /// The type that claimed it second.
        claimant: &'static str,
    },
    /// Two configuration types declare one namespace.
    ///
    /// A namespace is read off the declaring file's path as the type's own name
    /// is, so from a variable a reader finds the one type that parses it. Two
    /// types under one namespace break that, and the unclaimed-variable report
    /// with it: its key check runs once a config of the namespace has been
    /// read, and the other type's keys were then reported as read by nothing on
    /// a deployment that set them correctly. Raised at the read of either type,
    /// whichever comes first.
    #[error(
        "the namespace `{namespace}` is declared by {} configuration types — {}. A namespace \
         belongs to one type, so that a variable under it names the type that reads it: give \
         each its own `#[config(namespace = \"…\")]`, read off its own path.",
        declarations.len(),
        declarations.iter().map(|d| format!("`{d}`")).collect::<Vec<_>>().join(" and "),
    )]
    SharedNamespace {
        /// The namespace both declare.
        namespace: &'static str,
        /// Every type declaring it, sorted.
        declarations: Vec<&'static str>,
    },
    /// A `<KEY>_FILE` variable names a file that could not be read.
    ///
    /// The variable's value is never carried: an operator who pastes key
    /// material where a path belongs would otherwise see it printed, and no
    /// test of the value's shape tells a path from a headerless base64 key.
    #[error("could not read the file named by {var}")]
    File {
        /// The `<PREFIX>_<DOMAIN>__<KEY>_FILE` variable naming the file.
        var: String,
        /// Why reading it failed.
        #[source]
        source: std::io::Error,
    },
}

impl ConfigError {
    /// Build a [`Parse`](Self::Parse) error naming the variable and the reason.
    ///
    /// The one sink every refusal of a value reaches, so it is where a quoted
    /// value is removed: a line pointing into the text a parser read — the
    /// `1 | token = "…"` excerpt a TOML error opens with — is dropped, and a
    /// sentence in one of serde's quoting shapes is said without its value, as
    /// [`DecodeError::redact`] says it, whichever format worded it.
    pub fn parse(var: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Parse {
            var: var.into(),
            message: redacted(&message.into()),
        }
    }

    /// Build a [`Validation`](Self::Validation) error for `namespace`.
    pub fn validation(namespace: &'static str, errors: ValidationErrors) -> Self {
        Self::Validation { namespace, errors }
    }
}

/// `message` without the values a decoder quoted in it.
fn redacted(message: &str) -> String {
    let kept: Vec<&str> = message
        .lines()
        .filter(|line| !is_source_excerpt(line))
        .collect();
    DecodeError::redact(&kept.join("\n"), None).into_owned()
}

/// A line of a source excerpt — `1 | key = "…"`, or the gutter `  |  ^^^`
/// under it — which a parser prints to point into the text it read, and which
/// therefore repeats that text verbatim.
fn is_source_excerpt(line: &str) -> bool {
    line.trim_start()
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start()
        .starts_with('|')
}

/// One `  - field: rule` line per failure, deepest field path flattened into a
/// dotted name.
///
/// A rule's parameters are never said: `value` is the rejected input, `must_match`
/// carries the other field's under `other`, and a bound is an expression that can
/// read another field — any of them would put a secret in every log, shell
/// history and CI transcript that captures the line. Same posture as
/// `nest_rs_pipes`' wire rendering.
fn render(errors: &ValidationErrors) -> String {
    let mut out = String::new();
    render_into(&mut out, errors, "");
    out.trim_end().to_owned()
}

#[expect(
    clippy::let_underscore_must_use,
    reason = "fmt::Write for String never fails"
)]
fn render_into(out: &mut String, errors: &ValidationErrors, prefix: &str) {
    for (field, kind) in errors.errors() {
        let path = if prefix.is_empty() {
            field.to_string()
        } else {
            format!("{prefix}.{field}")
        };
        match kind {
            ValidationErrorsKind::Field(list) => {
                for error in list {
                    let _ = writeln!(out, "  - {path}: {}", error.code);
                }
            }
            ValidationErrorsKind::Struct(nested) => render_into(out, nested, &path),
            ValidationErrorsKind::List(items) => {
                for (index, nested) in items {
                    render_into(out, nested, &format!("{path}[{index}]"));
                }
            }
        }
    }
}

/// A `Result` whose error is a [`ConfigError`].
pub type Result<T> = std::result::Result<T, ConfigError>;

#[cfg(test)]
mod tests {
    use validator::Validate;

    use super::*;

    #[derive(Validate)]
    struct Issuer {
        #[validate(length(min = 1))]
        client_id: String,
        #[validate(range(min = 1, max = 500))]
        page_size: u32,
    }

    fn rendered(namespace: &'static str, value: &impl Validate) -> String {
        let errors = value.validate().expect_err("must fail");
        ConfigError::validation(namespace, errors).to_string()
    }

    // A11 / G11: the message dropped the namespace — the part that says *which*
    // config failed when several are loaded — and leaked `validator`'s raw
    // debug payload, including the rejected value, into an operator-facing line.
    #[test]
    fn a_validation_failure_names_its_namespace_and_lists_the_fields() {
        let text = rendered(
            "issuer",
            &Issuer {
                client_id: String::new(),
                page_size: 900,
            },
        );

        assert!(
            text.starts_with("configuration validation failed for 'issuer'"),
            "the namespace disambiguates which config failed: {text}",
        );
        assert!(text.contains("\n  - client_id: length"), "{text}");
        assert!(text.contains("\n  - page_size: range"), "{text}");
    }

    const SECRET: &str = "sk_live_51HsecretTOKEN";

    #[derive(Validate)]
    struct Signing {
        signing_key: String,
        #[validate(must_match(other = "signing_key"))]
        signing_key_confirm: String,
        ceiling: u64,
        #[validate(range(max = self.ceiling))]
        burst: u64,
    }

    /// A rule's parameters can hold input: `must_match`'s `other` is another
    /// field's value, and a bound is an expression that can read one.
    #[test]
    fn a_validation_failure_says_its_rules_and_none_of_their_parameters() {
        let text = rendered(
            "signing",
            &Signing {
                signing_key: SECRET.into(),
                signing_key_confirm: "mistyped".into(),
                ceiling: 4_242_424_242,
                burst: 9_999_999_999,
            },
        );
        assert!(!text.contains("sk_live"), "{text}");
        assert!(!text.contains("4242424242"), "{text}");
        assert!(
            text.contains("\n  - signing_key_confirm: must_match"),
            "{text}"
        );
        assert!(text.contains("\n  - burst: range"), "{text}");
    }

    /// A JSON decoder's sentence handed to the sink whole, as a reader judging
    /// a value of its own would hand it, quotes nothing it refused.
    #[test]
    fn a_json_decode_failure_reaches_the_boot_error_without_its_value() {
        let error = serde_json::from_str::<u16>(&format!("\"{SECRET}\""))
            .expect_err("a string is not a port");
        assert!(
            error.to_string().contains(SECRET),
            "the probe quotes: {error}"
        );

        let var = crate::var_name("fixture", "PORT");
        let text = ConfigError::parse(var.clone(), error.to_string()).to_string();
        assert!(!text.contains(SECRET), "{text}");
        assert!(
            text.starts_with(&format!("invalid value for {var}: invalid type: ")),
            "the failure is still said: {text}"
        );
    }

    /// TOML quotes twice: the excerpt it opens with repeats the line it read,
    /// and serde's sentence after it quotes the value. Neither reaches the boot
    /// error — a type error or a syntax error alike.
    #[test]
    fn a_toml_decode_failure_reaches_the_boot_error_without_its_value() {
        use figment::providers::{Format, Toml};
        use std::collections::BTreeMap;

        for (document, what) in [
            (format!("port = \"{SECRET}\""), "invalid type"),
            (format!("token = {SECRET} trailing"), "TOML parse error"),
        ] {
            let error = Toml::from_str::<BTreeMap<String, u16>>(&document)
                .expect_err("the document does not decode");
            assert!(
                error.to_string().contains(SECRET),
                "the probe quotes: {error}"
            );

            let text =
                ConfigError::parse(crate::var_name("fixture", "SETTINGS"), error.to_string())
                    .to_string();
            assert!(!text.contains(SECRET), "{text}");
            assert!(text.contains(what), "the failure is still said: {text}");
        }
    }

    /// A refusal that quotes nothing is said word for word.
    #[test]
    fn a_reason_with_nothing_quoted_is_kept_whole() {
        let text = ConfigError::parse("V", "must be at least 1 second — why").to_string();
        assert_eq!(text, "invalid value for V: must be at least 1 second — why");
    }

    #[test]
    fn the_submitted_value_and_the_bounds_are_never_said() {
        let text = rendered(
            "issuer",
            &Issuer {
                client_id: "ok".into(),
                page_size: 900,
            },
        );

        assert!(text.contains("\n  - page_size: range"), "{text}");
        // The rejected input would land in every log and CI transcript that
        // captures the line, and a bound is an expression that can read another
        // field, so neither is said.
        assert!(
            !text.contains("900"),
            "the submitted value must not be echoed: {text}",
        );
        assert!(!text.contains("500"), "no bound: {text}");
        assert!(!text.contains("Number("), "no raw debug payload: {text}");
    }
}
