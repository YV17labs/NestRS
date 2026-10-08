//! Configuration failures.

use std::fmt::Write as _;

use nest_rs_core::DecodeError;
use thiserror::Error;
use validator::{ValidationErrors, ValidationErrorsKind};

/// A configuration load failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// A value refused, naming the offending variable.
    ///
    /// Built only through [`ConfigError::parse`], which strips any value a
    /// decoder quoted in the message.
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
    /// Carries [`DecodeError`], never serde's own sentence, which quotes the
    /// value it refused.
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
    /// range — a defect in the config's `from_env`.
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
    /// Renders the **namespace** and one line per offending field, never
    /// `validator`'s `Debug` payload, which holds the submitted value.
    #[error(
        "configuration validation failed for '{namespace}'\n{}",
        render(errors)
    )]
    Validation {
        /// The `#[config(namespace = "…")]` of the config that failed.
        namespace: &'static str,
        /// The field-level failures. Not `#[source]`: a `Display` chain would
        /// print `validator`'s raw payload, submitted value included.
        errors: ValidationErrors,
    },
    /// Two config types read one environment variable, raised at boot from the
    /// resolved name.
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
    /// Two configuration types declare one namespace, raised at the read of
    /// either type, whichever comes first.
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
    /// The variable's value is never carried: it may be key material pasted
    /// where a path belongs.
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
    /// A parser's source excerpt (`1 | token = "…"`) is dropped from `message`,
    /// and a serde-quoted value removed, as [`DecodeError::redact`] does.
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
/// under it — which repeats the parsed text verbatim.
fn is_source_excerpt(line: &str) -> bool {
    line.trim_start()
        .trim_start_matches(|c: char| c.is_ascii_digit())
        .trim_start()
        .starts_with('|')
}

/// One `  - field: rule` line per failure, deepest field path flattened into a
/// dotted name.
///
/// A rule's parameters are never said: `value` is the rejected input,
/// `must_match`'s `other` another field's, and a bound can read another field.
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
        assert!(
            !text.contains("900"),
            "the submitted value must not be echoed: {text}",
        );
        assert!(!text.contains("500"), "no bound: {text}");
        assert!(!text.contains("Number("), "no raw debug payload: {text}");
    }
}
