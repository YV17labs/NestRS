//! Autoref-specialization probe for *global* validation — the analog of NestJS's
//! `app.useGlobalPipes(new ValidationPipe())`, minus the reflection Rust doesn't
//! have.
//!
//! A transport macro can't tell at codegen time whether a handler's input type
//! carries `validator::Validate` rules. This probe lets it emit **one uniform
//! call** for every typed input: `ValidateProbe(&input).maybe_validate()?` runs
//! `Validate::validate` when `T: Validate`, and is a no-op for any other type.
//! The dispatch is compile-time (an inherent method shadows the trait fallback),
//! so a non-`Validate` argument costs nothing.
//!
//! Bring the fallback trait into scope at the call site for the resolution to
//! work: `use nest_rs_pipes::MaybeValidateFallback as _;`.

use validator::{Validate, ValidationErrors};

use crate::PipeError;

/// Turn a `validator` failure into a [`PipeError`] whose `details` carry the
/// field-level errors **without** the echoed submitted value.
///
/// `validator` records the rejected input under `params.value` on every error,
/// and `must_match` the other field's input under `params.other`. Returning
/// them leaks what was submitted — a too-short password, the password a
/// confirmation failed to match — into the response body and anything that
/// captures it (a log, a cache, a proxy). Keep the field name, the
/// `code`/`message`, and the constraint parameters that make the message
/// actionable ([`CONSTRAINT_PARAMS`]), at every nesting depth; every other
/// parameter goes, a custom rule's included, since any of them can carry input.
/// Fail-secure: an unserializable error map collapses to `Null` rather than
/// surfacing raw input. Shared by
/// every validation entry point ([`ValidateProbe`] here,
/// [`ValidationPipe`](crate::ValidationPipe) in `pipes/`) so no transport can
/// echo the credential.
pub(crate) fn validation_error(errors: ValidationErrors) -> PipeError {
    PipeError::with_details("validation failed", validation_details(&errors))
}

/// The wire-safe JSON for a `validator` failure — the field errors with every
/// echoed input stripped, as `validation_error` ships them.
///
/// Public because the redaction is the *policy*, not an implementation detail
/// of pipes: any layer that puts `ValidationErrors` on a wire (a service-level
/// `ServiceError::Validation`, a transport's own renderer) has to inherit it,
/// or the framework leaks the submitted value on the one path that skipped the
/// pipe.
pub fn validation_details(errors: &ValidationErrors) -> serde_json::Value {
    let mut details = serde_json::to_value(errors).unwrap_or(serde_json::Value::Null);
    redact_submitted_values(&mut details);
    details
}

/// The parameters `validator`'s own rules set to describe a constraint:
/// `length`'s and `range`'s bounds, and the substring `contains` looks for.
const CONSTRAINT_PARAMS: [&str; 6] = [
    "min",
    "max",
    "equal",
    "exclusive_min",
    "exclusive_max",
    "needle",
];

/// Recursively keep only the [`CONSTRAINT_PARAMS`] of serialized `validator`
/// errors. Nested (`#[validate(nested)]`) and list validations embed further
/// error maps, so the walk descends through every object and array.
fn redact_submitted_values(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if let Some(serde_json::Value::Object(params)) = map.get_mut("params") {
                params.retain(|key, _| CONSTRAINT_PARAMS.contains(&key.as_str()));
            }
            for nested in map.values_mut() {
                redact_submitted_values(nested);
            }
        }
        serde_json::Value::Array(items) => {
            for item in items.iter_mut() {
                redact_submitted_values(item);
            }
        }
        _ => {}
    }
}

/// Wraps a borrowed input so method resolution can pick the `Validate`-aware
/// inherent method over the no-op trait fallback.
pub struct ValidateProbe<'a, T>(pub &'a T);

/// Specialized path: an **inherent** method (higher priority than the trait
/// fallback) exists only when `T: Validate`, so this runs the real validation.
impl<T: Validate> ValidateProbe<'_, T> {
    /// Run `validator::Validate` on the wrapped value (the specialized path when
    /// `T: Validate`).
    pub fn maybe_validate(&self) -> Result<(), PipeError> {
        match self.0.validate() {
            Ok(()) => Ok(()),
            Err(errors) => Err(validation_error(errors)),
        }
    }
}

/// Fallback path: available for **every** `T`, so an input type without
/// `Validate` resolves here and skips validation. Shadowed by the inherent
/// method above whenever `T: Validate`.
pub trait MaybeValidateFallback {
    /// No-op validation — the fallback for a `T` that does not implement
    /// `Validate`.
    fn maybe_validate(&self) -> Result<(), PipeError>;
}

impl<T> MaybeValidateFallback for ValidateProbe<'_, T> {
    fn maybe_validate(&self) -> Result<(), PipeError> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Validate)]
    struct Guarded {
        #[validate(length(min = 1))]
        name: String,
    }

    struct Plain {
        _name: String,
    }

    #[derive(Validate)]
    struct PasswordChange {
        #[validate(length(min = 12))]
        password: String,
        #[validate(must_match(other = "password"))]
        confirmation: String,
    }

    #[test]
    fn a_validate_type_that_passes_is_ok() {
        let ok = Guarded { name: "x".into() };
        assert!(ValidateProbe(&ok).maybe_validate().is_ok());
    }

    #[test]
    fn a_validate_type_that_fails_surfaces_the_error_with_details() {
        let bad = Guarded {
            name: String::new(),
        };
        let Err(err) = ValidateProbe(&bad).maybe_validate() else {
            panic!("empty name must fail the length rule");
        };
        assert_eq!(err.message(), "validation failed");
        assert!(err.details().is_some(), "field-level details are carried");
    }

    #[test]
    fn the_details_keep_the_constraints_and_drop_every_submitted_value() {
        let change = PasswordChange {
            password: "hunter2-hunter".into(),
            confirmation: "hunter3-hunter".into(),
        };
        let Err(errors) = change.validate() else {
            panic!("a confirmation that differs must fail must_match");
        };
        let details = validation_details(&errors).to_string();
        assert!(
            !details.contains("hunter"),
            "a submitted value survived: {details}"
        );

        let short = PasswordChange {
            password: "short".into(),
            confirmation: "short".into(),
        };
        let Err(errors) = short.validate() else {
            panic!("a short password must fail the length rule");
        };
        let details = validation_details(&errors);
        assert_eq!(
            details["password"][0]["params"],
            serde_json::json!({ "min": 12 })
        );
    }

    #[test]
    fn a_non_validate_type_is_a_no_op() {
        // `Plain` does not implement `Validate`; the fallback runs and passes.
        let plain = Plain {
            _name: "anything".into(),
        };
        assert!(ValidateProbe(&plain).maybe_validate().is_ok());
    }
}
