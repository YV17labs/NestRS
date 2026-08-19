//! Configuration failures.

use std::fmt::Write as _;

use thiserror::Error;
use validator::{ValidationErrors, ValidationErrorsKind};

/// A configuration load failure.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Names the offending variable so the misconfig is obvious at boot.
    #[error("invalid value for {var}: {message}")]
    Parse {
        /// The offending `<PREFIX>_<DOMAIN>__<KEY>` variable name.
        var: String,
        /// Why the value was rejected.
        message: String,
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
    /// `<PREFIX>_<DOMAIN>__<KEY>` is a flat, process-global name space, and
    /// several types sharing a `<DOMAIN>` is deliberate — `nest-rs-authn`'s JWT,
    /// OAuth and protected-resource configs are all `authn`, because a domain is
    /// the operator's word for a subsystem rather than one struct's identity.
    /// What may not be shared is a **variable**: two types reading one name means
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
}

impl ConfigError {
    /// Build a [`Parse`](Self::Parse) error naming the variable and the reason.
    pub fn parse(var: impl Into<String>, message: impl Into<String>) -> Self {
        Self::Parse {
            var: var.into(),
            message: message.into(),
        }
    }

    /// Build a [`Validation`](Self::Validation) error for `namespace`.
    pub fn validation(namespace: &'static str, errors: ValidationErrors) -> Self {
        Self::Validation { namespace, errors }
    }
}

/// One `  - field: rule (bound = n)` line per failure, deepest field path
/// flattened into a dotted name.
///
/// The rule's parameters are kept — a bound is what makes the message
/// actionable — **except** `value`, which is the rejected input itself:
/// echoing a too-short password or a malformed token into a boot error puts it
/// in every log, shell history and CI transcript that captures the line. Same
/// posture as `nest_rs_pipes`' wire rendering.
fn render(errors: &ValidationErrors) -> String {
    let mut out = String::new();
    render_into(&mut out, errors, "");
    out.trim_end().to_owned()
}

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
                    let params: Vec<String> = error
                        .params
                        .iter()
                        .filter(|(name, _)| name.as_ref() != "value")
                        .map(|(name, value)| format!("{name} = {value}"))
                        .collect();
                    let _ = write!(out, "  - {path}: {}", error.code);
                    if !params.is_empty() {
                        let _ = write!(out, " ({})", params.join(", "));
                    }
                    out.push('\n');
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

    #[test]
    fn the_bounds_survive_but_the_submitted_value_never_does() {
        let text = rendered(
            "issuer",
            &Issuer {
                client_id: "ok".into(),
                page_size: 900,
            },
        );

        // The bound is what makes the message actionable.
        assert!(
            text.contains("min = 1") && text.contains("max = 500"),
            "{text}"
        );
        // The rejected input is not: a too-short password or a malformed token
        // would otherwise land in every log and CI transcript that captures it.
        assert!(
            !text.contains("900"),
            "the submitted value must not be echoed: {text}",
        );
        assert!(!text.contains("Number("), "no raw debug payload: {text}");
    }
}
