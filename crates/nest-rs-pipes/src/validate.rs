//! Autoref-specialization probe for *global* validation — the analog of NestJS's
//! `app.useGlobalPipes(new ValidationPipe())`, minus the reflection Rust doesn't
//! have.
//!
//! A transport macro emits one call for every typed input:
//! `ValidateProbe(&input).maybe_validate()?` runs `Validate::validate` when
//! `T: Validate`, and is a compile-time no-op for any other type.
//!
//! Bring the fallback trait into scope at the call site for the resolution to
//! work: `use nest_rs_pipes::__private::MaybeValidateFallback as _;`.

use validator::{Validate, ValidationErrors};

use crate::PipeError;

/// Turn a `validator` failure into a [`PipeError`] whose `details` carry the
/// field-level errors without any echoed submitted value ([`validation_details`]).
pub(crate) fn validation_error(errors: ValidationErrors) -> PipeError {
    PipeError::with_details("validation failed", validation_details(&errors))
}

/// The wire-safe JSON for a `validator` failure.
///
/// A failure keeps its field name, `code` and `message` at every depth and no
/// `params`, which echo submitted input (`params.value`, `params.other`, a bound
/// reading another field). Any layer putting `ValidationErrors` on a wire must use
/// it. An unserializable error map collapses to `Null`.
pub fn validation_details(errors: &ValidationErrors) -> serde_json::Value {
    let mut details = serde_json::to_value(errors).unwrap_or(serde_json::Value::Null);
    redact_submitted_values(&mut details);
    details
}

/// Recursively drop the `params` of serialized `validator` errors. Nested
/// (`#[validate(nested)]`) and list validations embed further error maps, so
/// the walk descends through every object and array.
fn redact_submitted_values(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            if matches!(map.get("params"), Some(serde_json::Value::Object(_))) {
                map.remove("params");
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

    #[derive(Validate)]
    struct Transfer {
        ceiling: u64,
        #[validate(range(max = self.ceiling))]
        amount: u64,
    }

    /// A bound is an expression that can read another field, so the details say
    /// the rule and none of its parameters.
    #[test]
    fn the_details_say_the_rule_and_none_of_its_parameters() {
        let transfer = Transfer {
            ceiling: 4_242_424_242,
            amount: 9_999_999_999,
        };
        let Err(errors) = transfer.validate() else {
            panic!("an amount above its ceiling must fail the range rule");
        };
        let details = validation_details(&errors);
        assert_eq!(details["amount"][0]["code"], "range");
        assert!(details["amount"][0].get("params").is_none(), "{details}");
        assert!(!details.to_string().contains("4242424242"), "{details}");
    }

    #[test]
    fn a_non_validate_type_is_a_no_op() {
        let plain = Plain {
            _name: "anything".into(),
        };
        assert!(ValidateProbe(&plain).maybe_validate().is_ok());
    }
}
